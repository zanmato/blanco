//! Completion provider for JavaScript script tabs.
//!
//! The injected `db` object is the only API a script tab has that isn't plain
//! JavaScript, and it is documented nowhere the editor can reach, so it is
//! exactly the thing worth completing. Two cases are handled:
//!
//! - after `db.`, offer the `db` methods with their signatures and docs;
//! - after `<expr>.` where `<expr>` came out of `db.query(...)`, offer the
//!   fields of a query result.
//!
//! Everything else (user variables, JS builtins) is left alone: guessing at
//! them without a real JS analysis would offer wrong completions confidently.

use anyhow::Result;
use gpui::{Context, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, MarkupContent, MarkupKind, Range, TextEdit,
};

/// A member of one of the completable objects.
struct Member {
    name: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    documentation: &'static str,
}

const DB_MEMBERS: &[Member] = &[
    Member {
        name: "query",
        kind: CompletionItemKind::METHOD,
        detail: "query(sql, params?) -> QueryResult",
        documentation: "Run a statement and return its rows.\n\n\
             Returns `{ columns, rows, rowsAffected, executionTimeMs }`, where each row is an \
             object keyed by column name whose values are strings or `null`.\n\n\
             Blocks until the database answers.",
    },
    Member {
        name: "execute",
        kind: CompletionItemKind::METHOD,
        detail: "execute(sql, params?) -> number",
        documentation: "Run a statement and return the number of affected rows.\n\n\
             Use this for INSERT/UPDATE/DELETE/DDL where the rows themselves don't matter.",
    },
    Member {
        name: "transaction",
        kind: CompletionItemKind::METHOD,
        detail: "transaction(statements) -> { rowsAffected, operationsExecuted }",
        documentation: "Run an array of statements as one atomic unit.\n\n\
             Backends with transactional DML roll the whole batch back on failure; those without \
             (Redis, ClickHouse) report how many statements were applied before the error. \
             Statements that cannot run inside a transaction (e.g. Postgres `DROP DATABASE`) \
             belong in separate `db.execute` calls.",
    },
    Member {
        name: "display",
        kind: CompletionItemKind::METHOD,
        detail: "display(value) -> void",
        documentation: "Render a value in the results grid.\n\n\
             A result from `db.query` keeps its original column types. Anything else is \
             converted: an array of objects becomes rows and columns, an object becomes a \
             single row, a scalar becomes a single cell.",
    },
];

const RESULT_MEMBERS: &[Member] = &[
    Member {
        name: "rows",
        kind: CompletionItemKind::FIELD,
        detail: "Array<{ [column: string]: string | null }>",
        documentation: "The returned rows, each an object keyed by column name. \
             SQL NULL arrives as `null`; every other value is a string.",
    },
    Member {
        name: "columns",
        kind: CompletionItemKind::FIELD,
        detail: "Array<string>",
        documentation: "Column names in order. Unlike the row objects this preserves \
             duplicate column names.",
    },
    Member {
        name: "rowsAffected",
        kind: CompletionItemKind::FIELD,
        detail: "number",
        documentation: "Rows affected by the statement, as reported by the driver.",
    },
    Member {
        name: "executionTimeMs",
        kind: CompletionItemKind::FIELD,
        detail: "number | null",
        documentation: "Server-side execution time in milliseconds, when the driver reports one.",
    },
];

/// Appended to `query`/`execute` docs on backends without bind parameters,
/// where passing `params` fails at the driver. Shared by the completion items
/// and the agent's API reference so both say the same thing.
const NO_BIND_PARAMETERS_NOTE: &str =
    "This connection has no bind parameters: pass the command fully formed and omit `params`.";

/// Markdown reference for the injected `db` object, rendered from the same
/// tables the editor completes from so the agent prompt cannot drift from the
/// completion docs.
pub fn api_reference(supports_bind_parameters: bool) -> String {
    let mut reference = String::new();

    for (heading, members) in [("db", DB_MEMBERS), ("QueryResult", RESULT_MEMBERS)] {
        reference.push_str(&format!("`{heading}`:\n\n"));
        for member in members {
            // The docs are written as prose paragraphs for a hover popup; a
            // prompt reads better as one line per member.
            let mut documentation = member.documentation.replace("\n\n", " ");
            if !supports_bind_parameters && member_takes_parameters(member) {
                documentation.push(' ');
                documentation.push_str(NO_BIND_PARAMETERS_NOTE);
            }
            // A method's detail is its signature and already names it; a
            // field's is only its type.
            let signature = if member.kind == CompletionItemKind::METHOD {
                member.detail.to_string()
            } else {
                format!("{}: {}", member.name, member.detail)
            };
            reference.push_str(&format!("- `{signature}` — {documentation}\n"));
        }
        reference.push('\n');
    }

    reference
}

fn member_takes_parameters(member: &Member) -> bool {
    matches!(member.name, "query" | "execute")
}

/// The set of members to offer at the cursor.
enum Candidates {
    Db,
    QueryResult,
    None,
}

pub struct ScriptCompletionProvider {
    /// Bind parameters are a SQL notion; on key/value backends they fail at the
    /// driver, so the docs say so rather than letting a script find out at run
    /// time.
    supports_bind_parameters: bool,
}

impl ScriptCompletionProvider {
    pub fn new(db_type: database::DatabaseType) -> Self {
        Self {
            supports_bind_parameters: db_type.supports_sql(),
        }
    }

    /// For snippets, which aren't bound to a connection. Documents the general
    /// API, bind parameters included, since the snippet may end up anywhere.
    pub fn for_snippet() -> Self {
        Self {
            supports_bind_parameters: true,
        }
    }
}

/// Strip a trailing balanced `(...)` or `[...]` group from `text`, returning
/// the remainder. Used to step back over a call's arguments when resolving what
/// a member access is being performed on.
fn strip_trailing_group(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let (open, close) = match bytes.last()? {
        b')' => (b'(', b')'),
        b']' => (b'[', b']'),
        _ => return None,
    };

    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate().rev() {
        if *byte == close {
            depth += 1;
        } else if *byte == open {
            depth -= 1;
            if depth == 0 {
                return Some(&text[..index]);
            }
        }
    }
    None
}

/// Decide what the `.` before the cursor is a member access on.
///
/// This is deliberately a small textual walk rather than a JS parse: it only
/// needs to recognise `db.` and expressions that end in a `db.query(...)` call,
/// including chained indexing like `db.query(sql).rows[0]` (which yields a row
/// object, not a result, so it completes nothing).
fn classify(before_dot: &str) -> Candidates {
    let trimmed = before_dot.trim_end();

    // `db.query(...)` (possibly with whitespace around the call) is the only
    // expression whose type is known.
    if let Some(head) = strip_trailing_group(trimmed) {
        let head = head.trim_end();
        if head.ends_with("db.query") && is_standalone_db(head, head.len() - "db.query".len()) {
            return Candidates::QueryResult;
        }
        return Candidates::None;
    }

    if trimmed.ends_with("db") && is_standalone_db(trimmed, trimmed.len() - 2) {
        return Candidates::Db;
    }

    Candidates::None
}

/// Guard against matching the `db` inside a longer identifier such as
/// `mydb.` or `a.db.`.
fn is_standalone_db(text: &str, db_start: usize) -> bool {
    match text[..db_start].chars().next_back() {
        None => true,
        Some(previous) => !(previous.is_alphanumeric() || previous == '_' || previous == '$'),
    }
}

/// From the buffer text and cursor byte offset, work out which members to
/// offer, the partial identifier under the cursor, and the byte offset its
/// replacement starts at. Kept rope-free so it can be unit-tested directly.
fn analyze(text: &str, offset: usize) -> (Candidates, &str, usize) {
    let offset = offset.min(text.len());
    let left = &text[..offset];

    let word_start = left
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .map_or(0, |index| index + 1);
    let current_word = &left[word_start..];

    let before_word = &left[..word_start];
    let Some(before_dot) = before_word.strip_suffix('.') else {
        return (Candidates::None, current_word, offset - current_word.len());
    };

    (
        classify(before_dot),
        current_word,
        offset - current_word.len(),
    )
}

fn matches_prefix(candidate: &str, needle: &str) -> bool {
    needle.is_empty() || candidate.starts_with(needle)
}

fn build_items(
    candidates: Candidates,
    current_word: &str,
    range: Range,
    supports_bind_parameters: bool,
) -> Vec<CompletionItem> {
    let members = match candidates {
        Candidates::Db => DB_MEMBERS,
        Candidates::QueryResult => RESULT_MEMBERS,
        Candidates::None => return Vec::new(),
    };

    members
        .iter()
        .filter(|member| matches_prefix(member.name, current_word))
        .map(|member| {
            let mut documentation = member.documentation.to_string();
            if !supports_bind_parameters && member_takes_parameters(member) {
                documentation.push_str("\n\n");
                documentation.push_str(NO_BIND_PARAMETERS_NOTE);
            }

            CompletionItem {
                label: member.name.to_string(),
                kind: Some(member.kind),
                detail: Some(member.detail.to_string()),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: documentation,
                })),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    member.name.to_string(),
                ))),
                insert_text: Some(member.name.to_string()),
                ..Default::default()
            }
        })
        .collect()
}

impl CompletionProvider for ScriptCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let text = rope.slice(0..rope.len()).to_string();
        let (candidates, current_word, start) = analyze(&text, offset);
        let end = start + current_word.len();
        let range = Range::new(rope.offset_to_position(start), rope.offset_to_position(end));

        let items = build_items(
            candidates,
            current_word,
            range,
            self.supports_bind_parameters,
        );
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        // `.` opens the member list; identifier characters narrow it.
        new_text == "." || new_text.chars().all(|c| c.is_alphanumeric() || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn labels(text: &str) -> Vec<String> {
        let (candidates, current_word, _start) = analyze(text, text.len());
        let range = Range::new(Position::new(0, 0), Position::new(0, 0));
        build_items(candidates, current_word, range, true)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn api_reference_covers_every_member() {
        let reference = api_reference(true);

        for member in DB_MEMBERS.iter().chain(RESULT_MEMBERS) {
            assert!(
                reference.contains(member.detail),
                "missing {}: {reference}",
                member.name
            );
        }
        assert!(!reference.contains(NO_BIND_PARAMETERS_NOTE), "{reference}");
    }

    #[test]
    fn api_reference_notes_missing_bind_parameters() {
        assert!(api_reference(false).contains(NO_BIND_PARAMETERS_NOTE));
    }

    #[test]
    fn db_offers_its_methods() {
        let items = labels("db.");
        assert_eq!(items, vec!["query", "execute", "transaction", "display"]);
    }

    #[test]
    fn db_prefix_filters_methods() {
        assert_eq!(labels("db.q"), vec!["query"]);
        assert_eq!(labels("const x = db.di"), vec!["display"]);
    }

    #[test]
    fn query_results_offer_their_fields() {
        let items = labels(r#"db.query("SELECT 1")."#);
        assert!(items.contains(&"rows".to_string()));
        assert!(items.contains(&"columns".to_string()));
        assert!(items.contains(&"rowsAffected".to_string()));
    }

    #[test]
    fn query_results_complete_through_a_stored_call() {
        let items = labels("const result = db.query(sql, [a, b]).ro");
        assert_eq!(items, vec!["rows", "rowsAffected"]);
    }

    #[test]
    fn row_access_offers_nothing() {
        // A row object's keys are the query's columns, which are not knowable
        // without running it.
        assert!(labels(r#"db.query("SELECT 1").rows[0]."#).is_empty());
    }

    #[test]
    fn unrelated_identifiers_are_not_completed() {
        assert!(labels("mydb.").is_empty());
        assert!(labels("other.db2.").is_empty());
        assert!(labels("console.").is_empty());
        assert!(labels("const total = ").is_empty());
    }

    #[test]
    fn db_completes_after_punctuation_and_newlines() {
        assert!(!labels("if (db.").is_empty());
        assert!(!labels("const a = 1;\ndb.").is_empty());
    }

    #[test]
    fn replacement_starts_at_the_partial_identifier() {
        let text = "db.que";
        let (_candidates, current_word, start) = analyze(text, text.len());
        assert_eq!(current_word, "que");
        assert_eq!(start, text.len() - 3);
    }

    #[test]
    fn backends_without_bind_parameters_say_so() {
        let range = Range::new(Position::new(0, 0), Position::new(0, 0));
        let (candidates, current_word, _) = analyze("db.q", 4);
        let items = build_items(candidates, current_word, range, false);
        let documentation = match items[0].documentation.as_ref() {
            Some(Documentation::MarkupContent(markup)) => markup.value.clone(),
            other => panic!("unexpected documentation: {other:?}"),
        };
        assert!(documentation.contains("no bind parameters"));
    }
}
