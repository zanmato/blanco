//! Completion provider for the Redis console editor.
//!
//! Redis "syntax" is line-oriented: the first token on a line is a command, and
//! for container commands (`CONFIG`, `CLIENT`, ...) the second token is a
//! subcommand. Everything after that is data we cannot meaningfully complete.
//! The command/subcommand catalog is the same one baked into the
//! `tree-sitter-redis` grammar (generated from a live server), so highlighting
//! and completion agree on the command set.

use anyhow::Result;
use gpui::{App, Task, Window};
use gpui_component::input::{CompletionProvider, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range, TextEdit,
};

#[derive(Clone, Default)]
pub struct RedisCompletionProvider;

impl RedisCompletionProvider {
    pub fn new() -> Self {
        Self
    }
}

/// One completable token: the candidates to offer and how to label them.
enum Candidates {
    /// First token on the line: any command name.
    Commands,
    /// Second token after a container command: its subcommands.
    Subcommands {
        container: String,
        subcommands: &'static [&'static str],
    },
    /// Position holds a value/argument we cannot complete.
    None,
}

/// Decide what to complete given the text on the current line up to the cursor.
fn classify(line_before_cursor: &str, current_word_start: usize) -> Candidates {
    let prior = &line_before_cursor[..current_word_start];
    let prior_tokens: Vec<&str> = prior.split_whitespace().collect();
    match prior_tokens.as_slice() {
        [] => Candidates::Commands,
        [command] => match tree_sitter_redis::subcommands_for(command) {
            Some(subcommands) => Candidates::Subcommands {
                container: command.to_ascii_uppercase(),
                subcommands,
            },
            None => Candidates::None,
        },
        _ => Candidates::None,
    }
}

fn matches_prefix(candidate: &str, needle: &str) -> bool {
    needle.is_empty()
        || candidate
            .to_ascii_uppercase()
            .starts_with(&needle.to_ascii_uppercase())
}

fn make_item(
    label: &str,
    kind: CompletionItemKind,
    detail: String,
    range: Range,
) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
            range,
            label.to_string(),
        ))),
        detail: Some(detail),
        insert_text: Some(label.to_string()),
        ..Default::default()
    }
}

fn build_items(candidates: Candidates, current_word: &str, range: Range) -> Vec<CompletionItem> {
    match candidates {
        Candidates::Commands => tree_sitter_redis::commands::COMMANDS
            .iter()
            .filter(|command| matches_prefix(command, current_word))
            .map(|command| {
                make_item(
                    command,
                    CompletionItemKind::KEYWORD,
                    "Redis command".to_string(),
                    range,
                )
            })
            .collect(),
        Candidates::Subcommands {
            container,
            subcommands,
        } => subcommands
            .iter()
            .filter(|subcommand| matches_prefix(subcommand, current_word))
            .map(|subcommand| {
                make_item(
                    subcommand,
                    CompletionItemKind::FUNCTION,
                    format!("{container} subcommand"),
                    range,
                )
            })
            .collect(),
        Candidates::None => Vec::new(),
    }
}

/// Pure core of [`RedisCompletionProvider::completions`]: from the buffer text
/// and cursor byte offset, work out what to complete, the partial word under
/// the cursor, and the byte offset where its replacement should start. Kept
/// rope-free so the routing rules can be unit-tested without a `Window`/`Rope`.
fn analyze(text: &str, offset: usize) -> (Candidates, &str, usize) {
    let offset = offset.min(text.len());
    let left = &text[..offset];

    // Restrict to the command line the cursor sits on.
    let line_start = left.rfind('\n').map_or(0, |index| index + 1);
    let line_before_cursor = &left[line_start..];

    // The partial word under the cursor is the trailing run of non-whitespace
    // characters; everything before it is prior tokens.
    let current_word_start = line_before_cursor
        .rfind([' ', '\t'])
        .map_or(0, |index| index + 1);
    let current_word = &line_before_cursor[current_word_start..];

    let candidates = classify(line_before_cursor, current_word_start);
    (candidates, current_word, offset - current_word.len())
}

impl CompletionProvider for RedisCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let text = rope.slice(0..rope.len()).to_string();
        let (candidates, current_word, start) = analyze(&text, offset);
        let end = start + current_word.len();

        let range = Range::new(rope.offset_to_position(start), rope.offset_to_position(end));

        let items = build_items(candidates, current_word, range);
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        // A space advances to the next token (command -> subcommand); otherwise
        // trigger as the user types a command/subcommand name. Module commands
        // contain `.` (e.g. `BF.ADD`), so allow it through too.
        new_text == " "
            || new_text
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    /// Labels offered for `text` with the cursor at the end.
    fn labels(text: &str) -> Vec<String> {
        let (candidates, current_word, _start) = analyze(text, text.len());
        let range = Range::new(Position::new(0, 0), Position::new(0, 0));
        build_items(candidates, current_word, range)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn first_token_completes_commands() {
        let items = labels("GE");
        assert!(items.contains(&"GET".to_string()));
        assert!(items.contains(&"GETEX".to_string()));
        // Unrelated commands are filtered out by the prefix.
        assert!(!items.contains(&"SET".to_string()));
    }

    #[test]
    fn first_token_is_case_insensitive() {
        assert!(labels("co").contains(&"CONFIG".to_string()));
    }

    #[test]
    fn empty_buffer_offers_all_commands() {
        let items = labels("");
        assert!(items.contains(&"GET".to_string()));
        assert!(items.contains(&"CONFIG".to_string()));
        assert!(items.len() > 100);
    }

    #[test]
    fn container_command_completes_subcommands() {
        let items = labels("CONFIG ");
        assert!(items.contains(&"GET".to_string()));
        assert!(items.contains(&"SET".to_string()));
        assert!(items.contains(&"REWRITE".to_string()));
        // Top-level-only commands are not subcommands of CONFIG.
        assert!(!items.contains(&"APPEND".to_string()));
    }

    #[test]
    fn subcommand_prefix_filters() {
        let items = labels("config ge");
        assert!(items.contains(&"GET".to_string()));
        assert!(!items.contains(&"SET".to_string()));
    }

    #[test]
    fn non_container_command_has_no_argument_completions() {
        assert!(labels("GET ").is_empty());
        assert!(labels("GET foo ").is_empty());
    }

    #[test]
    fn third_token_is_not_completed() {
        assert!(labels("CONFIG GET maxmemory").is_empty());
        assert!(labels("CONFIG GET ").is_empty());
    }

    #[test]
    fn completion_targets_the_cursor_line() {
        // The command on a previous line must not affect the current line.
        let items = labels("GET a\nCO");
        assert!(items.contains(&"CONFIG".to_string()));
        assert!(items.contains(&"COMMAND".to_string()));
        assert!(!items.contains(&"GET".to_string()));
    }

    #[test]
    fn replacement_starts_at_the_current_word() {
        let text = "CONFIG ge";
        let (_candidates, current_word, start) = analyze(text, text.len());
        assert_eq!(current_word, "ge");
        assert_eq!(start, text.len() - 2);
    }
}
