use std::ops::Range;
use std::time::Duration;

use gpui::{AppContext, Context, Entity, SharedString, Window};
use gpui_component::input::{EditorState, RangeDecoration, RangeDecorationStyle};
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

/// Documents above this size are parsed for the outline once typing pauses
/// rather than on every keystroke.
const OUTLINE_DEBOUNCED_BYTES: usize = 256 * 1024;
const OUTLINE_DEBOUNCE: Duration = Duration::from_millis(150);

impl EditorPanel {
    /// Re-frame the statement the cursor sits in, and re-lint when it moved.
    ///
    /// Driven from an observer of the editor, which notifies on both edits and
    /// cursor moves. The editor also notifies for repaints that change
    /// neither, so the buffer is only parsed again when the document version
    /// or the selection has actually changed.
    ///
    /// Finding the statement parses the whole buffer, which takes seconds for
    /// an INSERT of many megabytes, so it runs on a background thread, after
    /// typing pauses in a large document. The frame moves with the edits
    /// until the new one is in.
    pub(super) fn refresh_statement_outline(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let Some(query_tab) = self.tabs.iter_mut().find_map(|tab| match tab {
            TabType::Query(query_tab) if &query_tab.base.editor == editor => Some(query_tab),
            _ => None,
        }) else {
            return;
        };
        if query_tab.statement_outline.is_none() {
            return;
        }

        let state = editor.read(cx);
        let selection = state.selected_range();
        let parsed_from = (state.document_version(), selection.clone());
        if query_tab.outlined_statement.parsed_from == Some(parsed_from.clone()) {
            return;
        }
        query_tab.outlined_statement.parsed_from = Some(parsed_from.clone());

        // A selection is the user's own statement of scope; outlining the
        // statement around it would only compete with it.
        if !selection.is_empty() {
            query_tab.outlined_statement.parse = None;
            self.finish_statement_outline(editor, parsed_from, None, cx);
            return;
        }

        let grammar = query_tab.context.db_type.sql_grammar();
        let text = state.text().clone();
        let cursor = state.cursor();
        let debounce = text.len() > OUTLINE_DEBOUNCED_BYTES;
        let editor = editor.clone();
        query_tab.outlined_statement.parse = Some(cx.spawn(async move |panel, cx| {
            if debounce {
                cx.background_executor().timer(OUTLINE_DEBOUNCE).await;
            }
            let range = cx
                .background_spawn(async move { active_statement_range(grammar, &text, cursor) })
                .await;
            panel
                .update(cx, |panel, cx| {
                    panel.finish_statement_outline(&editor, parsed_from, range, cx)
                })
                .log_err();
        }));
    }

    /// Frame `range`, found for the document version and selection in
    /// `parsed_from`, unless the editor has moved on since.
    fn finish_statement_outline(
        &mut self,
        editor: &Entity<EditorState>,
        parsed_from: (u64, Range<usize>),
        range: Option<Range<usize>>,
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
        if query_tab.outlined_statement.parsed_from.as_ref() != Some(&parsed_from) {
            return;
        }
        let Some(outline) = query_tab.statement_outline.clone() else {
            return;
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
                            let start_pos = state.text().offset_to_position(byte_range.start);

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
    pub(crate) fn format_current_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let grammar = query_tab.context.db_type.sql_grammar();
        let Some(statement_info) = extract_statement_info(grammar, &text, cursor_pos) else {
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
                            // Replace the statement with its formatted text.
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

/// The case the keyword commands change keywords to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeywordCase {
    Upper,
    Lower,
}

/// One edit that puts the keywords at `ranges` of `text` in `case`, spanning
/// from the first keyword it changes to the last, so it is one undo step.
/// `None` when every keyword already has that case. Keywords are ASCII, so
/// the edit keeps every offset of the text.
pub(crate) fn keyword_case_edit(
    text: &str,
    ranges: &[Range<usize>],
    case: KeywordCase,
) -> Option<(Range<usize>, String)> {
    let changed: Vec<(Range<usize>, String)> = ranges
        .iter()
        .filter_map(|range| {
            let word = text.get(range.clone())?;
            let cased = match case {
                KeywordCase::Upper => word.to_ascii_uppercase(),
                KeywordCase::Lower => word.to_ascii_lowercase(),
            };
            (cased != word).then(|| (range.clone(), cased))
        })
        .collect();
    let start = changed.first()?.0.start;
    let end = changed.last()?.0.end;
    let mut replacement = String::with_capacity(end - start);
    let mut copied_to = start;
    for (range, cased) in changed {
        replacement.push_str(text.get(copied_to..range.start)?);
        replacement.push_str(&cased);
        copied_to = range.end;
    }
    Some((start..end, replacement))
}

impl EditorPanel {
    /// Change the case of the keywords in the selection, or in the statement
    /// at the cursor when nothing is selected.
    pub(crate) fn change_keyword_case(
        &mut self,
        case: KeywordCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self
            .tabs
            .get(self.active_tab_ix)
            .and_then(TabType::connection_tab)
        else {
            return;
        };
        if !tab.context.db_type.supports_sql() {
            return;
        }
        let grammar = tab.context.db_type.sql_grammar();
        let editor = tab.editor.clone();
        let (text, selection) = {
            let state = editor.read(cx);
            (state.text().clone(), state.selected_range())
        };
        let scope = if selection.is_empty() {
            match active_statement_range(grammar, &text, selection.start) {
                Some(range) => range,
                None => {
                    window.push_notification(
                        (NotificationType::Info, "No query found at cursor"),
                        cx,
                    );
                    return;
                }
            }
        } else {
            selection.clone()
        };
        let source = text.to_string();
        let Some(ranges) = sql_parser::keyword_ranges(grammar, &source, scope) else {
            return;
        };
        let Some((range, replacement)) = keyword_case_edit(&source, &ranges, case) else {
            return;
        };
        editor.update(cx, |state, cx| {
            let edit = lsp_types::TextEdit {
                range: lsp_types::Range {
                    start: state.text().offset_to_position(range.start),
                    end: state.text().offset_to_position(range.end),
                },
                new_text: replacement,
            };
            state.apply_lsp_edits(&vec![edit], window, cx);
            // The edit keeps every offset, so the selection goes back where
            // it was rather than ending after the replaced text.
            state.set_selected_range(selection, window, cx);
        });
    }
}

#[cfg(test)]
mod format_tests {
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_component::input::Undo;

    use super::{KeywordCase, keyword_case_edit};
    use crate::app::{LowerCaseKeywords, UpperCaseKeywords};
    use crate::test_harness::{TestHarness, set_editor_text};

    /// Format replaces exactly the statement at the cursor when multibyte
    /// characters come before it, where byte and character offsets differ.
    #[gpui::test]
    async fn format_after_multibyte_text_replaces_the_right_range(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let sql = "select 'åäö';\nselect   a from t;";
        set_editor_text(&harness, sql, &mut cx);
        let editor = harness.editor_panel.read_with(&cx, |panel, _| {
            panel
                .active_query_tab()
                .expect("query tab")
                .base
                .editor
                .clone()
        });
        let second = sql.find("select   a").expect("second statement");
        editor.update_in(&mut cx, |editor, window, cx| {
            editor.set_selected_range(second + 3..second + 3, window, cx);
        });
        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.format_current_query(window, cx);
            });
        cx.run_until_parked();

        let text = editor.read_with(&cx, |editor, _| editor.text().to_string());
        assert!(
            text.starts_with("select 'åäö';\n"),
            "the first statement is untouched: {text:?}"
        );
        assert!(
            !text.contains("select   a") && text.trim_end().ends_with("from t;"),
            "the second statement is formatted in place: {text:?}"
        );
    }

    fn editor_of(
        harness: &TestHarness,
        cx: &VisualTestContext,
    ) -> gpui::Entity<gpui_component::input::EditorState> {
        harness.editor_panel.read_with(cx, |panel, _| {
            panel
                .active_query_tab()
                .expect("query tab")
                .base
                .editor
                .clone()
        })
    }

    /// The commands change the keywords of the statement at the cursor, keep
    /// the cursor where it was and undo in one step.
    #[gpui::test]
    async fn keyword_case_changes_the_statement_at_the_cursor(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let sql = "select 'select' from t;\nselect count(*) as Total, \"From\" from t where a is not null -- select\n;";
        set_editor_text(&harness, sql, &mut cx);
        let editor = editor_of(&harness, &cx);
        let cursor = sql.find("Total").expect("second statement");
        editor.update_in(&mut cx, |editor, window, cx| {
            editor.focus(window, cx);
            editor.set_selected_range(cursor..cursor, window, cx);
        });

        cx.dispatch_action(UpperCaseKeywords);
        cx.run_until_parked();
        let upper = "select 'select' from t;\nSELECT count(*) AS Total, \"From\" FROM t WHERE a IS NOT NULL -- select\n;";
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.text().to_string(), upper);
            assert_eq!(editor.selected_range(), cursor..cursor);
        });

        cx.dispatch_action(LowerCaseKeywords);
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| assert_eq!(editor.text().to_string(), sql));

        cx.dispatch_action(Undo);
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.text().to_string(), upper)
        });
        cx.dispatch_action(Undo);
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| assert_eq!(editor.text().to_string(), sql));
    }

    /// A selection limits the change to the keywords inside it, across
    /// statements, and stays selected.
    #[gpui::test]
    async fn keyword_case_changes_only_the_selection(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let sql = "select a from t; select b from u; select c from v;";
        set_editor_text(&harness, sql, &mut cx);
        let editor = editor_of(&harness, &cx);
        let selection = sql.find("from t").expect("from")..sql.find(" from u").expect("from");
        editor.update_in(&mut cx, |editor, window, cx| {
            editor.focus(window, cx);
            editor.set_selected_range(selection.clone(), window, cx);
        });
        cx.dispatch_action(UpperCaseKeywords);
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert_eq!(
                editor.text().to_string(),
                "select a FROM t; SELECT b from u; select c from v;"
            );
            assert_eq!(editor.selected_range(), selection);
        });
    }

    /// Redis has no SQL keywords, so its tabs are left alone.
    #[gpui::test]
    async fn keyword_case_leaves_redis_tabs_alone(cx: &mut TestAppContext) {
        let harness = TestHarness::new_with_db_type(cx, database::DatabaseType::Redis);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        set_editor_text(&harness, "get select", &mut cx);
        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                assert!(!panel.active_tab_edits_sql());
                panel.change_keyword_case(KeywordCase::Upper, window, cx);
            });
        cx.run_until_parked();
        let editor = editor_of(&harness, &cx);
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.text().to_string(), "get select")
        });
    }

    #[test]
    fn keyword_case_edit_spans_only_the_changed_keywords() {
        let text = "SELECT a from t WHERE b";
        let ranges = [0..6, 9..13, 16..21];
        assert_eq!(
            keyword_case_edit(text, &ranges, KeywordCase::Upper),
            Some((9..13, "FROM".to_string()))
        );
        assert_eq!(
            keyword_case_edit(text, &ranges, KeywordCase::Lower),
            Some((0..21, "select a from t where".to_string()))
        );
        assert_eq!(
            keyword_case_edit("SELECT A", &[0..6], KeywordCase::Upper),
            None
        );
    }
}
