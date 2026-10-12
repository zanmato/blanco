//! Snippets offered in completion by their trigger word. The snippets panel
//! keeps [`SnippetTriggers`] current, and the SQL and script completion
//! providers add the snippets whose trigger starts with the word being typed.

use std::sync::Arc;

use app_database::{EditorKind, SnippetData};
use gpui::{App, Global};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Documentation, InsertTextFormat,
    MarkupContent, MarkupKind, Range, TextEdit,
};

/// The saved snippets that have a trigger word.
#[derive(Default)]
pub struct SnippetTriggers(Arc<Vec<SnippetData>>);

impl Global for SnippetTriggers {}

impl SnippetTriggers {
    /// Replace the catalog with the triggered snippets among `snippets`.
    pub fn set(snippets: &[SnippetData], cx: &mut App) {
        let triggered = snippets
            .iter()
            .filter(|snippet| !snippet.is_group && snippet.trigger.is_some())
            .cloned()
            .collect();
        cx.set_global(Self(Arc::new(triggered)));
    }

    /// The triggered snippets meant for editors of `kind`.
    pub fn for_kind(kind: EditorKind, cx: &App) -> Vec<SnippetData> {
        cx.try_global::<Self>()
            .map(|triggers| {
                triggers
                    .0
                    .iter()
                    .filter(|snippet| snippet.kind == kind)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Completion items for the snippets whose trigger starts with
/// `current_word`, ignoring case. Nothing is offered for an empty word, so
/// snippets do not crowd every list.
pub fn snippet_items(
    snippets: &[SnippetData],
    current_word: &str,
    range: Range,
) -> Vec<CompletionItem> {
    if current_word.is_empty() {
        return Vec::new();
    }
    let needle = current_word.to_lowercase();
    snippets
        .iter()
        .filter_map(|snippet| {
            let trigger = snippet.trigger.as_deref()?;
            trigger
                .to_lowercase()
                .starts_with(&needle)
                .then(|| CompletionItem {
                    label: trigger.to_string(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    detail: Some(snippet.name.clone()),
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: format!("```\n{}\n```", snippet.content),
                    })),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                        range,
                        snippet.content.clone(),
                    ))),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    filter_text: Some(trigger.to_string()),
                    sort_text: Some(format!("0{}", trigger.to_lowercase())),
                    ..Default::default()
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn snippet(trigger: &str, content: &str) -> SnippetData {
        SnippetData {
            id: Some(1),
            name: format!("{trigger} snippet"),
            content: content.to_string(),
            kind: EditorKind::Query,
            parent_id: None,
            is_group: false,
            position: 0,
            trigger: Some(trigger.to_string()),
        }
    }

    #[test]
    fn test_snippet_items_match_the_trigger_prefix() {
        let snippets = [
            snippet("selc", "SELECT COUNT(*) FROM $1"),
            snippet("upd", "UPDATE $1 SET $0"),
        ];
        let range = Range::new(Position::new(0, 0), Position::new(0, 3));

        let items = snippet_items(&snippets, "SEL", range);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "selc");
        assert_eq!(items[0].detail.as_deref(), Some("selc snippet"));
        assert_eq!(items[0].insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(
            items[0].text_edit,
            Some(CompletionTextEdit::Edit(TextEdit::new(
                range,
                "SELECT COUNT(*) FROM $1".to_string()
            )))
        );

        assert!(snippet_items(&snippets, "", range).is_empty());
        assert!(snippet_items(&snippets, "zz", range).is_empty());
    }
}
