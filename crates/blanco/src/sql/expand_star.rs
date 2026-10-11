//! The "Expand * into columns" code action: replaces `*` or `alias.*` in a
//! select list with the columns it stands for.

use std::collections::HashMap;
use std::ops::Range;

use blanco_core::DatabaseType;
use gpui::{App, AppContext as _, Entity, SharedString, Task, Window};
use gpui_component::input::{CodeActionProvider, EditorState, Rope, RopeExt as _};
use sql_parser::statement_parser::{self, Relation, ident_eq};

use super::completion::{SqlCompletionProvider, is_sql_keyword};

/// A `*` in a select list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StarAt {
    /// The `*`, or `alias.*` with its qualifier.
    range: Range<usize>,
    /// The table or alias before `.*`, unquoted for matching.
    qualifier: Option<String>,
    /// That qualifier as written, which the expansion keeps.
    written_qualifier: Option<String>,
}

/// The `*` at or just before `offset`, if it stands for columns: right after
/// SELECT, DISTINCT, ALL or a comma, or as `name.*`. A `*` that multiplies or
/// sits in `count(*)` is not one.
fn star_at(text: &str, offset: usize) -> Option<StarAt> {
    let bytes = text.as_bytes();
    let star = [offset, offset.checked_sub(1)?]
        .into_iter()
        .find(|&i| bytes.get(i) == Some(&b'*'))?;

    let after = text[star + 1..].trim_start();
    let ends_item = after.is_empty()
        || after.starts_with([',', ';', ')'])
        || after
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("FROM"));
    if !ends_item {
        return None;
    }

    let before = text[..star].trim_end();
    if let Some(qualified) = before.strip_suffix('.') {
        let start = qualified
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_alphanumeric() || matches!(c, '_' | '"' | '`' | '[' | ']')))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let qualifier = &qualified[start..];
        if qualifier.is_empty() {
            return None;
        }
        return Some(StarAt {
            range: start..star + 1,
            qualifier: Some(unquote(qualifier)),
            written_qualifier: Some(qualifier.to_string()),
        });
    }

    let last_word = before
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("");
    let follows_list_start = before.ends_with(',')
        || ["SELECT", "DISTINCT", "ALL"]
            .iter()
            .any(|keyword| last_word.eq_ignore_ascii_case(keyword));
    follows_list_start.then_some(StarAt {
        range: star..star + 1,
        qualifier: None,
        written_qualifier: None,
    })
}

fn unquote(name: &str) -> String {
    statement_parser::strip_identifier_quotes(name).to_string()
}

/// `name` as the dialect needs it written: bare when it is a plain word the
/// database would read back unchanged, quoted otherwise.
fn column_name(driver: DatabaseType, name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !is_sql_keyword(name);
    // PostgreSQL folds bare names to lower case, so a capital needs quotes.
    let folds = driver == DatabaseType::PostgreSQL && name.chars().any(|c| c.is_ascii_uppercase());
    if plain && !folds {
        name.to_string()
    } else {
        driver.dialect().quote_identifier(name)
    }
}

/// The text that replaces the star: the columns of the tables it covers,
/// prefixed with the table's alias or name when it is qualified or more than
/// one table is in play. `None` when a table's columns are unknown.
async fn expansion(
    provider: &SqlCompletionProvider,
    star: &StarAt,
    relations: &[Relation],
) -> Option<String> {
    let (_, supports_schemas) = provider.get_cached_schemas().await.ok()?;
    let reference = |relation: &Relation| {
        relation.alias.clone().unwrap_or_else(|| {
            relation
                .table_name
                .rsplit('.')
                .next()
                .unwrap_or(&relation.table_name)
                .to_string()
        })
    };
    let covered: Vec<&Relation> = match &star.qualifier {
        Some(qualifier) => vec![relations.iter().find(|relation| {
            ident_eq(&reference(relation), qualifier) || ident_eq(&relation.table_name, qualifier)
        })?],
        None => relations.iter().collect(),
    };
    if covered.is_empty() {
        return None;
    }

    let prefixed = star.qualifier.is_some() || covered.len() > 1;
    let mut columns = Vec::new();
    for relation in covered {
        let (schema, table) = match relation.table_name.split_once('.') {
            Some((schema, table)) if supports_schemas => (schema.to_string(), table.to_string()),
            Some((_, table)) => (provider.current_schema.clone(), table.to_string()),
            None => (provider.current_schema.clone(), relation.table_name.clone()),
        };
        let table_columns = provider.get_cached_columns(&schema, &table).await.ok()?;
        if table_columns.is_empty() {
            return None;
        }
        let prefix = match &star.written_qualifier {
            Some(qualifier) => format!("{qualifier}."),
            None if prefixed => format!("{}.", reference(relation)),
            None => String::new(),
        };
        columns.extend(
            table_columns
                .iter()
                .map(|column| format!("{prefix}{}", column_name(provider.driver, &column.name))),
        );
    }
    Some(columns.join(", "))
}

/// Offers "Expand * into columns" with the caret on a select-list `*`.
pub struct ExpandStarProvider {
    provider: SqlCompletionProvider,
}

impl ExpandStarProvider {
    pub fn new(provider: SqlCompletionProvider) -> Self {
        Self { provider }
    }
}

const TITLE: &str = "Expand * into columns";

impl CodeActionProvider for ExpandStarProvider {
    fn id(&self) -> SharedString {
        "blanco-expand-star".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<lsp_types::CodeAction>>> {
        let rope = state.read(cx).text().clone();
        let provider = self.provider.clone();
        cx.background_spawn(async move {
            let text = rope.to_string();
            let Some(star) = star_at(&text, range.start) else {
                return Ok(Vec::new());
            };
            let relations = statement_parser::extract_relations(&rope, star.range.start);
            let Some(new_text) = expansion(&provider, &star, &relations).await else {
                return Ok(Vec::new());
            };
            Ok(vec![code_action(&rope, star.range, new_text)])
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: lsp_types::CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        let edits: Vec<lsp_types::TextEdit> = action
            .edit
            .and_then(|edit| edit.changes)
            .map(|changes| changes.into_values().flatten().collect())
            .unwrap_or_default();
        let state = state.downgrade();
        window.spawn(cx, async move |cx| {
            state.update_in(cx, |state, window, cx| {
                state.apply_lsp_edits(&edits, window, cx);
            })
        })
    }
}

fn code_action(rope: &Rope, range: Range<usize>, new_text: String) -> lsp_types::CodeAction {
    let range = lsp_types::Range::new(
        rope.offset_to_position(range.start),
        rope.offset_to_position(range.end),
    );
    let uri: lsp_types::Uri = "blanco-editor:query".parse().expect("valid uri");
    lsp_types::CodeAction {
        title: TITLE.to_string(),
        kind: Some(lsp_types::CodeActionKind::REFACTOR_REWRITE),
        edit: Some(lsp_types::WorkspaceEdit {
            changes: Some(HashMap::from([(
                uri,
                vec![lsp_types::TextEdit { range, new_text }],
            )])),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(sql: &str, needle: &str) -> Option<StarAt> {
        star_at(sql, sql.find(needle)? + needle.find('*')?)
    }

    #[test]
    fn finds_select_list_stars_only() {
        let sql = "SELECT *, u.* FROM users u WHERE count(*) > 2 * 3";
        assert_eq!(
            star(sql, "*,"),
            Some(StarAt {
                range: 7..8,
                qualifier: None,
                written_qualifier: None,
            })
        );
        let qualified = star(sql, "u.*").unwrap();
        assert_eq!(&sql[qualified.range.clone()], "u.*");
        assert_eq!(qualified.qualifier.as_deref(), Some("u"));
        assert_eq!(star(sql, "(*)"), None);
        assert_eq!(star(sql, "2 * 3"), None);
        // The caret just after the star still counts.
        assert!(star_at("SELECT * FROM t", 8).is_some());
        assert!(star_at("SELECT DISTINCT *", 16).is_some());
    }

    #[test]
    fn names_are_quoted_only_when_needed() {
        assert_eq!(column_name(DatabaseType::PostgreSQL, "user_id"), "user_id");
        assert_eq!(
            column_name(DatabaseType::PostgreSQL, "UserId"),
            "\"UserId\""
        );
        assert_eq!(column_name(DatabaseType::MySQL, "UserId"), "UserId");
        assert_eq!(column_name(DatabaseType::MySQL, "order"), "`order`");
        assert_eq!(column_name(DatabaseType::MsSql, "full name"), "[full name]");
    }

    async fn expand_at(
        harness: &crate::test_harness::TestHarness,
        cx: &mut gpui::VisualTestContext,
        editor: &Entity<EditorState>,
        expand: &ExpandStarProvider,
        sql: &str,
        needle: &str,
    ) -> String {
        crate::test_harness::set_editor_text(harness, sql, cx);
        let offset = sql.find(needle).unwrap() + needle.find('*').unwrap();
        let actions = cx
            .update(|window, cx| expand.code_actions(editor.clone(), offset..offset, window, cx))
            .await
            .unwrap();
        assert_eq!(actions.len(), 1, "one action at {needle:?} in {sql:?}");
        cx.update(|window, cx| {
            expand.perform_code_action(editor.clone(), actions[0].clone(), true, window, cx)
        })
        .await
        .unwrap();
        editor.read_with(cx, |editor, _| editor.text().to_string())
    }

    /// The action rewrites the editor text with the tables' columns.
    #[gpui::test]
    async fn expands_stars_in_the_editor(cx: &mut gpui::TestAppContext) {
        use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

        let harness = TestHarness::new(cx);
        let mut cx = gpui::VisualTestContext::from_window(harness.window_handle.into(), cx);
        for sql in [
            "CREATE TABLE users (id INTEGER, \"Full Name\" TEXT)",
            "CREATE TABLE orders (id INTEGER, user_id INTEGER)",
        ] {
            set_editor_text(&harness, sql, &mut cx);
            run_query(&harness, &mut cx);
            wait_for_query(&harness, &mut cx).await;
        }
        let (editor, provider) = harness.editor_panel.read_with(&cx, |panel, _| {
            let tab = panel.active_query_tab().expect("query tab");
            (
                tab.base.editor.clone(),
                tab.completion_provider
                    .clone()
                    .expect("completion provider"),
            )
        });
        let expand = ExpandStarProvider::new(provider);

        let cases = [
            (
                "SELECT * FROM users",
                "*",
                "SELECT id, \"Full Name\" FROM users",
            ),
            (
                "SELECT u.* FROM users u JOIN orders o ON o.user_id = u.id",
                "u.*",
                "SELECT u.id, u.\"Full Name\" FROM users u JOIN orders o ON o.user_id = u.id",
            ),
            (
                "SELECT * FROM users u JOIN orders ON true",
                "*",
                "SELECT u.id, u.\"Full Name\", orders.id, orders.user_id FROM users u JOIN orders ON true",
            ),
        ];
        for (sql, needle, expected) in cases {
            let expanded = expand_at(&harness, &mut cx, &editor, &expand, sql, needle).await;
            assert_eq!(expanded, expected);
        }
    }
}
