//! Action handlers for the `CopyAs*` actions on `ResultsPanel`.

use gpui::{Context, Window};

use crate::app::{CopyAsCSV, CopyAsJSON, CopyAsMarkdown, CopyAsSQL, CopyAsTSV, CopyAsVALUES};

use super::ResultsPanel;

impl ResultsPanel {
    fn copy_selected_as(&mut self, format: &str, cx: &mut Context<Self>) {
        let table_state = self.table_state.read(cx);
        let mut selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            if let Some(cell) = table_state.selected_cell() {
                selected_rows.insert(cell.0);
            } else {
                tracing::error!("Failed to copy as {}: No rows selected for copying", format);
                return;
            }
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);
        self.copy_handler.copy_as_format(&selected_data, format, cx);
    }

    pub(super) fn on_copy_as_csv(
        &mut self,
        _action: &CopyAsCSV,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("csv", cx);
    }

    pub(super) fn on_copy_as_tsv(
        &mut self,
        _action: &CopyAsTSV,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("tsv", cx);
    }

    pub(super) fn on_copy_as_json(
        &mut self,
        _action: &CopyAsJSON,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("json", cx);
    }

    pub(super) fn on_copy_as_sql(
        &mut self,
        _action: &CopyAsSQL,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("sql", cx);
    }

    pub(super) fn on_copy_as_values(
        &mut self,
        _action: &CopyAsVALUES,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("values", cx);
    }

    pub(super) fn on_copy_as_markdown(
        &mut self,
        _action: &CopyAsMarkdown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_as("markdown", cx);
    }
}
