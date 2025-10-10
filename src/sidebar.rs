use gpui::{
    App, Context, FocusHandle, Focusable, IntoElement, Render, Window,
};
use gpui_component::{
    sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem},
    IconName, Side,
};

use crate::connection::Connection;

pub struct ConnectionSidebar {
    focus_handle: FocusHandle,
    connections: Vec<Connection>,
    collapsed: bool,
}

impl ConnectionSidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            connections: Connection::new_mock(),
            collapsed: false,
        }
    }
}

impl Focusable for ConnectionSidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConnectionSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Sidebar::new(Side::Left)
            .collapsed(self.collapsed)
            .child(
                SidebarGroup::new("Databases").child(
                    SidebarMenu::new().children(
                        self.connections
                            .iter()
                            .map(|conn| {
                                SidebarMenuItem::new(conn.name.clone())
                                    .icon(IconName::Globe)
                                    .children(
                                        conn.schemas.iter().map(|schema| {
                                            SidebarMenuItem::new(schema.name.clone())
                                                .icon(IconName::Folder)
                                                .children(
                                                    schema.tables.iter().map(|table| {
                                                        SidebarMenuItem::new(table.name.clone())
                                                            .icon(IconName::SquareTerminal)
                                                    })
                                                )
                                        })
                                    )
                            })
                    )
                )
            )
    }
}
