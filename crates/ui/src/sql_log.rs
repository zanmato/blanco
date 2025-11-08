use gpui::{
    div, px, Context, IntoElement, ParentElement, Render, SharedString, Styled, StyledText, Window,
};
use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_component::ActiveTheme;
use ropey::{LineType, Rope};
use std::sync::Arc;

pub enum SqlLogMessage {
    SqlStatement(String),
    Comment(String),
}

/// SQL Log entity for displaying SQL queries and logs with proper syntax highlighting
pub struct SqlLog {
    text: Rope,
    max_lines: usize,
    highlighter: SyntaxHighlighter,
    theme: Arc<HighlightTheme>,
}

impl SqlLog {
    /// Create a new SQL log with the specified maximum number of lines
    pub fn new(max_lines: usize, theme: Arc<HighlightTheme>) -> Self {
        let highlighter = SyntaxHighlighter::new("sql");

        Self {
            text: Rope::from(""),
            max_lines,
            highlighter,
            theme,
        }
    }

    /// Append text to the log, managing line limits
    pub fn append_text(&mut self, text: &SqlLogMessage, cx: &mut Context<Self>) {
        // Check if this is a comment or a SQL statement, if it doesn't end with a delimiter, add it
        let new_text = match text {
            SqlLogMessage::SqlStatement(statement) => {
                if !statement.trim_start().ends_with(";") {
                    format!("{};\n", statement)
                } else {
                    format!("{}\n", statement)
                }
            }
            SqlLogMessage::Comment(comment) => format!("-- {}\n", comment),
        };

        // Append the new text
        self.text.insert(self.text.len(), &new_text);

        // Check if we need to trim old lines
        self.trim_lines();

        // Update the highlighter with the new text
        self.highlighter.update(None, &self.text);

        // Notify that the view needs to be re-rendered
        cx.notify();
    }

    /// Clear all text from the log
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text = Rope::from("");
        self.highlighter.update(None, &self.text);
        cx.notify();
    }

    /// Set the maximum number of lines and trim if necessary
    pub fn set_max_lines(&mut self, max_lines: usize, cx: &mut Context<Self>) {
        self.max_lines = max_lines;
        self.trim_lines();
        self.highlighter.update(None, &self.text);
        cx.notify();
    }

    /// Get the current number of lines in the log
    pub fn line_count(&self) -> usize {
        self.text.len_lines(LineType::LF)
    }

    /// Get the current text content
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// Trim old lines if we exceed the maximum
    fn trim_lines(&mut self) {
        let line_count = self.text.len_lines(LineType::LF);
        if line_count > self.max_lines {
            let lines_to_remove = line_count - self.max_lines;

            // Calculate the byte offset to remove
            let mut total_bytes = 0;
            for i in 0..lines_to_remove {
                if i < line_count {
                    let line = self.text.line(i, LineType::LF);
                    total_bytes += line.bytes().len() + 1; // +1 for newline
                }
            }

            // Remove the old lines
            self.text.remove(0..total_bytes);
        }
    }
}

impl Render for SqlLog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Get the full text content as a SharedString
        let text_content = self.text.to_string();
        let shared_text = SharedString::from(text_content);

        // Get syntax highlights from the highlighter
        let text_len = shared_text.len();
        let range = std::ops::Range::<usize> {
            start: 0,
            end: text_len,
        };
        let highlights = self.highlighter.styles(&range, &self.theme);

        // Create a container with syntax-highlighted text
        div()
            .w_full()
            .h_full()
            .bg(cx
                .theme()
                .highlight_theme
                .style
                .editor_background
                .unwrap_or(cx.theme().background))
            .p_4()
            .font_family("Fira Code")
            .text_size(px(12.))
            .text_color(cx.theme().foreground)
            .child(StyledText::new(shared_text).with_highlights(highlights))
    }
}
