use gpui::{div, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render, SharedString, Styled, Window};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarMenu, SidebarMenuItem, SidebarToggleButton},
    v_flex, ActiveTheme, ContextModal as _, IconName, Side,
};

use crate::app::OpenNewConnectionModal;
use crate::connection::Connection;
use crate::db_service::DbService;

pub struct ConnectionSidebar {
    focus_handle: FocusHandle,
    connections: Vec<Connection>,
    test_db_tables: Vec<String>,
    collapsed: bool,
    test_db_expanded: bool,
}

impl ConnectionSidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = Self {
            focus_handle: cx.focus_handle(),
            connections: Connection::new_mock(),
            test_db_tables: Vec::new(),
            collapsed: false,
            test_db_expanded: true, // Start expanded
        };

        // Load test database tables asynchronously
        let db_service = DbService::global(cx).clone();
        let user_db = db_service.user_db_handle();

        let task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            // Give the database a moment to initialize
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            let db = user_db.read().await;
            println!(
                "Sidebar: Loading tables from database, connected: {}",
                db.is_connected()
            );

            match db.get_tables().await {
                Ok(tables) => {
                    println!("Sidebar: Loaded {} tables: {:?}", tables.len(), tables);
                    Ok(tables)
                }
                Err(e) => {
                    eprintln!("Sidebar: Failed to load tables: {}", e);
                    Err(anyhow::anyhow!("{}", e))
                }
            }
        });

        cx.spawn(async move |sidebar, cx| {
            if let Ok(tables) = task.await {
                println!("Sidebar: Updating UI with {} tables", tables.len());
                let _ = sidebar.update(cx, |sidebar, cx| {
                    sidebar.test_db_tables = tables;
                    cx.notify();
                });
            }
        })
        .detach();

        sidebar
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        cx.notify();
    }

}

impl Focusable for ConnectionSidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConnectionSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .h_full()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        Sidebar::new(Side::Left).collapsed(self.collapsed).child(
                            SidebarGroup::new("Databases").child(
                                SidebarMenu::new()
                                    // Test Database (actual connection)
                                    .child(
                                        SidebarMenuItem::new(SharedString::from("Test Database"))
                                            .icon(IconName::Building2)
                                            .active(self.test_db_expanded)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.test_db_expanded = !this.test_db_expanded;
                                                cx.notify();
                                            }))
                                            .children(self.test_db_tables.iter().map(|table| {
                                                SidebarMenuItem::new(SharedString::from(table.clone()))
                                                    .icon(IconName::SquareTerminal)
                                            })),
                                    ),
                            ),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .p_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("new-connection")
                            .w_full()
                            .outline()
                            .icon(IconName::Plus)
                            .label("New Connection")
                            .on_click(|_, _, cx| {
                                println!("DEBUG: New Connection button clicked, dispatching action");
                                cx.dispatch_action(&OpenNewConnectionModal);
                            }),
                    ),
            )
    }
}
