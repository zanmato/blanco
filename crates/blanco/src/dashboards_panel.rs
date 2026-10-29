//! The sidebar's list of dashboards, opened from the rail. Dashboards are
//! renamed in place, as snippets are.

use app_database::AppDatabase;
use blanco_ui::IconName;
use gpui::{
    AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, PopupMenuItem},
    notification::NotificationType,
    scroll::ScrollableElement as _,
    v_flex,
};

use crate::editor::EditorPanel;
use crate::editor::dashboard::{DashboardDirectory, confirm_delete_dashboard};
use crate::result_ext::ResultExt as _;

const ROW_HEIGHT: gpui::Pixels = px(28.);

/// The name a dashboard made from the sidebar starts with, renamed at once.
const NEW_DASHBOARD_NAME: &str = "New Dashboard";

/// The name field shown in place of a dashboard's name while it is renamed.
struct Renaming {
    id: i64,
    input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

pub struct DashboardsPanel {
    editor_panel: Entity<EditorPanel>,
    filter_input: Entity<InputState>,
    scroll_handle: ScrollHandle,
    renaming: Option<Renaming>,
    _subscriptions: Vec<Subscription>,
}

impl DashboardsPanel {
    pub fn new(
        editor_panel: Entity<EditorPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let subscriptions = vec![
            // The active tab decides which dashboard is marked.
            cx.observe(&editor_panel, |_, _, cx| cx.notify()),
            cx.observe_global::<DashboardDirectory>(|_, cx| cx.notify()),
            cx.subscribe(&filter_input, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
        ];
        Self {
            editor_panel,
            filter_input,
            scroll_handle: ScrollHandle::new(),
            renaming: None,
            _subscriptions: subscriptions,
        }
    }

    /// Make a dashboard, show it, and start renaming it here.
    fn create_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let database = AppDatabase::global(cx).clone();
        let editor = self.editor_panel.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let created = database.create_dashboard(NEW_DASHBOARD_NAME, None).await;
            this.update_in(cx, |_, window, cx| match created {
                Ok(dashboard_id) => {
                    DashboardDirectory::reload(cx);
                    editor
                        .update(cx, |editor, cx| {
                            editor.open_dashboard_tab(dashboard_id, window, cx);
                        })
                        .log_err();
                    // After the tab took focus, so the field keeps it.
                    cx.defer_in(window, move |this, window, cx| {
                        this.start_rename(dashboard_id, NEW_DASHBOARD_NAME.to_string(), window, cx)
                    });
                }
                Err(error) => window.push_notification(
                    (
                        NotificationType::Error,
                        format!("The dashboard was not created: {error}"),
                    ),
                    cx,
                ),
            })
            .log_err();
        })
        .detach();
    }

    /// Show a name field in place of the dashboard's name. Enter or leaving
    /// the field renames it, and Escape leaves it as it was.
    pub(crate) fn start_rename(
        &mut self,
        id: i64,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(cx),
                    InputEvent::Change | InputEvent::Focus => {}
                },
            );
        // Escape belongs to the field itself, so it is caught before the
        // field handles it.
        let panel = cx.entity().downgrade();
        let escape = cx.intercept_keystrokes(move |event, window, cx| {
            if event.keystroke.key != "escape" || event.keystroke.modifiers.modified() {
                return;
            }
            panel
                .update(cx, |panel, cx| {
                    let focused = panel
                        .renaming
                        .as_ref()
                        .is_some_and(|renaming| renaming.input.focus_handle(cx).is_focused(window));
                    if focused {
                        cx.stop_propagation();
                        panel.renaming = None;
                        cx.notify();
                    }
                })
                .log_err();
        });
        self.renaming = Some(Renaming {
            id,
            input,
            _subscriptions: vec![events, escape],
        });
        cx.notify();
    }

    /// Rename the dashboard to what the name field holds. An empty name
    /// leaves it as it was.
    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        cx.notify();
        let name = renaming.input.read(cx).value().trim().to_string();
        let unchanged = DashboardDirectory::dashboards(cx)
            .iter()
            .find(|dashboard| dashboard.id == renaming.id)
            .is_some_and(|dashboard| dashboard.name == name);
        if name.is_empty() || unchanged {
            return;
        }
        let database = AppDatabase::global(cx).clone();
        let id = renaming.id;
        cx.spawn(async move |_, cx| {
            let renamed = database.rename_dashboard(id, &name).await;
            cx.update(|cx| match renamed {
                Ok(()) => DashboardDirectory::reload(cx),
                Err(error) => tracing::error!("Failed to rename the dashboard: {error}"),
            });
        })
        .detach();
    }

    fn rename_input(&self, id: i64) -> Option<Entity<InputState>> {
        self.renaming
            .as_ref()
            .filter(|renaming| renaming.id == id)
            .map(|renaming| renaming.input.clone())
    }

    fn render_row(
        &self,
        id: i64,
        name: SharedString,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let rename_input = self.rename_input(id);
        let renaming = rename_input.is_some();
        let delete_name = name.to_string();
        let rename_name = name.to_string();
        let open = self.editor_panel.downgrade();
        let open_from_menu = self.editor_panel.downgrade();
        let panel = cx.entity().downgrade();
        h_flex()
            .id(("dashboard-row", id as usize))
            .h(ROW_HEIGHT)
            .px_3()
            .gap_2()
            .items_center()
            .text_sm()
            .when(!renaming, |row| row.cursor_pointer())
            .when(active, |row| row.bg(theme.list_active))
            .when(!active, |row| row.hover(|row| row.bg(theme.list_hover)))
            .child(
                Icon::new(IconName::LayoutDashboard)
                    .small()
                    .text_color(theme.green),
            )
            .map(|row| match rename_input {
                Some(input) => {
                    row.child(div().flex_1().min_w_0().child(Input::new(&input).xsmall()))
                }
                None => row.child(div().flex_1().min_w_0().truncate().child(name)),
            })
            .on_click(move |_, window, cx| {
                // Clicks in the name field place the caret, and open nothing.
                if renaming {
                    return;
                }
                open.update(cx, |editor, cx| {
                    editor.open_dashboard_tab(id, window, cx);
                })
                .log_err();
            })
            .context_menu(move |menu, _, _| {
                let delete_name = delete_name.clone();
                let rename_name = rename_name.clone();
                let open = open_from_menu.clone();
                let panel = panel.clone();
                menu.item(PopupMenuItem::new("Open").on_click(move |_, window, cx| {
                    open.update(cx, |editor, cx| {
                        editor.open_dashboard_tab(id, window, cx);
                    })
                    .log_err();
                }))
                .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                    let name = rename_name.clone();
                    // Once the menu has closed, since closing it gives focus
                    // back to where it was.
                    panel
                        .update(cx, |_, cx| {
                            cx.defer_in(window, move |panel, window, cx| {
                                panel.start_rename(id, name, window, cx)
                            });
                        })
                        .log_err();
                }))
                .separator()
                .item(
                    PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                        confirm_delete_dashboard(id, delete_name.clone(), window, cx);
                    }),
                )
            })
    }
}

impl Render for DashboardsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.editor_panel.read(cx).active_dashboard_id(cx);
        let needle = self.filter_input.read(cx).value().to_lowercase();
        let dashboards: Vec<(i64, SharedString)> = DashboardDirectory::dashboards(cx)
            .iter()
            .filter(|dashboard| {
                needle.is_empty()
                    || dashboard.name.to_lowercase().contains(&needle)
                    || self.rename_input(dashboard.id).is_some()
            })
            .map(|dashboard| (dashboard.id, SharedString::from(dashboard.name.clone())))
            .collect();
        let no_dashboards = DashboardDirectory::dashboards(cx).is_empty();
        let rows: Vec<_> = dashboards
            .into_iter()
            .map(|(id, name)| {
                self.render_row(id, name, active == Some(id), cx)
                    .into_any_element()
            })
            .collect();
        let muted = cx.theme().muted_foreground;

        v_flex()
            .id("dashboards-panel")
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .pb_1p5()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.filter_input).small().cleanable(true)),
                    )
                    .child(
                        Button::new("dashboards-new")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip("New Dashboard")
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.create_dashboard(window, cx)
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when(no_dashboards, |this| {
                        this.child(
                            v_flex()
                                .px_3()
                                .py_4()
                                .gap_2()
                                .text_sm()
                                .text_color(muted)
                                .child("No dashboards yet.")
                                .child(
                                    "Create one here, or run a query and pick New Dashboard \
                                     under the dashboard button in its results.",
                                ),
                        )
                    })
                    .child(
                        v_flex()
                            .id("dashboards-scroll")
                            .size_full()
                            .py_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .children(rows),
                    )
                    .vertical_scrollbar(&self.scroll_handle),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_harness::TestHarness;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    async fn test_a_dashboard_is_renamed_in_place(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let (database, handle) = cx.update(|_, cx| {
            (
                AppDatabase::global(cx).clone(),
                gpui_tokio::Tokio::handle(cx),
            )
        });
        let dashboard_id = handle
            .block_on(database.create_dashboard("Sales", None))
            .expect("dashboard");
        cx.update(|_, cx| DashboardDirectory::reload(cx));
        cx.run_until_parked();
        let editor = harness.editor_panel.clone();
        let panel = cx.update(|window, cx| cx.new(|cx| DashboardsPanel::new(editor, window, cx)));

        // Escape leaves the name as it was.
        panel.update_in(&mut cx, |panel, window, cx| {
            panel.start_rename(dashboard_id, "Sales".to_string(), window, cx)
        });
        let input = panel
            .read_with(&cx, |panel, _| panel.rename_input(dashboard_id))
            .expect("a name field");
        input.update_in(&mut cx, |input, window, cx| {
            input.set_value("Ignored", window, cx)
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(panel.read_with(&cx, |panel, _| panel.renaming.is_none()));

        panel.update_in(&mut cx, |panel, window, cx| {
            panel.start_rename(dashboard_id, "Sales".to_string(), window, cx)
        });
        let input = panel
            .read_with(&cx, |panel, _| panel.rename_input(dashboard_id))
            .expect("a name field");
        input.update_in(&mut cx, |input, window, cx| {
            input.set_value("Revenue", window, cx)
        });
        // Enter and leaving the field both commit; the panel is not drawn
        // here for the field to take the key.
        panel.update(&mut cx, |panel, cx| panel.commit_rename(cx));
        let mut name = None;
        for _ in 0..100 {
            cx.run_until_parked();
            name = cx.update(|_, cx| {
                DashboardDirectory::dashboards(cx)
                    .iter()
                    .find(|dashboard| dashboard.id == dashboard_id)
                    .map(|dashboard| dashboard.name.clone())
            });
            if name.as_deref() == Some("Revenue") {
                break;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
        assert_eq!(name.as_deref(), Some("Revenue"));
    }
}
