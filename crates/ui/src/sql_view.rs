use gpui::{
    ClipboardItem, Context, HighlightStyle, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledText, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme, IconName, Sizable as _};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

pub enum SqlViewMessage {
    SqlStatement(String),
    Comment(String),
}

struct LogEntry {
    text: SharedString,
    highlights: Box<[(Range<usize>, HighlightStyle)]>,
}

/// SQL view entity for displaying SQL statements (queries, DDL, etc.) with proper syntax highlighting
pub struct SqlView {
    entries: Vec<LogEntry>,
    max_entries: usize,
    theme: Arc<HighlightTheme>,
    scroll_handle: ScrollHandle,
    copied: bool,
    show_copy_button: bool,
}

impl SqlView {
    /// Create a new SQL view with the specified maximum number of entries
    pub fn new(max_entries: usize, theme: Arc<HighlightTheme>) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            theme,
            scroll_handle: ScrollHandle::default(),
            copied: false,
            show_copy_button: true,
        }
    }

    /// Control whether the copy-to-clipboard button is rendered.
    pub fn show_copy_button(mut self, show: bool) -> Self {
        self.show_copy_button = show;
        self
    }

    /// Concatenate every entry's text, in order, for copying to the clipboard.
    fn all_text(&self) -> String {
        self.entries
            .iter()
            .map(|entry| entry.text.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn copy_to_clipboard(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.all_text()));
        self.copied = true;
        cx.notify();

        // Revert the button back to the copy icon after a short confirmation.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Append text to the log, managing entry limits
    pub fn append_text(&mut self, text: &SqlViewMessage, cx: &mut Context<Self>) {
        // Check if this is a comment or a SQL statement, if it doesn't end with a delimiter, add it
        let new_text = match text {
            SqlViewMessage::SqlStatement(statement) => {
                if !statement.trim_start().ends_with(";") {
                    format!("{};", statement)
                } else {
                    statement.clone()
                }
            }
            SqlViewMessage::Comment(comment) => format!("-- {}", comment),
        };

        // Create a new highlighter for this entry, parse, and compute styles once.
        // Highlight ranges are cached on the entry so render() doesn't redo this work
        // on every frame.
        let mut highlighter = SyntaxHighlighter::new("sql");
        let rope = ropey::Rope::from(&new_text[..]);
        highlighter.update(None, &rope, None);
        let highlights = highlighter
            .styles(&(0..new_text.len()), &self.theme)
            .into_boxed_slice();

        let entry = LogEntry {
            text: SharedString::from(new_text),
            highlights,
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

impl Render for SqlView {
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
                        div().mb_1().child(
                            StyledText::new(entry.text.clone())
                                .with_highlights(entry.highlights.iter().cloned()),
                        )
                    })),
            )
            .vertical_scrollbar(&self.scroll_handle)
            .when(self.show_copy_button && !self.entries.is_empty(), |this| {
                let copied = self.copied;
                this.child(
                    div().absolute().top_2().right_2().child(
                        Button::new("sql-log-copy")
                            .ghost()
                            .xsmall()
                            .icon(if copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .tooltip(if copied { "Copied" } else { "Copy" })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.copy_to_clipboard(cx);
                            })),
                    ),
                )
            })
    }
}
