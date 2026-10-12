//! Action handlers for the `CopyAs*` actions on `ResultsPanel`.

use gpui::{Context, Window};

use crate::app::{
    CopyAsCSV, CopyAsJSON, CopyAsMarkdown, CopyAsSQL, CopyAsTSV, CopyAsVALUES, CopyCellValue,
};

use super::ResultsPanel;

impl ResultsPanel {
    fn copy_selected_as(&mut self, format: &str, window: &mut Window, cx: &mut Context<Self>) {
        let selected_rows = {
            let table_state = self.table_state.read(cx);
            let mut selected_rows = table_state.selected_rows().clone();
            if selected_rows.is_empty() {
                if let Some(cell) = table_state.selected_cell() {
                    selected_rows.insert(cell.0);
                } else {
                    tracing::error!("Failed to copy as {}: No rows selected for copying", format);
                    return;
                }
            }
            selected_rows
        };

        // A copy holds values, not previews of them.
        let format_for_retry = format.to_string();
        if self.load_deferred_values_then(
            selected_rows.iter().copied(),
            None,
            window,
            cx,
            move |panel, window, cx| panel.copy_selected_as(&format_for_retry, window, cx),
        ) {
            return;
        }

        let delegate = self.table_state.read(cx).delegate();
        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);
        self.copy_handler.copy_as_format(&selected_data, format, cx);
    }

    /// Copy one cell's value, fetching it first when the grid holds only a
    /// preview.
    pub(super) fn on_copy_cell_value(
        &mut self,
        action: &CopyCellValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.defer_action(
            [action.row],
            Some(action.col),
            action,
            window,
            cx,
            Self::on_copy_cell_value,
        ) {
            return;
        }
        let value = self
            .table_state
            .read(cx)
            .delegate()
            .stored_cell_value(action.row, action.col);
        if let Some(value) = value {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
        }
    }

    pub(super) fn on_copy_as_csv(
        &mut self,
        _action: &CopyAsCSV,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("csv", window, cx);
    }

    pub(super) fn on_copy_as_tsv(
        &mut self,
        _action: &CopyAsTSV,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("tsv", window, cx);
    }

    pub(super) fn on_copy_as_json(
        &mut self,
        _action: &CopyAsJSON,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("json", window, cx);
    }

    pub(super) fn on_copy_as_sql(
        &mut self,
        _action: &CopyAsSQL,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("sql", window, cx);
    }

    pub(super) fn on_copy_as_values(
        &mut self,
        _action: &CopyAsVALUES,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("values", window, cx);
    }

    pub(super) fn on_copy_as_markdown(
        &mut self,
        _action: &CopyAsMarkdown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("markdown", window, cx);
    }
}
