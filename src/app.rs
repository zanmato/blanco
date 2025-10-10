use gpui::{
    div, px, prelude::FluentBuilder, Action, App, AppContext, Context, Entity, FocusHandle,
    Focusable, InteractiveElement, IntoElement, Menu, MenuItem, MouseButton, ParentElement,
    Render, Styled, Window, actions,
};
use gpui_component::{
    h_flex, v_flex, ActiveTheme, IconName, Sizable, TitleBar,
    button::{Button, ButtonVariants},
    menu::AppMenuBar,
};
use serde::Deserialize;

use crate::{
    editor_panel::{EditorPanel, EditorPanelEvent},
    results_panel::ResultsPanel,
    sidebar::ConnectionSidebar,
};

actions!(blanco_app, [Quit, About, NewQuery, OpenConnection, OpenSettings]);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = blanco_app, no_json)]
struct ToggleSidebar;



use gpui::Subscription;

pub struct BlancoApp {
    focus_handle: FocusHandle,
    sidebar: Entity<ConnectionSidebar>,
    editor_panel: Entity<EditorPanel>,
    results_panel: Entity<ResultsPanel>,
    sidebar_collapsed: bool,
    app_menu_bar: Entity<AppMenuBar>,
    _subscriptions: Vec<Subscription>,
}

impl BlancoApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        init_menus(cx);

        let sidebar = cx.new(|cx| ConnectionSidebar::new(window, cx));
        let editor_panel = cx.new(|cx| EditorPanel::new(window, cx));
        let results_panel = cx.new(|cx| ResultsPanel::new(window, cx));
        let app_menu_bar = AppMenuBar::new(window, cx);

        // Load saved query tabs
        editor_panel.update(cx, |panel, cx| {
            panel.load_saved_tabs(window, cx);
        });

        // Subscribe to editor panel events
        let results_panel_clone = results_panel.clone();
        let subscription = cx.subscribe(&editor_panel, move |_this, _emitter, event: &EditorPanelEvent, cx| {
            match event {
                EditorPanelEvent::QueryExecuted(result) => {
                    results_panel_clone.update(cx, |panel, cx| {
                        panel.set_query_result(result.clone(), cx);
                    });
                }
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            editor_panel,
            results_panel,
            sidebar_collapsed: false,
            app_menu_bar,
            _subscriptions: vec![subscription],
        }
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        // Save tabs before quitting
        self.editor_panel.update(cx, |panel, cx| {
            panel.save_tabs(cx);
        });

        // Give a moment for the save to complete
        std::thread::sleep(std::time::Duration::from_millis(100));
        cx.quit();
    }

    fn on_about(&mut self, _: &About, _: &mut Window, _: &mut Context<Self>) {
        println!("Blanco SQL Editor v0.1.0");
    }

    fn on_new_query(&mut self, _: &NewQuery, _: &mut Window, cx: &mut Context<Self>) {
        // TODO: Add new query tab
        cx.notify();
    }

    fn on_open_connection(&mut self, _: &OpenConnection, _: &mut Window, cx: &mut Context<Self>) {
        // TODO: Open connection dialog
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        cx.notify();
    }

    fn on_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_settings_tab(window, cx);
        });
    }
}

impl Focusable for BlancoApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BlancoApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_about))
            .on_action(cx.listener(Self::on_new_query))
            .on_action(cx.listener(Self::on_open_connection))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Title bar
            .child(
                TitleBar::new()
                    .child(div().flex().items_center().child(self.app_menu_bar.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .px_2()
                            .gap_2()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new("github")
                                    .icon(IconName::GitHub)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| {
                                        cx.open_url("https://github.com/yourusername/blanco")
                                    }),
                            ),
                    ),
            )
            // Main content area
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    // Sidebar
                    .when(!self.sidebar_collapsed, |this| {
                        this.child(
                            div()
                                .w(px(250.))
                                .h_full()
                                .border_r_1()
                                .border_color(cx.theme().border)
                                .child(self.sidebar.clone()),
                        )
                    })
                    // Main panel
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            // Editor panel (with tabs and editor)
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .child(self.editor_panel.clone()),
                            )
                            // Results panel
                            .child(
                                div()
                                    .h(px(300.))
                                    .min_h(px(100.))
                                    .child(self.results_panel.clone()),
                            ),
                    ),
            )
    }
}

fn init_menus(cx: &mut App) {
    // Register keyboard shortcut for settings (Ctrl/Cmd + ,)
    cx.bind_keys([
        gpui::KeyBinding::new("cmd-,", OpenSettings, None),
        gpui::KeyBinding::new("ctrl-,", OpenSettings, None),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Blanco".into(),
            items: vec![
                MenuItem::action("Preferences...", OpenSettings),
                MenuItem::Separator,
                MenuItem::action("About Blanco", About),
                MenuItem::Separator,
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Query", NewQuery),
                MenuItem::Separator,
                MenuItem::action("Open Connection", OpenConnection),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", gpui_component::input::Undo),
                MenuItem::action("Redo", gpui_component::input::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", gpui_component::input::Cut),
                MenuItem::action("Copy", gpui_component::input::Copy),
                MenuItem::action("Paste", gpui_component::input::Paste),
                MenuItem::separator(),
                MenuItem::action("Select All", gpui_component::input::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![MenuItem::action("Toggle Sidebar", ToggleSidebar)],
        },
    ]);
}
