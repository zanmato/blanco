use anyhow::Result;
use gpui::{App, AppContext, Task, Window};
use lsp_types::{Position, Range, SelectionRange};
use ropey::Rope;

use gpui_component::RopeExt;
use gpui_component::input::SelectionRangeProvider;

use sql_parser::statement_parser::extract_statement_info;

/// SQL selection range provider that highlights the current SQL statement at cursor
pub struct SqlSelectionRangeProvider;

impl SqlSelectionRangeProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SqlSelectionRangeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionRangeProvider for SqlSelectionRangeProvider {
    fn selection_ranges(
        &self,
        text: &Rope,
        position: Position,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<SelectionRange>>> {
        // Clone the rope to move it into the async task
        let text = text.clone();

        // Spawn the parsing work on a background thread
        cx.background_spawn(async move {
            // Convert LSP Position to byte offset using RopeExt
            let cursor_byte_offset = text.position_to_offset(&position);
            // Then convert byte offset to character position for extract_statement_info
            let cursor_char_pos = text.byte_to_char_idx(cursor_byte_offset);

            // Use extract_statement_info which internally uses the thread-local parser
            let statement_info = extract_statement_info(&text, cursor_char_pos);

            Ok(statement_info.map(|info| {
                // Convert byte range to LSP Range
                let start_position = text.offset_to_position(info.byte_range.start);
                let end_position = text.offset_to_position(info.byte_range.end);

                SelectionRange {
                    range: Range {
                        start: start_position,
                        end: end_position,
                    },
                    parent: None,
                }
            }))
        })
    }
}
