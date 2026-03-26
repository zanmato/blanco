use std::time::Duration;

use gpui::{AppContext, Context, SharedString, Window};
use gpui_component::{RopeExt, WindowExt as _, notification::NotificationType};

use crate::result_ext::ResultExt;
use crate::sql::extract_statement_info;

use super::{EditorPanel, LINT_DEBOUNCE_MS, TabType};

impl EditorPanel {
    /// Trigger a debounced lint of the current query.
    ///
    /// This cancels any pending lint task and schedules a new one after the debounce delay.
    pub(super) fn lint_current_query_debounced(
        &mut self,
        range: lsp_types::Range,
        cx: &mut Context<Self>,
    ) {
        self._lint_debounce_task = cx.spawn(async move |entity_handle, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LINT_DEBOUNCE_MS))
                .await;

            entity_handle
                .update(cx, |this, cx| {
                    this.lint_current_query(range, cx);
                })
                .log_err();
        });
    }

    /// Lint the current query/statement and update editor diagnostics.
    pub(super) fn lint_current_query(&mut self, range: lsp_types::Range, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;

        let Some(TabType::Query(query_tab)) = self.tabs.get(tab_index) else {
            return;
        };

        let editor = query_tab.editor.clone();

        // Get the text for the given range
        let (statement_text, byte_range) = {
            let editor_ref = editor.read(cx);
            let text = editor_ref.text();

            // Convert LSP range to byte offsets
            let start_byte = text.position_to_offset(&range.start);
            let end_byte = text.position_to_offset(&range.end);

            // Extract text from the range
            let extracted = text.slice(start_byte..end_byte.min(text.len())).to_string();

            (extracted, start_byte..end_byte)
        };

        let sqruff_service = if let Some(sqruff_service) = &query_tab.sqruff_service {
            sqruff_service.clone()
        } else {
            tracing::warn!("Sqruff service not available for linting");
            return;
        };

        // Background task: do the heavy linting work
        let lint_task = cx.background_spawn(async move {
            sqruff_service.lint(&statement_text, Some(byte_range.start))
        });

        // Foreground task: wait for background task to complete and update UI
        cx.spawn(async move |entity_handle, async_cx| {
            let diagnostics = match lint_task.await {
                Ok(diags) => diags,
                Err(e) => {
                    tracing::error!("Linting failed: {}", e);
                    return Ok::<(), anyhow::Error>(());
                }
            };

            // Update editor diagnostics
            // The diagnostics from sqruff are relative to the statement text,
            // so we need to adjust them to the full file position
            entity_handle
                .update(async_cx, |editor_panel, cx| {
                    if let Some(TabType::Query(query_tab)) = editor_panel.tabs.get_mut(tab_index) {
                        query_tab.editor.update(cx, |state, cx| {
                            use gpui_component::highlighter::Diagnostic;

                            // Calculate the statement start position
                            let start_char = state.text().byte_to_char_idx(byte_range.start);
                            let start_pos = state.text().offset_to_position(start_char);

                            // Adjust diagnostics by adding the statement start position
                            let adjusted_diagnostics: Vec<Diagnostic> = diagnostics
                                .into_iter()
                                .map(|diag| {
                                    let start = lsp_types::Position::new(
                                        diag.range.start.line + start_pos.line,
                                        if diag.range.start.line == 0 {
                                            diag.range.start.character + start_pos.character
                                        } else {
                                            diag.range.start.character
                                        },
                                    );
                                    let end = lsp_types::Position::new(
                                        diag.range.end.line + start_pos.line,
                                        if diag.range.end.line == 0 {
                                            diag.range.end.character + start_pos.character
                                        } else {
                                            diag.range.end.character
                                        },
                                    );
                                    Diagnostic::new(start..end, diag.message)
                                        .with_severity(diag.severity)
                                })
                                .collect();

                            if let Some(set) = state.diagnostics_mut() {
                                set.clear();
                                set.extend(adjusted_diagnostics);
                            }
                            cx.notify();
                        });
                    }
                })
                .log_err();

            Ok(())
        })
        .detach();
    }

    /// Format the current query/statement.
    pub(super) fn format_current_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;

        let Some(TabType::Query(query_tab)) = self.tabs.get(tab_index) else {
            window.push_notification((NotificationType::Error, "No query tab active"), cx);
            return;
        };

        let editor = query_tab.editor.clone();

        // Get current text and cursor position
        let (text, cursor_pos) = {
            let editor_ref = editor.read(cx);
            (editor_ref.text().clone(), editor_ref.cursor())
        };

        // Extract current statement using existing function
        let Some(statement_info) = extract_statement_info(&text, cursor_pos) else {
            window.push_notification((NotificationType::Info, "No query found at cursor"), cx);
            return;
        };

        let statement_text = statement_info.text.clone();
        let byte_range = statement_info.byte_range.clone();

        // Format in background
        let sqruff_service = if let Some(sqruff_service) = &query_tab.sqruff_service {
            sqruff_service.clone()
        } else {
            tracing::warn!("Sqruff service not available for linting");
            return;
        };

        cx.spawn_in(window, async move |entity_handle, window| {
            let formatted_result = sqruff_service.format(&statement_text);
            let formatted = match formatted_result {
                Ok(f) => f,
                Err(e) => {
                    window
                        .update(|_window, cx| {
                            let error_message = SharedString::from(format!("Format failed: {}", e));
                            _window.push_notification((NotificationType::Error, error_message), cx);
                        })
                        .log_err();
                    return Ok::<(), anyhow::Error>(());
                }
            };

            entity_handle
                .update_in(window, |editor_panel, window, cx| {
                    if let Some(TabType::Query(query_tab)) =
                        editor_panel.tabs.get_mut(editor_panel.active_tab_ix)
                    {
                        query_tab.editor.update(cx, |state, cx| {
                            // Replace the statement range with formatted text
                            // Create an LSP TextEdit for the replacement
                            let start = state.text().byte_to_char_idx(byte_range.start);
                            let end = state.text().byte_to_char_idx(byte_range.end);
                            let start_pos = state.text().offset_to_position(start);
                            let end_pos = state.text().offset_to_position(end);
                            let text_edit = lsp_types::TextEdit {
                                range: lsp_types::Range {
                                    start: start_pos,
                                    end: end_pos,
                                },
                                new_text: formatted,
                            };
                            state.apply_lsp_edits(&vec![text_edit], window, cx);
                        });

                        window
                            .push_notification((NotificationType::Success, "Query formatted"), cx);
                    }
                })
                .log_err();

            Ok(())
        })
        .detach();
    }
}
