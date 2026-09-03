use blanco_core::RoutineKind;
use blanco_ui::{IconName, SqlViewMessage, Tab, TabBar};
use gpui::{
    AnyElement, App, ClickEvent, Context, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, WeakEntity, Window, div, prelude::FluentBuilder,
    px, rems,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, Sizable, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::Editor,
    popover::{Popover, PopoverState},
    resizable::{resizable_panel, v_resizable},
    v_flex,
};

use super::rename_form::RenameTabForm;
use super::{EditorPanel, QueryTab, ScriptTab, TabType};
use crate::app::{ExecuteSubstitutedQuery, FormatQuery, RenameTab};
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;
use app_database::EnvironmentType;

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

                let tab_title = this
                    .tabs
                    .get(*ix)
                    .and_then(TabType::connection_tab)
                    .map(|tab| tab.title.clone());
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
            .border_color(crate::connections::environment_color(env_type, cx))
            .text_color(crate::connections::environment_color(env_type, cx))
            .child(env_type.display_name())
    }

    fn render_tab_bar_item(&self, ix: usize, tab: &TabType, cx: &mut Context<Self>) -> Tab {
        let last_run_at = tab.connection_tab().and_then(|tab| tab.last_run_at);
        // Mirror the tab strip: the last remaining tab cannot be closed.
        let closable = self.tabs.len() > 1;
        self.render_tab_bar_item_inner(ix, tab, cx)
            .when_some(last_run_at, |this, timestamp| {
                this.menu_detail(crate::time_format::format_relative(timestamp))
            })
            .when(closable, |this| {
                let close = cx.listener(|this, index: &usize, _, cx| this.close_tab(*index, cx));
                this.on_close(move |index, _, window, cx| close(index, window, cx))
            })
    }

    /// The icon that tells the tab kinds apart in the strip.
    fn tab_icon(tab: &TabType, cx: &App) -> Option<Icon> {
        let theme = cx.theme();
        Some(match tab {
            TabType::Query(_) => return None,
            TabType::Script(_) => Icon::new(IconName::Braces).text_color(theme.yellow),
            TabType::Snippet(_) => Icon::new(IconName::File).text_color(theme.green),
            TabType::Settings(_) => Icon::new(IconName::Settings),
            TabType::ObjectDdl(tab) => {
                let (icon, color) = match tab.read(cx).kind {
                    RoutineKind::Procedure => (IconName::SquareTerminal, theme.magenta),
                    RoutineKind::Function => (IconName::Braces, theme.cyan),
                    RoutineKind::Trigger => (IconName::DatabaseConnected, theme.yellow),
                };
                Icon::new(icon).text_color(color)
            }
            TabType::TableStructure(_) => Icon::new(IconName::Sheet).text_color(theme.blue),
            TabType::SchemaGraph(_) => Icon::new(IconName::Network).text_color(theme.cyan),
        })
    }

    /// One strip item for any tab kind. Tabs bound to a connection are grouped
    /// under its name with the environment badge; the rest sit under "Other".
    fn render_tab_bar_item_inner(&self, ix: usize, tab: &TabType, cx: &mut Context<Self>) -> Tab {
        let show_close_button = self.tabs.len() > 1;
        let label = tab.title(cx);
        let context = tab.context(cx);
        let icon = Self::tab_icon(tab, cx);

        let mut item = Tab::new().label(label);
        item = match &context {
            Some(context) => {
                let group_label = context.connection_name.clone();
                let group_env_type = context.environment_type;
                item.group(context.connection_name.clone())
                    .group_label(move |_, cx| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(group_label.clone()),
                            )
                            .when_some(group_env_type, |this, env_type| {
                                this.child(Self::environment_badge(env_type, cx))
                            })
                    })
            }
            None => item.group("Other"),
        };

        let connection_suffix = tab.connection_tab().map(|tab| {
            (
                tab.context.connection_name.clone(),
                tab.context.environment_type,
            )
        });

        item.suffix(
            h_flex()
                .gap_1()
                .items_center()
                .pr_1()
                .when_some(icon, |this, icon| this.child(icon))
                .when_some(connection_suffix, |this, (name, env_type)| {
                    this.child(
                        div()
                            .pr_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(name),
                    )
                    .when_some(env_type, |this, env_type| {
                        this.child(Self::environment_badge(env_type, cx))
                    })
                })
                .when(show_close_button, |this| {
                    this.child(self.close_tab_button(("close-tab", ix), ix, cx))
                })
                .into_any_element(),
        )
    }

    fn render_row_operations_bar(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Row editing (Add/Duplicate/Delete/Apply/Discard) mutates table rows,
        // which non-SQL backends (e.g. Redis key/value) don't support, so those
        // buttons are hidden for them. The SQL Log and Terminal toggles stay:
        // Redis tabs still have a command log view. Gate on the backend capability so
        // new drivers slot in automatically.
        let supports_sql = query_tab.context.db_type.supports_sql();
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
            // The terminal pane docks under this bar, so the rounded corners
            // belong to the terminal when it is open.
            .when(!query_tab.terminal_enabled, |this| {
                this.rounded_b(crate::app::PANEL_RADIUS)
            })
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
                    .icon(IconName::Logs)
                    .tooltip("Toggle SQL Log")
                    .when(query_tab.sql_view_visible, |btn| btn.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_sql_view_for_active_tab(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-terminal")
                    .outline()
                    .small()
                    .icon(IconName::SquareTerminal)
                    .tooltip("Toggle Terminal")
                    .when(query_tab.terminal_enabled, |btn| btn.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_terminal_for_active_tab(window, cx);
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
        let preview_log = query_tab.commit_preview.clone();

        Popover::new("commit-changes-popover")
            .trigger(button)
            .on_open_change({
                let results_panel = results_panel.clone();
                let preview_log = preview_log.clone();
                move |open, _window, cx| {
                    if !*open {
                        return;
                    }
                    // A cell may still be in edit mode (its blur commit only
                    // fires when focus moves into the table); fold it into the
                    // tracked changes so the preview reflects it.
                    results_panel.update(cx, |panel, cx| panel.finalize_active_cell_edit(cx));
                    let statements = results_panel.read(cx).preview_pending_sql(cx);
                    preview_log.update(cx, |log, cx| {
                        log.clear(cx);
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
                    });
                }
            })
            .content(move |_state, _window, cx| {
                let results_panel = results_panel.clone();
                let sql_view = sql_view.clone();
                let preview_log = preview_log.clone();
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

    /// The tab's editor.
    fn render_editor(
        &self,
        editor: &gpui::Entity<gpui_component::input::EditorState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div().size_full().min_h_0().min_w_0().relative().child(
            Editor::new(editor)
                .bordered(false)
                .h_full()
                .w_full()
                .rounded_none()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(14.)),
        )
    }

    /// The tab's main column (editor, results and the bottom bar) with the
    /// terminal pane docked under it when the pane is open, like the panel
    /// area in VS Code and Zed. The split has its own state so the terminal
    /// height is remembered across toggles without disturbing the
    /// editor/results split.
    fn render_with_terminal(
        &self,
        content: impl IntoElement,
        terminal: Option<gpui::Entity<blanco_terminal::view::TerminalView>>,
        split_id: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = div().size_full().min_h_0().min_w_0().child(content);
        div()
            .h_full()
            .w_full()
            .overflow_hidden()
            .min_w_0()
            .map(|this| match terminal {
                Some(terminal) => this.child(
                    v_resizable(split_id)
                        .with_state(&self.editor_terminal_resize_state)
                        .child(resizable_panel().child(content))
                        .child(
                            resizable_panel().size(220.).child(
                                div()
                                    .size_full()
                                    .min_h_0()
                                    .border_t_1()
                                    .border_color(cx.theme().border)
                                    .child(terminal),
                            ),
                        ),
                ),
                None => this.child(content),
            })
    }

    fn render_query_tab_content(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let supports_sql = query_tab.context.db_type.supports_sql();
        self.render_with_terminal(
            v_resizable("editor-results-split")
                .with_state(&self.editor_results_resize_state)
                .child(
                    resizable_panel()
                        .size(200.)
                        .child(self.render_editor(&query_tab.editor, cx)),
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
                                        let position = query_tab.editor.read(cx).cursor_position();
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
                                                                    panel.on_run_query(window, cx)
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
                                                    .with_state(&self.results_log_resize_state)
                                                    .child(
                                                        resizable_panel()
                                                            .child(query_tab.results_panel.clone()),
                                                    )
                                                    .child(
                                                        resizable_panel()
                                                            .size(120.)
                                                            .child(query_tab.sql_view.clone()),
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
            query_tab.base.terminal_pane_view(),
            "editor-terminal-split",
            cx,
        )
    }

    /// Script tabs are the query layout minus everything SQL-specific: editor
    /// on top, then the results grid (fed only by `db.display`) beside the
    /// console log, and a bar with Run/Stop.
    fn render_script_tab_content(
        &self,
        script_tab: &ScriptTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.render_with_terminal(
            v_resizable("script-editor-results-split")
                .with_state(&self.editor_results_resize_state)
                .child(
                    resizable_panel()
                        .size(200.)
                        .child(self.render_editor(&script_tab.editor, cx)),
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
                                        let position = script_tab.editor.read(cx).cursor_position();
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
                            .child(
                                div()
                                    .flex_1()
                                    .w_full()
                                    .min_h_0()
                                    .overflow_hidden()
                                    .map(|d| {
                                        if script_tab.log_visible {
                                            d.child(
                                                v_resizable("script-results-log-split")
                                                    .with_state(&self.results_log_resize_state)
                                                    .child(
                                                        resizable_panel().child(
                                                            script_tab.results_panel.clone(),
                                                        ),
                                                    )
                                                    .child(
                                                        resizable_panel()
                                                            .size(160.)
                                                            .child(script_tab.log_view.clone()),
                                                    ),
                                            )
                                        } else {
                                            d.child(script_tab.results_panel.clone())
                                        }
                                    }),
                            )
                            .child(
                                h_flex()
                                    .p_2()
                                    .gap_2()
                                    .border_t_1()
                                    .bg(cx.theme().title_bar)
                                    .border_color(cx.theme().border)
                                    .when(!script_tab.terminal_enabled, |this| {
                                        this.rounded_b(crate::app::PANEL_RADIUS)
                                    })
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("toggle-script-log")
                                            .outline()
                                            .small()
                                            .icon(IconName::Logs)
                                            .tooltip("Toggle console log")
                                            .when(script_tab.log_visible, |btn| btn.primary())
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                if let Some(TabType::Script(script_tab)) =
                                                    this.tabs.get_mut(this.active_tab_ix)
                                                {
                                                    script_tab.log_visible =
                                                        !script_tab.log_visible;
                                                    cx.notify();
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new("toggle-script-terminal")
                                            .outline()
                                            .small()
                                            .icon(IconName::SquareTerminal)
                                            .tooltip("Toggle Terminal")
                                            .when(script_tab.terminal_enabled, |btn| btn.primary())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.toggle_terminal_for_active_tab(window, cx);
                                            })),
                                    ),
                            ),
                    ),
                ),
            script_tab.base.terminal_pane_view(),
            "script-editor-terminal-split",
            cx,
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
