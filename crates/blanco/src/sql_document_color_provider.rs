use anyhow::Result;
use gpui::{App, AppContext, Task, Window};
use lsp_types::{Color, ColorInformation, Position, Range};
use ropey::Rope;
use ropey::LineType;

use gpui_component::input::DocumentColorProvider;

/// A document color provider that highlights the current SQL query
pub struct SqlDocumentColorProvider {
    /// Highlight color for the current query
    highlight_color: Color,
}

impl SqlDocumentColorProvider {
    /// Create a new SQL document color provider
    pub fn new() -> Self {
        Self {
            // Use a subtle blue highlight with low alpha
            highlight_color: Color {
                red: 0.3,
                green: 0.6,
                blue: 1.0,
                alpha: 0.1,
            },
        }
    }

    /// Create a new provider with a custom highlight color
    pub fn with_color(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self {
            highlight_color: Color { red, green, blue, alpha },
        }
    }
}

impl Default for SqlDocumentColorProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentColorProvider for SqlDocumentColorProvider {
    fn document_colors(
        &self,
        text: &Rope,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<ColorInformation>>> {
        let text = text.to_string();
        let highlight_color = self.highlight_color;

        // For now, let's just highlight the whole text until we can properly get cursor position
        // TODO: Get cursor position from the editor
        cx.background_spawn(async move {
            let rope = Rope::from_str(&text);

            // Get line count using rope's len_lines method
            let line_count = rope.len_lines(LineType::LF);
            let total_chars = rope.len_chars();

            let color_info = ColorInformation {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: line_count.saturating_sub(1) as u32,
                        character: rope.byte_to_utf16_idx(rope.char_to_byte_idx(total_chars)) as u32,
                    },
                },
                color: highlight_color,
            };

            Ok(vec![color_info])
        })
    }
}