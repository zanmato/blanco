//! Running JavaScript script tabs and reporting their output back to the UI.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use blanco_core::QueryResult;
use blanco_ui::SqlViewMessage;
use gpui::{AppContext as _, Context, SharedString, Window};
use gpui_component::{RopeExt, WindowExt as _, notification::NotificationType};
use tracing::{debug, error};

use crate::result_ext::ResultExt;
use crate::status_bar::{ActivityReporter, ActivityResult};
use crate::time_format;
use app_database::{AppDatabase, QueryTabData};
use database::{DatabaseService, DatabaseServiceTrait};
use scripting::{ConsoleLevel, ScriptEvent};

use super::{EditorPanel, LINT_DEBOUNCE_MS, TabType};

impl EditorPanel {
    /// Run the active script tab: the selection when there is one, otherwise
    /// the whole buffer.
    pub(super) fn execute_script_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;
        let run_timestamp = chrono::Utc::now().timestamp();
        if let Some(TabType::Script(script_tab)) = self.tabs.get_mut(tab_index) {
            script_tab.last_run_at = Some(run_timestamp);
        }
        let Some(TabType::Script(script_tab)) = self.tabs.get(tab_index) else {
            return;
        };

        let editor = script_tab.editor.read(cx);
        let full_text = editor.text().to_string();
        let selected_text = editor.selected_text().to_string();
        let source = if selected_text.trim().is_empty() {
            full_text.clone()
        } else {
            selected_text
        };

        if source.trim().is_empty() {
            window.push_notification((NotificationType::Error, "No script to run"), cx);
            return;
        }

        // A script owns an OS thread and can't be cancelled by dropping a task,
        // so refuse to start a second one for the same tab.
        if self.loading {
            window.push_notification(
                (NotificationType::Warning, "A script is already running"),
                cx,
            );
            return;
        }

        let connection_id = script_tab.connection_id;
        let database_name = script_tab.database_name.clone();
        let results_panel = script_tab.results_panel.clone();
        let log_view = script_tab.log_view.clone();
        let editor_entity = script_tab.editor.clone();

        let tab_data = QueryTabData {
            id: script_tab.db_id,
            title: script_tab.title.clone(),
            content: full_text,
            position: tab_index as i32,
            connection_id: Some(connection_id),
            connection_type: None,
            connection_name: script_tab.connection_name.clone(),
            database_name: Some(database_name.clone()),
            schema_name: script_tab.schema_name.clone(),
            environment_type: script_tab.environment_type,
            tab_kind: app_database::EditorKind::Script,
            last_run_at: Some(run_timestamp),
        };

        let app_database = AppDatabase::global(cx).clone();
        cx.spawn(async move |entity_handle, cx| {
            let tab_db_id = match app_database.save_query_tab(&tab_data).await {
                Ok(db_id) => {
                    debug!("Script tab saved successfully with db_id: {}", db_id);
                    db_id
                }
                Err(e) => {
                    error!("Failed to save script tab: {}", e);
                    return;
                }
            };

            app_database
                .touch_query_tab_last_run(tab_db_id, run_timestamp)
                .await
                .map_err(anyhow::Error::from)
                .log_err();

            entity_handle
                .update(cx, |editor_panel: &mut EditorPanel, _| {
                    if let Some(TabType::Script(script_tab)) = editor_panel.tabs.get_mut(tab_index)
                    {
                        script_tab.db_id = Some(tab_db_id);
                    }
                })
                .log_err();
        })
        .detach();

        let activity_label = script_tab
            .connection_name
            .clone()
            .unwrap_or_else(|| database_name.clone());
        let activity =
            ActivityReporter::global(cx).begin(format!("{activity_label}: running script"));

        self.loading = true;
        cx.notify();

        log_view.update(cx, |log, cx| {
            log.append_text(&SqlViewMessage::Comment("running script…".to_string()), cx);
        });

        let db_service: Arc<dyn DatabaseServiceTrait> =
            Arc::new(DatabaseService::global(cx).clone());
        // Scripts have no per-query timeout: the whole point is long,
        // sequential work. The optional wall-clock budget and Stop are the ways
        // out.
        let script_timeout_seconds = crate::app_settings::AppSettings::global(cx)
            .settings
            .database
            .script_timeout_seconds;
        let timeout = (script_timeout_seconds > 0)
            .then(|| std::time::Duration::from_secs(u64::from(script_timeout_seconds)));
        let job = scripting::spawn_script(
            source,
            db_service,
            connection_id,
            database_name,
            gpui_tokio::Tokio::handle(cx),
            timeout,
        );
        self.script_cancel = Some(job.cancel.clone());

        let events = job.events;
        let start_time = std::time::Instant::now();

        self._run_query_task = cx.spawn_in(window, async move |editor_panel_entity, window| {
            let activity = activity;
            let mut displayed: Vec<QueryResult> = Vec::new();
            let mut outcome: Result<(), String> = Ok(());

            while let Ok(event) = events.recv().await {
                match event {
                    ScriptEvent::Console { level, text } => {
                        let line = match level {
                            ConsoleLevel::Log => text,
                            ConsoleLevel::Warn => format!("[warn] {text}"),
                            ConsoleLevel::Error => format!("[error] {text}"),
                        };
                        window
                            .update(|_window, cx| {
                                log_view.update(cx, |log, cx| {
                                    log.append_text(&SqlViewMessage::Comment(line), cx);
                                });
                            })
                            .log_err();
                    }
                    ScriptEvent::Display(result) => {
                        let mut result = *result;
                        result.connection_id = Some(connection_id);
                        displayed.push(result);
                        let results = displayed.clone();
                        window
                            .update(|window, cx| {
                                results_panel.update(cx, |panel, cx| {
                                    panel.set_query_results(
                                        results,
                                        Some(connection_id),
                                        window,
                                        cx,
                                    );
                                });
                            })
                            .log_err();
                    }
                    ScriptEvent::Finished(result) => {
                        outcome = result;
                        break;
                    }
                }
            }

            let duration_us = start_time.elapsed().as_micros() as i64;
            let elapsed = time_format::format_duration(duration_us);

            match &outcome {
                Ok(()) => {
                    activity.finish(ActivityResult::Ok(format!("Script OK · {elapsed}").into()))
                }
                Err(message) if message == scripting::CANCELLED_MESSAGE => {
                    activity.finish(ActivityResult::Err("script cancelled".into()))
                }
                Err(message) if message.starts_with(scripting::TIMED_OUT_PREFIX) => {
                    activity.finish(ActivityResult::Err("script timed out".into()))
                }
                Err(_) => activity.finish(ActivityResult::Err("script failed".into())),
            }

            window
                .update(|window, cx| {
                    let summary = match &outcome {
                        Ok(()) => format!(
                            "{}, script finished in {}",
                            time_format::format_current_timestamp(),
                            elapsed
                        ),
                        Err(message) => format!("script failed: {message}"),
                    };
                    log_view.update(cx, |log, cx| {
                        log.append_text(&SqlViewMessage::Comment(summary), cx);
                    });

                    if let Err(message) = &outcome
                        && message != scripting::CANCELLED_MESSAGE
                    {
                        window.push_notification(
                            (NotificationType::Error, SharedString::from(message.clone())),
                            cx,
                        );
                    }

                    // A compile error never reaches the tree-sitter linter's
                    // notion of "valid enough"; surface the engine's own line
                    // number in the gutter too.
                    let engine_diagnostic = match &outcome {
                        Err(message) if message != scripting::CANCELLED_MESSAGE => {
                            scripting::error_source_line(message)
                                .map(|line| (line, first_line(message)))
                        }
                        _ => None,
                    };
                    editor_entity.update(cx, |state, cx| {
                        set_engine_diagnostic(state, engine_diagnostic, cx);
                    });

                    editor_panel_entity
                        .update(cx, |editor_panel, cx| {
                            editor_panel.loading = false;
                            editor_panel.script_cancel = None;
                            cx.notify();
                        })
                        .ok();
                })
                .log_err();
        });
    }

    /// Ask the running script to stop. Unlike a query, the foreground drain
    /// task must keep running: it is what observes the script's terminal event
    /// and clears the loading state.
    pub fn cancel_running_script(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.script_cancel {
            cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    /// Debounced syntax check of the active script tab.
    pub(super) fn lint_current_script_debounced(&mut self, cx: &mut Context<Self>) {
        self._lint_debounce_task = cx.spawn(async move |entity_handle, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LINT_DEBOUNCE_MS))
                .await;

            entity_handle
                .update(cx, |this, cx| {
                    this.lint_current_script(cx);
                })
                .log_err();
        });
    }

    /// Parse the whole script buffer with tree-sitter and mark `ERROR`/missing
    /// nodes as diagnostics. Script buffers are small, so a full reparse per
    /// debounce is cheaper than maintaining an incremental tree.
    pub(super) fn lint_current_script(&mut self, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;
        let Some(TabType::Script(script_tab)) = self.tabs.get(tab_index) else {
            return;
        };

        let editor = script_tab.editor.clone();
        let source = editor.read(cx).text().to_string();

        let parse_task = cx.background_spawn(async move { syntax_errors(&source) });

        cx.spawn(async move |entity_handle, async_cx| {
            let errors = parse_task.await;

            entity_handle
                .update(async_cx, |editor_panel, cx| {
                    if let Some(TabType::Script(script_tab)) = editor_panel.tabs.get_mut(tab_index)
                    {
                        script_tab.editor.update(cx, |state, cx| {
                            use gpui_component::highlighter::Diagnostic;

                            let text = state.text().clone();
                            let diagnostics: Vec<Diagnostic> = errors
                                .iter()
                                .map(|error| {
                                    let start =
                                        text.offset_to_position(text.byte_to_char_idx(error.start));
                                    let end =
                                        text.offset_to_position(text.byte_to_char_idx(error.end));
                                    Diagnostic::new(start..end, error.message.clone())
                                        .with_severity(lsp_types::DiagnosticSeverity::ERROR)
                                })
                                .collect();

                            if let Some(set) = state.diagnostics_mut() {
                                set.clear();
                                set.extend(diagnostics);
                            }
                            cx.notify();
                        });
                    }
                })
                .log_err();
        })
        .detach();
    }
}

/// A syntax error located in the script buffer, in byte offsets.
struct SyntaxError {
    start: usize,
    end: usize,
    message: String,
}

/// Walk the JavaScript parse tree collecting `ERROR` and missing nodes. Only
/// the outermost error of a nested cascade is reported, so one typo doesn't
/// paint the rest of the file red.
fn syntax_errors(source: &str) -> Vec<SyntaxError> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .is_err()
    {
        tracing::error!("failed to load the JavaScript grammar for script linting");
        return Vec::new();
    }

    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    if !tree.root_node().has_error() {
        return Vec::new();
    }

    let mut errors = Vec::new();
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            let message = if node.is_missing() {
                format!("missing {}", node.kind())
            } else {
                "syntax error".to_string()
            };
            // An empty ERROR node (a missing token at EOF) would render as a
            // zero-width squiggle; widen it to at least one character.
            let start = node.start_byte();
            let end = node.end_byte().max(start + 1).min(source.len());
            errors.push(SyntaxError {
                start,
                end,
                message,
            });
            continue;
        }
        if node.has_error() {
            stack.extend(node.children(&mut cursor));
        }
    }

    errors.sort_by_key(|error| error.start);
    errors
}

/// Replace the editor's diagnostics with the engine-reported error, if any.
/// Called on every run so a fixed script clears its previous squiggle.
fn set_engine_diagnostic(
    state: &mut gpui_component::input::EditorState,
    diagnostic: Option<(u32, String)>,
    cx: &mut Context<gpui_component::input::EditorState>,
) {
    use gpui_component::highlighter::Diagnostic;

    let Some(set) = state.diagnostics_mut() else {
        return;
    };
    set.clear();

    if let Some((line, message)) = diagnostic {
        // QuickJS reports lines 1-based and gives no column, so mark the whole
        // line.
        let line = line.saturating_sub(1);
        let start = lsp_types::Position::new(line, 0);
        let end = lsp_types::Position::new(line, u32::MAX);
        set.extend([Diagnostic::new(start..end, message)
            .with_severity(lsp_types::DiagnosticSeverity::ERROR)]);
    }
    cx.notify();
}

fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_string()
}
