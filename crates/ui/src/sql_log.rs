use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Window, div, px,
};
use gpui_component::ActiveTheme;
use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_component::scroll::ScrollableElement as _;
use std::sync::Arc;

pub enum SqlLogMessage {
    SqlStatement(String),
    Comment(String),
}

struct LogEntry {
    text: SharedString,
    highlighter: SyntaxHighlighter,
}

/// SQL Log entity for displaying SQL queries and logs with proper syntax highlighting
pub struct SqlLog {
    entries: Vec<LogEntry>,
    max_entries: usize,
    theme: Arc<HighlightTheme>,
    scroll_handle: ScrollHandle,
}

impl SqlLog {
    /// Create a new SQL log with the specified maximum number of entries
    pub fn new(max_entries: usize, theme: Arc<HighlightTheme>) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            theme,
            scroll_handle: ScrollHandle::default(),
        }
    }

    /// Append text to the log, managing entry limits
    pub fn append_text(&mut self, text: &SqlLogMessage, cx: &mut Context<Self>) {
        // Check if this is a comment or a SQL statement, if it doesn't end with a delimiter, add it
        let new_text = match text {
            SqlLogMessage::SqlStatement(statement) => {
                if !statement.trim_start().ends_with(";") {
                    format!("{};", statement)
                } else {
                    statement.clone()
                }
            }
            SqlLogMessage::Comment(comment) => format!("-- {}", comment),
        };

        // Create a new highlighter for this entry
        let mut highlighter = SyntaxHighlighter::new("sql");

        // Create a Rope for the highlighter to parse
        let rope = ropey::Rope::from(&new_text[..]);
        highlighter.update(None, &rope, None);

        // Create the log entry
        let entry = LogEntry {
            text: SharedString::from(new_text),
            highlighter,
        };

        // Append the new entry
        self.entries.push(entry);

        // Check if we need to trim old entries
        self.trim_entries();

        // Set scroll-to-bottom flag. This is consumed during the next prepaint
        // triggered by cx.notify(), so no delayed task is needed.
        self.scroll_handle.scroll_to_bottom();

        cx.notify();
    }

    /// Clear all entries from the log
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.entries.clear();
        cx.notify();
    }

    /// Set the maximum number of entries and trim if necessary
    pub fn set_max_entries(&mut self, max_entries: usize, cx: &mut Context<Self>) {
        self.max_entries = max_entries;
        self.trim_entries();
        cx.notify();
    }

    /// Get the current number of entries in the log
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Trim old entries if we exceed the maximum
    fn trim_entries(&mut self) {
        if self.entries.len() > self.max_entries {
            let entries_to_remove = self.entries.len() - self.max_entries;
            self.entries.drain(0..entries_to_remove);
        }
    }
}

impl Render for SqlLog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(cx
                .theme()
                .highlight_theme
                .style
                .editor_background
                .unwrap_or(cx.theme().background))
            .child(
                div()
                    .id("sql-log")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .p_4()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .text_color(cx.theme().foreground)
                    .children(self.entries.iter().map(|entry| {
                        let text_len = entry.text.len();
                        let range = std::ops::Range::<usize> {
                            start: 0,
                            end: text_len,
                        };
                        let highlights = entry.highlighter.styles(&range, &self.theme);
                        div()
                            .mb_1()
                            .child(
                                StyledText::new(entry.text.clone()).with_highlights(highlights),
                            )
                    })),
            )
            .vertical_scrollbar(&self.scroll_handle)
    }
}
