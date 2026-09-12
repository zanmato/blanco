use std::ops::Range;
use std::time::Duration;

use gpui::{AppContext, Context, Entity, SharedString, Window};
use gpui_component::input::{EditorState, RangeDecoration};
use gpui_component::{ActiveTheme as _, RopeExt, WindowExt as _, notification::NotificationType};

use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::settings::EditorSettings;
use crate::settings::FormatterSettings;
use crate::sql::{active_statement_range, extract_statement_info};
use crate::status_bar::ActivityReporter;

use super::{EditorPanel, LINT_DEBOUNCE_MS, TabType};

/// Opacity of the frame drawn around the statement under the cursor. The
/// outline marks what Run would execute, so it has to stay behind the text
/// rather than compete with the syntax colors.
const STATEMENT_OUTLINE_OPACITY: f32 = 0.5;

impl EditorPanel {
    /// Re-frame the statement the cursor sits in, and re-lint when it moved.
    ///
    /// Driven from an observer of the editor, which notifies on both edits and
    /// cursor moves. The editor also notifies for repaints that change
    /// neither, so the buffer is only parsed again when the document version
    /// or the selection has actually changed.
    pub(super) fn refresh_statement_outline(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let Some((tab_index, query_tab)) =
            self.tabs
                .iter_mut()
                .enumerate()
                .find_map(|(tab_index, tab)| match tab {
                    TabType::Query(query_tab) if &query_tab.base.editor == editor => {
                        Some((tab_index, query_tab))
                    }
                    _ => None,
                })
        else {
            return;
        };
        let Some(outline) = query_tab.statement_outline.clone() else {
            return;
        };

        let range = {
            let state = editor.read(cx);
            let selection = state.selected_range();
            let parsed_from = (state.document_version(), selection.clone());
            if query_tab.outlined_statement.parsed_from == Some(parsed_from.clone()) {
                return;
            }
            query_tab.outlined_statement.parsed_from = Some(parsed_from);

            // A selection is the user's own statement of scope; outlining the
            // statement around it would only compete with it.
            selection
                .is_empty()
                .then(|| active_statement_range(state.text(), state.cursor()))
                .flatten()
        };

        // Setting entries the collection already holds neither repaints nor
        // notifies the editor back, so the outline can be re-set from an
        // observer of that same editor.
        let color = cx.theme().primary.opacity(STATEMENT_OUTLINE_OPACITY);
        outline.set(
            range
                .clone()
                .map(|range| vec![RangeDecoration::new(range).with_color(color)])
                .unwrap_or_default(),
            cx,
        );

        if query_tab.outlined_statement.range == range {
            return;
        }
        query_tab.outlined_statement.range = range.clone();

        // The linter works on the active tab, so a background tab's outline
        // must not point it at a range from the wrong buffer.
        if self.linting_enabled
            && tab_index == self.active_tab_ix
            && let Some(range) = range
        {
            self.lint_current_query_debounced(range, cx);
        }
    }

    /// Trigger a debounced lint of the current query.
    ///
    /// This cancels any pending lint task and schedules a new one after the debounce delay.
    pub(super) fn lint_current_query_debounced(
        &mut self,
        range: Range<usize>,
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

    /// Lint the statement in `byte_range` and update editor diagnostics.
    pub(super) fn lint_current_query(&mut self, byte_range: Range<usize>, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;

        let Some(TabType::Query(query_tab)) = self.tabs.get(tab_index) else {
            return;
        };

        // Linting is SQL-only (sqruff); non-SQL backends have no linter.
        if !query_tab.context.db_type.supports_sql() {
            return;
        }

        let editor = query_tab.editor.clone();

        // The range was parsed from an earlier revision of the buffer, so clamp
        // it before slicing.
        let (statement_text, byte_range) = {
            let text = editor.read(cx).text();
            let byte_range = byte_range.start.min(text.len())..byte_range.end.min(text.len());

            (text.slice(byte_range.clone()).to_string(), byte_range)
        };

        let sqruff_service = if let Some(sqruff_service) = &query_tab.sqruff_service {
            sqruff_service.clone()
        } else {
            tracing::warn!("Sqruff service not available for linting");
            return;
        };

        let formatter_settings = AppSettings::global(cx).settings.formatter.clone();
        let editor_settings = AppSettings::global(cx).settings.editor.clone();

        let activity = ActivityReporter::global(cx).begin("linting");

        // Background task: do the heavy linting work
        let lint_task = cx.background_spawn(async move {
            sqruff_service.lint(
                &statement_text,
                &formatter_settings,
                &editor_settings,
                Some(byte_range.start),
            )
        });

        // Foreground task: wait for background task to complete and update UI
        cx.spawn(async move |entity_handle, async_cx| {
            let _activity = activity;
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

        // Formatting is SQL-only (sqruff); non-SQL backends have no formatter.
        if !query_tab.context.db_type.supports_sql() {
            return;
        }

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
        let byte_range = statement_info.byte_range;

        // Format in background
        let sqruff_service = if let Some(sqruff_service) = &query_tab.sqruff_service {
            sqruff_service.clone()
        } else {
            tracing::warn!("Sqruff service not available for linting");
            return;
        };

        let formatter_settings: FormatterSettings =
            AppSettings::global(cx).settings.formatter.clone();
        let editor_settings: EditorSettings = AppSettings::global(cx).settings.editor.clone();

        cx.spawn_in(window, async move |entity_handle, window| {
            let formatted_result =
                sqruff_service.format(&statement_text, &formatter_settings, &editor_settings);
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
