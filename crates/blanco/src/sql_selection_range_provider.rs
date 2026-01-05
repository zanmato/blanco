use anyhow::Result;
use gpui::Task;
use lsp_types::{Position, Range, SelectionRange};
use ropey::Rope;

use gpui_component::RopeExt;
use gpui_component::input::SelectionRangeProvider;

use crate::sql_statement_parser::extract_statement_info;

/// SQL selection range provider that highlights the current SQL statement at cursor
pub struct SqlSelectionRangeProvider;

impl SqlSelectionRangeProvider {
    /// Create a new SQL selection range provider
    pub fn new() -> Result<Self, String> {
        Ok(Self)
    }
}

impl Default for SqlSelectionRangeProvider {
    fn default() -> Self {
        Self::new().expect("Failed to create SQL selection range provider")
    }
}

impl SelectionRangeProvider for SqlSelectionRangeProvider {
    fn selection_ranges(
        &self,
        text: &Rope,
        position: Position,
    ) -> Task<Result<Option<SelectionRange>>> {
        let text = text.to_string();

        let result = {
            // Convert LSP Position to byte offset using RopeExt
            let rope = Rope::from_str(&text);
            let cursor_byte_pos = rope.position_to_offset(&position);

            // Use extract_statement_info which internally uses the thread-local parser
            let statement_info = extract_statement_info(&text, cursor_byte_pos);

            Ok(statement_info.map(|info| {
                // Convert byte range to LSP Range
                let start_position = rope.offset_to_position(info.byte_range.start);
                let end_position = rope.offset_to_position(info.byte_range.end);

                SelectionRange {
                    range: Range {
                        start: start_position,
                        end: end_position,
                    },
                    parent: None,
                }
            }))
        };

        Task::ready(result)
    }
}
