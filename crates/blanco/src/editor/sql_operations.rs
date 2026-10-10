use std::ops::Range;
use std::time::Duration;

use gpui::{AppContext, Context, Entity, SharedString, Window};
use gpui_component::input::{EditorState, RangeDecoration, RangeDecorationStyle, Rope};
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
                .map(|range| {
                    vec![
                        RangeDecoration::new(range)
                            .with_style(RangeDecorationStyle::Block)
                            .with_color(color),
                    ]
                })
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
    /// Format the selected statements, or with no selection the statement
    /// at the cursor.
    pub(super) fn format_current_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.format_sql(true, window, cx);
    }

    /// Format the statements the selection touches when `use_selection` and
    /// there is one, otherwise the statement at the cursor.
    pub(super) fn format_sql(
        &mut self,
        use_selection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let (text, cursor_pos, selection) = {
            let editor_ref = editor.read(cx);
            (
                editor_ref.text().clone(),
                editor_ref.cursor(),
                editor_ref.selected_range(),
            )
        };
        let selection = use_selection.then_some(selection);
        let Some(byte_range) = format_range(&text, cursor_pos, selection) else {
            window.push_notification((NotificationType::Info, "No query found at cursor"), cx);
            return;
        };
        let statement_text = text.slice(byte_range.clone()).to_string();

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
                            // `offset_to_position` takes byte offsets.
                            let text_edit = lsp_types::TextEdit {
                                range: lsp_types::Range {
                                    start: state.text().offset_to_position(byte_range.start),
                                    end: state.text().offset_to_position(byte_range.end),
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

/// The byte range to format: with a non-empty `selection`, from the start of
/// the statement it begins in to the end of the one it ends in, so a partly
/// selected statement is formatted whole; otherwise the statement at `cursor`.
fn format_range(
    text: &Rope,
    cursor: usize,
    selection: Option<Range<usize>>,
) -> Option<Range<usize>> {
    let statement_at = |offset: usize| extract_statement_info(text, offset).map(|s| s.byte_range);
    match selection.filter(|selection| !selection.is_empty()) {
        Some(selection) => {
            let start = statement_at(selection.start)
                .map_or(selection.start, |range| range.start.min(selection.start));
            let end = statement_at(selection.end.saturating_sub(1))
                .map_or(selection.end, |range| range.end.max(selection.end));
            Some(start..end)
        }
        None => statement_at(cursor),
    }
}

#[cfg(test)]
mod format_range_tests {
    use super::*;

    #[test]
    fn selections_grow_to_whole_statements() {
        let sql = "select 1;\nselect a from t where b = 2;\nselect 3;";
        let rope = Rope::from_str(sql);
        let second = sql.find("select a").unwrap();
        let third = sql.find("select 3").unwrap();

        // No selection: the statement at the cursor.
        let range = format_range(&rope, second + 3, None).unwrap();
        assert!(sql[range].starts_with("select a from t"));

        // Part of the second statement selected: the whole second statement.
        let range = format_range(&rope, 0, Some(second + 7..second + 12)).unwrap();
        let formatted = &sql[range];
        assert!(formatted.starts_with("select a") && formatted.contains("b = 2"));
        assert!(!formatted.contains("select 1") && !formatted.contains("select 3"));

        // A selection across two statements covers both, but not the first.
        let range = format_range(&rope, 0, Some(second + 2..third + 4)).unwrap();
        let formatted = &sql[range];
        assert!(formatted.starts_with("select a") && formatted.contains("select 3"));
        assert!(!formatted.contains("select 1"));
    }
}

#[cfg(test)]
mod format_tests {
    use gpui::{TestAppContext, VisualTestContext};

    use crate::test_harness::{TestHarness, set_editor_text};

    /// Format with part of one statement selected rewrites that statement and
    /// leaves the others as they were.
    #[gpui::test]
    async fn format_with_a_selection_leaves_other_statements(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let sql = "select   1;\nselect   a from t;";
        set_editor_text(&harness, sql, &mut cx);
        let editor = harness.editor_panel.read_with(&cx, |panel, _| {
            panel
                .active_query_tab()
                .expect("query tab")
                .base
                .editor
                .clone()
        });
        let second = sql.find("select   a").unwrap();
        editor.update_in(&mut cx, |editor, window, cx| {
            editor.set_selected_range(second + 9..second + 10, window, cx);
        });
        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.format_current_query(window, cx);
            });
        cx.run_until_parked();

        let text = editor.read_with(&cx, |editor, _| editor.text().to_string());
        assert!(
            text.starts_with("select   1;"),
            "first statement kept: {text:?}"
        );
        assert!(
            !text.contains("select   a"),
            "second statement formatted: {text:?}"
        );
    }
}
