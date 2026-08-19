use blanco_core::RoutineKind;
use blanco_ui::{IconName, SqlView, SqlViewMessage, Tab, TabBar};
use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, WeakEntity,
    Window, div, prelude::FluentBuilder, px, rems,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, Sizable, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::Editor,
    popover::{Popover, PopoverState},
    resizable::{h_resizable, resizable_panel, v_resizable},
    v_flex,
};

use super::rename_form::RenameTabForm;
use super::{EditorPanel, QueryTab, ScriptTab, TabType};
use crate::app::{ExecuteSubstitutedQuery, FormatQuery, RenameTab};
use crate::app_database::EnvironmentType;
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;

impl EditorPanel {
    fn close_tab_button(
        &self,
        id: impl Into<gpui::ElementId>,
        tab_index: usize,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .ghost()
            .xsmall()
            .icon(IconName::Close)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.close_tab(tab_index, cx);
            }))
    }

    /// Create a tab bar click handler closure
    fn tab_bar_click_handler(
        view: WeakEntity<Self>,
    ) -> impl Fn(&usize, &ClickEvent, &mut Window, &mut App) + 'static {
        move |ix: &usize, event: &ClickEvent, window: &mut Window, cx: &mut App| {
            view.update(cx, |this: &mut EditorPanel, cx| {
                if event.click_count() == 1 {
                    this.set_active_tab(*ix, window, cx);
                    return;
                }

                let tab_title = match this.tabs.get(*ix) {
                    Some(TabType::Query(query_tab)) => Some(query_tab.title.clone()),
                    Some(TabType::Script(script_tab)) => Some(script_tab.title.clone()),
                    _ => None,
                };
                if let Some(tab_title) = tab_title {
                    // Open rename modal on double click
                    let form = RenameTabForm::new(tab_title, window, cx);
                    let form_for_modal = form;
                    let tab_index = *ix;

                    window.open_dialog(cx, move |modal, _window, _cx| {
                        let form_clone = form_for_modal.clone();
                        let tab_index = tab_index;
                        modal
                            .title("Rename Tab")
                            .w(px(300.))
                            .child(form_for_modal.clone())
                            .footer(
                                DialogFooter::new()
                                    .child(
                                        DialogClose::new()
                                            .child(Button::new("cancel").label("Cancel").outline()),
                                    )
                                    .child(
                                        DialogAction::new()
                                            .child(Button::new("ok").primary().label("Rename")),
                                    ),
                            )
                            .on_ok({
                                let form = form_clone;
                                move |_modal, window, cx| {
                                    // Get the current value from the form
                                    let new_name = form.read(cx).get_value(cx);

                                    // Use the app's global action system instead of local context
                                    // Create a new RenameTab action and dispatch it through the app
                                    window.dispatch_action(
                                        Box::new(RenameTab {
                                            tab_index,
                                            new_name,
                                        }),
                                        cx,
                                    );
                                    true
                                }
                            })
                    });
                };
            })
            .log_err();
        }
    }

    fn with_active_results_panel(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ResultsPanel, &mut Context<ResultsPanel>),
    ) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.results_panel.update(cx, f);
        }
    }

    /// The small outlined dev/staging/prod chip shown on connection-backed tabs.
    fn environment_badge(env_type: EnvironmentType, cx: &App) -> impl IntoElement {
        div()
            .text_size(rems(0.55))
            .font_family(cx.theme().mono_font_family.clone())
            .px(px(6.))
            .pt_0p5()
            .rounded_md()
            .border_1()
            .border_color(env_type.get_color(cx))
            .text_color(env_type.get_color(cx))
            .child(env_type.display_name())
    }

    fn render_tab_bar_item(&self, ix: usize, tab: &TabType, cx: &mut Context<Self>) -> Tab {
        match tab {
            TabType::Query(query_tab) => {
                let show_close_button = self.tabs.len() > 1;
                let tab_index = ix;

                let connection_label = query_tab
                    .connection_name
                    .clone()
                    .unwrap_or_else(|| "No Connection".to_string());
                let group_env_type = query_tab.environment_type;
                let group_connection_label = connection_label.clone();

                Tab::new()
                    .label(&query_tab.title)
                    .group(connection_label)
                    .group_label(move |_, cx| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(group_connection_label.clone()),
                            )
                            .when_some(group_env_type, |this, env_type| {
                                this.child(Self::environment_badge(env_type, cx))
                            })
                    })
                    .suffix(
                        h_flex()
                            .gap_1()
                            .pr_1()
                            .child(
                                div()
                                    .pr_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        query_tab
                                            .connection_name
                                            .clone()
                                            .unwrap_or_else(|| "No Connection".to_string()),
                                    ),
                            )
                            .when_some(query_tab.environment_type, |this, env_type| {
                                this.child(Self::environment_badge(env_type, cx))
                            })
                            .when(show_close_button, |this| {
                                this.child(self.close_tab_button(("close-tab", ix), tab_index, cx))
                            })
                            .into_any_element(),
                    )
            }
            TabType::Script(script_tab) => {
                let show_close_button = self.tabs.len() > 1;
                let connection_label = script_tab
                    .connection_name
                    .clone()
                    .unwrap_or_else(|| "No Connection".to_string());
                let environment_type = script_tab.environment_type;

                Tab::new()
                    .label(&script_tab.title)
                    .group(connection_label.clone())
                    .group_label({
                        let connection_label = connection_label.clone();
                        move |_, cx| {
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(connection_label.clone()),
                                )
                                .when_some(environment_type, |this, env_type| {
                                    this.child(Self::environment_badge(env_type, cx))
                                })
                        }
                    })
                    .suffix(
                        h_flex()
                            .gap_1()
                            .pr_1()
                            .child(Icon::new(IconName::Braces).text_color(cx.theme().yellow))
                            .child(
                                div()
                                    .pr_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(connection_label),
                            )
                            .when_some(environment_type, |this, env_type| {
                                this.child(Self::environment_badge(env_type, cx))
                            })
                            .when(show_close_button, |this| {
                                this.child(self.close_tab_button(("close-script-tab", ix), ix, cx))
                            })
                            .into_any_element(),
                    )
            }
            TabType::Snippet(snippet_editor) => {
                let label = snippet_editor.read(cx).get_title();
                let tab_index = ix;

                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(IconName::File).text_color(cx.theme().green))
                        .child(self.close_tab_button(("close-snippet-tab", ix), tab_index, cx)),
                )
            }
            TabType::Settings(settings_tab) => {
                let label = settings_tab.title.clone();
                let tab_index = ix;

                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(IconName::Settings))
                        .child(self.close_tab_button(("close-settings-tab", ix), tab_index, cx)),
                )
            }
            TabType::ObjectDdl(object_ddl_tab) => {
                let inner = object_ddl_tab.read(cx);
                let label = SharedString::from(inner.title.clone());
                let (icon, color) = match inner.kind {
                    RoutineKind::Procedure => (IconName::SquareTerminal, cx.theme().magenta),
                    RoutineKind::Function => (IconName::Braces, cx.theme().cyan),
                    RoutineKind::Trigger => (IconName::DatabaseConnected, cx.theme().yellow),
                };
                let tab_index = ix;
                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(icon).text_color(color))
                        .child(self.close_tab_button(("close-object-ddl-tab", ix), tab_index, cx)),
                )
            }
            TabType::TableStructure(table_structure_tab) => {
                let label = table_structure_tab.read(cx).title.clone();
                let tab_index = ix;

                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(IconName::Sheet).text_color(cx.theme().blue))
                        .child(self.close_tab_button(
                            ("close-table-structure-tab", ix),
                            tab_index,
                            cx,
                        )),
                )
            }
            TabType::SchemaGraph(schema_graph_tab) => {
                let label = schema_graph_tab.read(cx).title.clone();
                let tab_index = ix;

                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(IconName::Network).text_color(cx.theme().cyan))
                        .child(self.close_tab_button(
                            ("close-schema-graph-tab", ix),
                            tab_index,
                            cx,
                        )),
                )
            }
        }
    }

    fn render_row_operations_bar(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Row editing (Add/Duplicate/Delete/Apply/Discard) mutates table rows,
        // which non-SQL backends (e.g. Redis key/value) don't support, so those
        // buttons are hidden for them. The SQL Log and Chat toggles stay: Redis
        // tabs still have a command log view. Gate on the backend capability so
        // new drivers slot in automatically.
        let supports_sql = query_tab._db_type.supports_sql();
        let commit_in_progress = query_tab.results_panel.read(cx).is_commit_in_progress();

        h_flex()
            .p_2()
            .gap_2()
            .border_t_1()
            .bg(cx.theme().title_bar)
            .border_color(cx.theme().border)
            // Sits flush against the bottom of the editor card, and GPUI's
            // content mask is rectangular, so it has to round its own corners
            // or it squares off the card's.
            .rounded_b(crate::app::PANEL_RADIUS)
            .flex_wrap()
            .when(supports_sql, |bar| {
                bar.child(
                    Button::new("add-row")
                        .outline()
                        .small()
                        .icon(IconName::Plus)
                        .label("Add")
                        .disabled(commit_in_progress)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.with_active_results_panel(cx, |panel, cx| {
                                panel.add_new_row(cx);
                            });
                        })),
                )
                .child(
                    Button::new("duplicate-row")
                        .outline()
                        .small()
                        .icon(IconName::Copy)
                        .label("Duplicate")
                        .disabled(commit_in_progress)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.with_active_results_panel(cx, |panel, cx| {
                                panel.duplicate_row(cx);
                            });
                        })),
                )
                .child(
                    Button::new("delete-row")
                        .outline()
                        .small()
                        .icon(IconName::Trash)
                        .label("Delete")
                        .disabled(commit_in_progress)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.with_active_results_panel(cx, |panel, cx| {
                                panel.delete_row(cx);
                            });
                        })),
                )
                .child(self.render_apply_edits_button(query_tab, cx))
                .child(
                    Button::new("rollback-changes")
                        .outline()
                        .small()
                        .icon(IconName::CircleX)
                        .label("Discard edits")
                        .tooltip("Discard pending cell edits")
                        .disabled(
                            commit_in_progress
                                || !query_tab.results_panel.read(cx).has_pending_edits(cx),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.with_active_results_panel(cx, |panel, cx| {
                                panel.rollback_changes(window, cx);
                            });
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("toggle-sql-log")
                    .outline()
                    .small()
                    .icon(IconName::SquareTerminal)
                    .tooltip("Toggle SQL Log")
                    .when(query_tab.sql_view_visible, |btn| btn.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_sql_view_for_active_tab(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-chat")
                    .outline()
                    .small()
                    .icon(IconName::Bot)
                    .tooltip("Toggle Chat")
                    .when(query_tab.chat_enabled, |btn| btn.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_chat_for_active_tab(window, cx);
                    })),
            )
    }

    fn render_apply_edits_button(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let results_panel = query_tab.results_panel.read(cx);
        let has_pending = results_panel.has_pending_edits(cx);
        let commit_in_progress = results_panel.is_commit_in_progress();
        let button = Button::new("commit-changes")
            .outline()
            .small()
            .icon(IconName::Check)
            .label("Apply edits")
            .tooltip("Apply pending cell edits to the database")
            .disabled(!has_pending || commit_in_progress);

        if !has_pending || commit_in_progress {
            return button.into_any_element();
        }

        let results_panel = query_tab.results_panel.clone();
        let sql_view = query_tab.sql_view.clone();

        Popover::new("commit-changes-popover")
            .trigger(button)
            .content(move |_state, _window, cx| {
                let results_panel = results_panel.clone();
                let sql_view = sql_view.clone();
                // A cell may still be in edit mode (its blur commit only fires
                // when focus moves into the table); fold it into the tracked
                // changes so the preview reflects it.
                results_panel.update(cx, |panel, cx| panel.finalize_active_cell_edit(cx));
                let statements = results_panel.read(cx).preview_pending_sql(cx);
                let preview_log = cx.new(|cx| {
                    let mut log =
                        SqlView::new(usize::MAX, cx.theme().highlight_theme.clone(), "sql");
                    if statements.is_empty() {
                        log.append_text(
                            &SqlViewMessage::Comment("no statements to apply".into()),
                            cx,
                        );
                    } else {
                        for statement in statements {
                            log.append_text(&SqlViewMessage::SqlStatement(statement), cx);
                        }
                    }
                    log
                });
                v_flex()
                    .p_2()
                    .gap_2()
                    .w(px(520.))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("Preview SQL"),
                    )
                    .child(
                        v_flex()
                            .h(px(280.))
                            .min_h_0()
                            .overflow_hidden()
                            .child(preview_log),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("preview-cancel")
                                    .outline()
                                    .small()
                                    .label("Cancel")
                                    .on_click(cx.listener(
                                        |state: &mut PopoverState, _, window, cx| {
                                            state.dismiss(window, cx);
                                        },
                                    )),
                            )
                            .child(
                                Button::new("preview-confirm")
                                    .primary()
                                    .small()
                                    .icon(IconName::Check)
                                    .label("Confirm")
                                    .on_click({
                                        let results_panel = results_panel;
                                        let sql_view = sql_view;
                                        cx.listener(
                                            move |state: &mut PopoverState, _, window, cx| {
                                                results_panel.update(cx, |panel, cx| {
                                                    panel.commit_changes_with_sql_view(
                                                        window, &sql_view, cx,
                                                    );
                                                });
                                                state.dismiss(window, cx);
                                            },
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_query_tab_content(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let supports_sql = query_tab._db_type.supports_sql();
        h_resizable("editor-split")
            .with_state(&self.editor_chat_resize_state)
            .child(
                resizable_panel().child(
                    v_resizable("editor-results-split")
                        .with_state(&self.editor_results_resize_state)
                        .child(
                            resizable_panel().size(200.).child(
                                v_flex()
                                    .h_full()
                                    .w_full()
                                    .overflow_hidden()
                                    .min_w_0()
                                    .child(
                                        div().flex_1().min_h_0().w_full().relative().child(
                                            Editor::new(&query_tab.editor)
                                                .bordered(false)
                                                .h_full()
                                                .w_full()
                                                .rounded_none()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_size(px(14.)),
                                        ),
                                    ),
                            ),
                        )
                        .child(
                            resizable_panel().size(200.).child(
                                v_flex()
                                    .h_full()
                                    .w_full()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .p_2()
                                            .gap_2()
                                            .border_t_1()
                                            .border_color(cx.theme().border)
                                            .bg(cx.theme().title_bar)
                                            .justify_between()
                                            .child({
                                                let position =
                                                    query_tab.editor.read(cx).cursor_position();
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "Ln {}, Col {}",
                                                        position.line + 1,
                                                        position.character + 1
                                                    ))
                                            })
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    // Format/lint and EXPLAIN are SQL-only; hide
                                                    // them for non-SQL backends (e.g. Redis).
                                                    .when(supports_sql, |this| {
                                                        this.child(
                                                            Button::new("format-query")
                                                                .outline()
                                                                .small()
                                                                .icon(IconName::WandSparkles)
                                                                .label("Format")
                                                                .tooltip(format!(
                                                                    "Format ({})",
                                                                    self.format_query_keystroke
                                                                ))
                                                                .on_click(cx.listener(
                                                                    |panel, _, window, cx| {
                                                                        panel.format_current_query(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                )),
                                                        )
                                                    })
                                                    .map(|this| {
                                                        if self.loading {
                                                            this.child(
                                                        Button::new("abort-query")
                                                            .danger()
                                                            .small()
                                                            .icon(IconName::SquareStop)
                                                            .label("Stop")
                                                            .tooltip("Abort the running query")
                                                            .on_click(cx.listener(
                                                                |panel, _, _window, cx| {
                                                                    panel.abort_running_query(cx);
                                                                },
                                                            )),
                                                    )
                                                        } else {
                                                            this.when(supports_sql, |this| {
                                                                this.child(
                                                        Button::new("explain-query")
                                                            .outline()
                                                            .small()
                                                            .icon(IconName::Map)
                                                            .label("Explain")
                                                            .tooltip(
                                                                "Run EXPLAIN on the statement \
                                                                 at the cursor",
                                                            )
                                                            .on_click(cx.listener(
                                                                |panel, _, window, cx| {
                                                                    panel.on_explain_query(
                                                                        window, cx,
                                                                    )
                                                                },
                                                            )),
                                                    )
                                                            })
                                                            .child(
                                                                Button::new("run-query")
                                                                    .outline()
                                                                    .small()
                                                                    .icon(IconName::Play)
                                                                    .label("Run Current")
                                                                    .tooltip(format!(
                                                                        "Run Current ({})",
                                                                        self.run_query_keystroke
                                                                    ))
                                                                    .on_click(cx.listener(
                                                                        |panel, _, window, cx| {
                                                                            panel.on_run_query(
                                                                                window, cx,
                                                                            )
                                                                        },
                                                                    )),
                                                            )
                                                        }
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            // Without an explicit width this wrapper collapses to
                                            // its content's max-content width during intrinsic
                                            // sizing: a single narrow result column (or a long,
                                            // unwrappable SQL line in the log below) leaves the
                                            // results table pinned to that width instead of filling
                                            // the resizable panel. `w_full` forces it to the panel.
                                            .w_full()
                                            .min_h_0()
                                            .overflow_hidden()
                                            .map(|d| {
                                                if query_tab.sql_view_visible {
                                                    d.child(
                                                        v_resizable("results-log-split")
                                                            .with_state(
                                                                &self.results_log_resize_state,
                                                            )
                                                            .child(resizable_panel().child(
                                                                query_tab.results_panel.clone(),
                                                            ))
                                                            .child(
                                                                resizable_panel().size(120.).child(
                                                                    query_tab.sql_view.clone(),
                                                                ),
                                                            ),
                                                    )
                                                } else {
                                                    d.child(query_tab.results_panel.clone())
                                                }
                                            }),
                                    )
                                    .child(self.render_row_operations_bar(query_tab, cx)),
                            ),
                        ),
                ),
            )
            .when(
                query_tab.chat_enabled && query_tab.chat_panel.is_some(),
                |this| {
                    this.child(
                        resizable_panel()
                            .size_range(px(500.)..gpui::Pixels::MAX)
                            .child(
                                div()
                                    .border_l_1()
                                    .border_color(cx.theme().border)
                                    .size_full()
                                    .min_h_0()
                                    .when_some(
                                        query_tab.chat_panel.as_ref(),
                                        |this, chat_panel| this.child(chat_panel.clone()),
                                    ),
                            ),
                    )
                },
            )
    }

    /// Script tabs are the query layout minus everything SQL-specific: editor
    /// on top, then the results grid (fed only by `db.display`) beside the
    /// console log, a bar with Run/Stop, and the chat panel alongside.
    fn render_script_tab_content(
        &self,
        script_tab: &ScriptTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_resizable("script-editor-split")
            .with_state(&self.script_editor_chat_resize_state)
            .child(
                resizable_panel().child(
                    v_resizable("script-editor-results-split")
                        .with_state(&self.editor_results_resize_state)
                        .child(
                            resizable_panel().size(200.).child(
                                v_flex()
                                    .h_full()
                                    .w_full()
                                    .overflow_hidden()
                                    .min_w_0()
                                    .child(
                                        div().flex_1().min_h_0().w_full().relative().child(
                                            Editor::new(&script_tab.editor)
                                                .bordered(false)
                                                .h_full()
                                                .w_full()
                                                .rounded_none()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_size(px(14.)),
                                        ),
                                    ),
                            ),
                        )
                        .child(
                            resizable_panel().size(200.).child(
                                v_flex()
                                    .h_full()
                                    .w_full()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .p_2()
                                            .gap_2()
                                            .border_t_1()
                                            .border_color(cx.theme().border)
                                            .bg(cx.theme().title_bar)
                                            .justify_between()
                                            .child({
                                                let position =
                                                    script_tab.editor.read(cx).cursor_position();
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "Ln {}, Col {}",
                                                        position.line + 1,
                                                        position.character + 1
                                                    ))
                                            })
                                            .child(h_flex().gap_2().map(|this| {
                                                if self.loading {
                                                    this.child(
                                                        Button::new("stop-script")
                                                            .danger()
                                                            .small()
                                                            .icon(IconName::SquareStop)
                                                            .label("Stop")
                                                            .tooltip("Stop the running script")
                                                            .on_click(cx.listener(
                                                                |panel, _, _window, cx| {
                                                                    panel.cancel_running_script(cx);
                                                                },
                                                            )),
                                                    )
                                                } else {
                                                    this.child(
                                                        Button::new("run-script")
                                                            .outline()
                                                            .small()
                                                            .icon(IconName::Play)
                                                            .label("Run Script")
                                                            .tooltip(format!(
                                                                "Run Script ({})",
                                                                self.run_query_keystroke
                                                            ))
                                                            .on_click(cx.listener(
                                                                |panel, _, window, cx| {
                                                                    panel.on_run_query(window, cx)
                                                                },
                                                            )),
                                                    )
                                                }
                                            })),
                                    )
                                    .child(div().flex_1().w_full().min_h_0().overflow_hidden().map(
                                        |d| {
                                            if script_tab.log_visible {
                                                d.child(
                                                    v_resizable("script-results-log-split")
                                                        .with_state(&self.results_log_resize_state)
                                                        .child(resizable_panel().child(
                                                            script_tab.results_panel.clone(),
                                                        ))
                                                        .child(
                                                            resizable_panel()
                                                                .size(160.)
                                                                .child(script_tab.log_view.clone()),
                                                        ),
                                                )
                                            } else {
                                                d.child(script_tab.results_panel.clone())
                                            }
                                        },
                                    ))
                                    .child(
                                        h_flex()
                                            .p_2()
                                            .gap_2()
                                            .border_t_1()
                                            .bg(cx.theme().title_bar)
                                            .border_color(cx.theme().border)
                                            .rounded_b(crate::app::PANEL_RADIUS)
                                            .child(div().flex_1())
                                            .child(
                                                Button::new("toggle-script-log")
                                                    .outline()
                                                    .small()
                                                    .icon(IconName::SquareTerminal)
                                                    .tooltip("Toggle console log")
                                                    .when(script_tab.log_visible, |btn| {
                                                        btn.primary()
                                                    })
                                                    .on_click(cx.listener(
                                                        |this, _, _window, cx| {
                                                            if let Some(TabType::Script(
                                                                script_tab,
                                                            )) = this
                                                                .tabs
                                                                .get_mut(this.active_tab_ix)
                                                            {
                                                                script_tab.log_visible =
                                                                    !script_tab.log_visible;
                                                                cx.notify();
                                                            }
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new("toggle-script-chat")
                                                    .outline()
                                                    .small()
                                                    .icon(IconName::Bot)
                                                    .tooltip("Toggle Chat")
                                                    .when(script_tab.chat_enabled, |btn| {
                                                        btn.primary()
                                                    })
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.toggle_chat_for_active_tab(
                                                                window, cx,
                                                            );
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                        ),
                ),
            )
            .when(
                script_tab.chat_enabled && script_tab.chat_panel.is_some(),
                |this| {
                    this.child(
                        resizable_panel()
                            .size_range(px(500.)..gpui::Pixels::MAX)
                            .child(
                                div()
                                    .border_l_1()
                                    .border_color(cx.theme().border)
                                    .size_full()
                                    .min_h_0()
                                    .when_some(
                                        script_tab.chat_panel.as_ref(),
                                        |this, chat_panel| this.child(chat_panel.clone()),
                                    ),
                            ),
                    )
                },
            )
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_tab = self.tabs.get(self.active_tab_ix);

        div()
            // The action handlers below live on this node, so the focus handle
            // has to be tracked here, or the dispatch path can miss them.
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .on_action(
                cx.listener(|this, action: &ExecuteSubstitutedQuery, window, cx| {
                    this.execute_query(
                        action.query.clone(),
                        action.connection_id,
                        &action.database_name,
                        window,
                        cx,
                    );
                }),
            )
            .on_action(cx.listener(|this, _: &FormatQuery, window, cx| {
                this.format_current_query(window, cx);
            }))
            // The segmented trough paints its own background, and padding on
            // the bar itself would make it taller rather than inset it, which
            // squares off the card's top corners. Inset it with a wrapper.
            // `min_w_0` is what lets the strip overflow and scroll.
            .child(
                div().p(crate::app::PANEL_GAP).min_w_0().child(
                    TabBar::new("editor-tabs")
                        .segmented()
                        .menu(true)
                        .w_full()
                        .selected_index(self.active_tab_ix)
                        .on_click(Self::tab_bar_click_handler(cx.entity().downgrade()))
                        .children(
                            self.tabs
                                .iter()
                                .enumerate()
                                .map(|(ix, tab)| self.render_tab_bar_item(ix, tab, cx)),
                        )
                        .track_scroll(&self.tabbar_scroll_handle),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when_some(current_tab, |this, tab| match tab {
                        TabType::Query(query_tab) => {
                            this.child(self.render_query_tab_content(query_tab, cx))
                        }
                        TabType::Script(script_tab) => {
                            this.child(self.render_script_tab_content(script_tab, cx))
                        }
                        TabType::Settings(settings_tab) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(settings_tab.settings_view.clone()),
                        ),
                        TabType::Snippet(snippet_editor) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(snippet_editor.clone()),
                        ),
                        TabType::TableStructure(table_structure_tab) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(table_structure_tab.clone()),
                        ),
                        TabType::ObjectDdl(object_ddl_tab) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(object_ddl_tab.clone()),
                        ),
                        TabType::SchemaGraph(schema_graph_tab) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(schema_graph_tab.clone()),
                        ),
                    }),
            )
    }
}
