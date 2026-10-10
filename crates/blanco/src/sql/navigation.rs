//! Going from a name in a query to the table, view or function it names:
//! Ctrl/Cmd-click (through the editor's definition provider) and the editor's
//! Open Data and Open Structure menu items.

use std::ops::Range;
use std::rc::Rc;

use blanco_core::{ConnectionContext, EntityType, RoutineKind};
use gpui::{App, AppContext as _, Task, Window};
use gpui_component::input::{DefinitionProvider, Rope, ShowDocumentHandler};
use sql_parser::statement_parser::{self, ident_eq};

use super::completion::SqlCompletionProvider;
use crate::app::{CreateNewQueryTab, OpenObjectDdl, OpenTableStructure};

/// An object a name in the editor resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTarget {
    Relation {
        entity: EntityType,
        schema: Option<String>,
        name: String,
    },
    Function {
        schema: Option<String>,
        name: String,
    },
}

/// The dotted name under an offset, such as `public.users` or `u.id`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NameAt {
    /// The parts of the dotted name, quotes removed.
    parts: Vec<String>,
    /// Which part the offset is in.
    index: usize,
    /// Byte range of that part in the text.
    range: Range<usize>,
    /// Whether an opening parenthesis follows the name, making it a call.
    is_call: bool,
}

/// Find the dotted name whose part covers `offset`. Strings and comments are
/// skipped, so a word inside them names nothing.
fn name_at(text: &str, offset: usize) -> Option<NameAt> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // Skip string literals and comments.
        if b == b'\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\'' {
                i += 1;
            }
            i += 1;
            continue;
        }
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if is_ident_start(b) || matches!(b, b'"' | b'`' | b'[') {
            let mut parts = Vec::new();
            let mut ranges = Vec::new();
            loop {
                let (part, range) = identifier(bytes, i)?;
                parts.push(part);
                ranges.push(range.clone());
                i = range.end;
                if bytes.get(i) == Some(&b'.')
                    && bytes.get(i + 1).is_some_and(|&next| {
                        is_ident_start(next) || matches!(next, b'"' | b'`' | b'[')
                    })
                {
                    i += 1;
                    continue;
                }
                break;
            }
            if let Some(index) = ranges
                .iter()
                .position(|range| range.start <= offset && offset <= range.end)
            {
                let rest = &text[ranges.last()?.end..];
                let is_call = index + 1 == parts.len() && rest.trim_start().starts_with('(');
                return Some(NameAt {
                    range: ranges[index].clone(),
                    parts,
                    index,
                    is_call,
                });
            }
            continue;
        }
        i += 1;
    }
    None
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// The identifier starting at `start`, unquoted, and its byte range.
fn identifier(bytes: &[u8], start: usize) -> Option<(String, Range<usize>)> {
    let close = match bytes[start] {
        b'"' => Some(b'"'),
        b'`' => Some(b'`'),
        b'[' => Some(b']'),
        _ => None,
    };
    let end = match close {
        Some(close) => {
            let inner = bytes[start + 1..].iter().position(|&b| b == close)?;
            start + 1 + inner + 1
        }
        None => {
            let mut end = start;
            while end < bytes.len() && is_ident_char(bytes[end]) {
                end += 1;
            }
            end
        }
    };
    let raw = std::str::from_utf8(&bytes[start..end]).ok()?;
    let name = match close {
        Some(_) => raw[1..raw.len() - 1].to_string(),
        None => raw.to_string(),
    };
    Some((name, start..end))
}

/// Resolve the name under `offset` to the object it names, reading the
/// schema's tables and functions through the completion cache. Aliases from
/// the statement are followed to their table.
pub async fn resolve(
    provider: &SqlCompletionProvider,
    rope: &Rope,
    offset: usize,
) -> Option<(NavigationTarget, Range<usize>)> {
    let text = rope.to_string();
    let name_at = name_at(&text, offset)?;
    let name = name_at.parts[name_at.index].clone();
    let qualifier = name_at
        .index
        .checked_sub(1)
        .map(|index| name_at.parts[index].clone());
    let (_, supports_schemas) = provider.get_cached_schemas().await.ok()?;
    let schema_of = |qualifier: Option<String>| -> String {
        match qualifier {
            Some(schema) if supports_schemas => schema,
            _ => provider.current_schema.clone(),
        }
    };
    let shown_schema = |schema: String| supports_schemas.then_some(schema);

    if name_at.is_call {
        let schema = schema_of(qualifier);
        let functions = provider.get_cached_functions(&schema).await.ok()?;
        let function = functions.iter().find(|f| ident_eq(&f.name, &name))?;
        return Some((
            NavigationTarget::Function {
                schema: shown_schema(schema),
                name: function.name.clone(),
            },
            name_at.range,
        ));
    }

    // An unqualified name may be an alias of a table in the statement.
    let (qualifier, name) = match qualifier {
        Some(qualifier) => (Some(qualifier), name),
        None => statement_parser::extract_completion_context(rope, name_at.range.end)
            .and_then(|context| {
                context
                    .table_aliases
                    .iter()
                    .find(|alias| ident_eq(&alias.alias, &name))
                    .map(|alias| alias.table_name.clone())
            })
            .map(|table| match table.rsplit_once('.') {
                Some((schema, table)) => (Some(unquote(schema)), unquote(table)),
                None => (None, unquote(&table)),
            })
            .unwrap_or((None, name)),
    };

    let schema = schema_of(qualifier);
    let tables = provider.get_cached_tables(&schema).await.ok()?;
    let entity = tables.iter().find(|entity| ident_eq(&entity.name, &name))?;
    Some((
        NavigationTarget::Relation {
            entity: entity.entity_type,
            schema: shown_schema(schema),
            name: entity.name.clone(),
        },
        name_at.range,
    ))
}

fn unquote(name: &str) -> String {
    let name = name.trim();
    for (open, close) in [('"', '"'), ('`', '`'), ('[', ']')] {
        if let Some(inner) = name.strip_prefix(open).and_then(|n| n.strip_suffix(close)) {
            return inner.to_string();
        }
    }
    name.to_string()
}

/// Open `target` from a tab on `context`: a table or view as a query tab
/// showing its rows, or with `structure` as its structure tab. A function opens
/// its DDL either way.
pub fn open_target(
    target: NavigationTarget,
    structure: bool,
    context: &ConnectionContext,
    window: &mut Window,
    cx: &mut App,
) {
    let in_schema = |schema: Option<String>| ConnectionContext {
        schema_name: schema.or_else(|| context.schema_name.clone()),
        ..context.clone()
    };
    let action: Box<dyn gpui::Action> = match target {
        NavigationTarget::Relation { schema, name, .. } if structure => {
            Box::new(OpenTableStructure {
                table_name: name,
                context: in_schema(schema),
            })
        }
        NavigationTarget::Relation { schema, name, .. } => Box::new(CreateNewQueryTab {
            context: in_schema(schema),
            table_name: Some(name),
            inspect_key: false,
        }),
        NavigationTarget::Function { schema, name } => Box::new(OpenObjectDdl {
            kind: RoutineKind::Function,
            context: in_schema(schema),
            object_name: name,
        }),
    };
    window.dispatch_action(action, cx);
}

/// The editor's `show_document` hook: following a definition opens the
/// object's rows (or a function's DDL) in a new tab.
pub fn show_document_handler(context: ConnectionContext) -> ShowDocumentHandler {
    Rc::new(move |params, window, cx| {
        let Some(target) = NavigationTarget::from_uri(&params.uri) else {
            return false;
        };
        open_target(target, false, &context, window, cx);
        true
    })
}

const URI_SCHEME: &str = "blanco-object";

impl NavigationTarget {
    /// The target as a URI, which is how a definition reaches the editor's
    /// `show_document` hook.
    pub fn to_uri(&self) -> Option<lsp_types::Uri> {
        let (sort, schema, name) = match self {
            NavigationTarget::Relation {
                entity,
                schema,
                name,
            } => (
                match entity {
                    EntityType::Table => "table",
                    EntityType::View => "view",
                    EntityType::MaterializedView => "matview",
                },
                schema,
                name,
            ),
            NavigationTarget::Function { schema, name } => ("function", schema, name),
        };
        let schema = schema.as_deref().map(encode).unwrap_or_default();
        format!("{URI_SCHEME}:{sort}/{schema}/{}", encode(name))
            .parse()
            .ok()
    }

    /// Read a target back from [`Self::to_uri`].
    pub fn from_uri(uri: &lsp_types::Uri) -> Option<Self> {
        let text = uri.as_str().strip_prefix(URI_SCHEME)?.strip_prefix(':')?;
        let mut parts = text.splitn(3, '/');
        let (sort, schema, name) = (parts.next()?, parts.next()?, parts.next()?);
        let schema = (!schema.is_empty()).then(|| decode(schema)).flatten();
        let name = decode(name)?;
        let entity = match sort {
            "table" => EntityType::Table,
            "view" => EntityType::View,
            "matview" => EntityType::MaterializedView,
            "function" => return Some(NavigationTarget::Function { schema, name }),
            _ => return None,
        };
        Some(NavigationTarget::Relation {
            entity,
            schema,
            name,
        })
    }
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Answers Go to Definition in a query editor with a link to the table, view
/// or function under the cursor.
pub struct SqlDefinitionProvider {
    provider: SqlCompletionProvider,
}

impl SqlDefinitionProvider {
    pub fn new(provider: SqlCompletionProvider) -> Rc<Self> {
        Rc::new(Self { provider })
    }
}

impl DefinitionProvider for SqlDefinitionProvider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<lsp_types::LocationLink>>> {
        let provider = self.provider.clone();
        let text = text.clone();
        cx.background_spawn(async move {
            let Some((target, range)) = resolve(&provider, &text, offset).await else {
                return Ok(Vec::new());
            };
            let Some(uri) = target.to_uri() else {
                return Ok(Vec::new());
            };
            let origin =
                lsp_types::Range::new(position(&text, range.start), position(&text, range.end));
            Ok(vec![lsp_types::LocationLink {
                origin_selection_range: Some(origin),
                target_uri: uri,
                target_range: lsp_types::Range::default(),
                target_selection_range: lsp_types::Range::default(),
            }])
        })
    }
}

/// The LSP position of byte `offset`, in UTF-16 code units as LSP counts.
fn position(text: &Rope, offset: usize) -> lsp_types::Position {
    use gpui_component::input::RopeExt as _;
    text.offset_to_position(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, needle: &str) -> Option<NameAt> {
        name_at(text, text.find(needle)? + 1)
    }

    #[test]
    fn finds_the_dotted_name_under_the_offset() {
        let sql = "SELECT u.id FROM public.\"Users\" u JOIN orders o ON count(o.id) > 0";
        let users = at(sql, "\"Users\"").unwrap();
        assert_eq!(users.parts, ["public", "Users"]);
        assert_eq!(users.index, 1);
        assert_eq!(&sql[users.range], "\"Users\"");

        let alias = at(sql, "u.id").unwrap();
        assert_eq!(
            (alias.parts.clone(), alias.index),
            (vec!["u".into(), "id".into()], 0)
        );

        let call = at(sql, "count").unwrap();
        assert!(call.is_call);
        assert!(!at(sql, "orders").unwrap().is_call);
    }

    #[test]
    fn strings_and_comments_name_nothing() {
        let sql = "SELECT 'orders' -- orders\nFROM t /* orders */";
        assert!(at(sql, "orders'").is_none());
        assert!(name_at(sql, sql.find("-- orders").unwrap() + 4).is_none());
        assert!(name_at(sql, sql.find("/* orders").unwrap() + 4).is_none());
        assert_eq!(at(sql, "t /*").unwrap().parts, ["t"]);
    }

    #[test]
    fn targets_round_trip_through_uris() {
        for target in [
            NavigationTarget::Relation {
                entity: EntityType::View,
                schema: Some("sales data".into()),
                name: "Order/Items".into(),
            },
            NavigationTarget::Function {
                schema: None,
                name: "total_for".into(),
            },
        ] {
            let uri = target.to_uri().expect("valid uri");
            assert_eq!(NavigationTarget::from_uri(&uri), Some(target));
        }
    }

    /// Table names and their aliases resolve through the tab's completion
    /// cache; other words resolve to nothing.
    #[gpui::test]
    async fn names_and_aliases_resolve_to_tables(cx: &mut gpui::TestAppContext) {
        use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

        let harness = TestHarness::new(cx);
        let mut cx = gpui::VisualTestContext::from_window(harness.window_handle.into(), cx);
        set_editor_text(&harness, "CREATE TABLE orders (id INTEGER)", &mut cx);
        run_query(&harness, &mut cx);
        wait_for_query(&harness, &mut cx).await;

        let provider = harness.editor_panel.read_with(&cx, |panel, _| {
            panel
                .active_query_tab()
                .and_then(|tab| tab.completion_provider.clone())
                .expect("query tab has a completion provider")
        });
        provider.invalidate_cache();
        let sql = "SELECT o.id FROM orders o WHERE missing = 1";
        let rope = Rope::from_str(sql);
        let resolve_at = |needle: &str| {
            let provider = provider.clone();
            let rope = rope.clone();
            let offset = sql.find(needle).unwrap() + 1;
            async move { resolve(&provider, &rope, offset).await }
        };

        let orders = NavigationTarget::Relation {
            entity: EntityType::Table,
            schema: None,
            name: "orders".into(),
        };
        let (target, range) = resolve_at("orders").await.expect("orders resolves");
        assert_eq!(target, orders);
        assert_eq!(&sql[range], "orders");
        let (target, range) = resolve_at("o.id").await.expect("the alias resolves");
        assert_eq!(target, orders);
        assert_eq!(&sql[range], "o");
        assert_eq!(resolve_at("missing").await, None);
    }
}
