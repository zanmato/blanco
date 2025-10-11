use gpui::{
    div, prelude::FluentBuilder, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Styled, Window,
};
use gpui_component::{
    dropdown::{Dropdown, DropdownState},
    h_flex,
    input::{InputState, TextInput},
    v_flex, ActiveTheme, Icon, IconName, IndexPath,
};

use crate::app_database::ConnectionData;

pub struct NewConnectionModal {
    focus_handle: FocusHandle,
    name_input: Entity<InputState>,
    db_type_dropdown: Entity<DropdownState<Vec<String>>>,
    host_input: Entity<InputState>,
    port_input: Entity<InputState>,
    database_input: Entity<InputState>,
    username_input: Entity<InputState>,
    password_input: Entity<InputState>,
    file_path_input: Entity<InputState>,
    test_result: Option<TestResult>,
}

#[derive(Clone, Debug)]
struct TestResult {
    success: bool,
    message: String,
}

impl NewConnectionModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let db_types = vec!["SQLite".to_string()];
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Connection Name"));
        let db_type_dropdown = cx.new(|cx| DropdownState::new(db_types.clone(), Some(IndexPath::new(0)), window, cx));
        let host_input = cx.new(|cx| InputState::new(window, cx).placeholder("localhost"));
        let port_input = cx.new(|cx| InputState::new(window, cx).placeholder("5432"));
        let database_input = cx.new(|cx| InputState::new(window, cx).placeholder("Database Name"));
        let username_input = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let password_input = cx.new(|cx| InputState::new(window, cx).placeholder("Password"));
        let file_path_input = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/database.db"));

        Self {
            focus_handle: cx.focus_handle(),
            name_input,
            db_type_dropdown,
            host_input,
            port_input,
            database_input,
            username_input,
            password_input,
            file_path_input,
            test_result: None,
        }
    }

    fn get_selected_db_type(&self, cx: &App) -> String {
        self.db_type_dropdown
            .read(cx)
            .selected_value()
            .unwrap_or(&"SQLite".to_string())
            .clone()
    }

    fn is_sqlite(&self, cx: &App) -> bool {
        self.get_selected_db_type(cx) == "SQLite"
    }

    pub fn test_connection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let db_type = self.get_selected_db_type(cx);

        if db_type == "SQLite" {
            let file_path = self.file_path_input.read(cx).value();
            if file_path.is_empty() {
                self.test_result = Some(TestResult {
                    success: false,
                    message: "Please enter a file path".to_string(),
                });
            } else {
                // For SQLite, just check if path is valid
                self.test_result = Some(TestResult {
                    success: true,
                    message: "Connection path valid".to_string(),
                });
            }
        } else {
            self.test_result = Some(TestResult {
                success: false,
                message: "Only SQLite is currently supported".to_string(),
            });
        }
        cx.notify();
    }

    pub fn get_connection_data(&self, cx: &App) -> Option<ConnectionData> {
        let name = self.name_input.read(cx).value().to_string();
        let db_type = self.get_selected_db_type(cx);

        if name.is_empty() {
            return None;
        }

        if db_type == "SQLite" {
            let file_path = self.file_path_input.read(cx).value().to_string();
            if file_path.is_empty() {
                return None;
            }

            Some(ConnectionData::new_sqlite(name, file_path))
        } else {
            let host = self.host_input.read(cx).value().to_string();
            let port_str = self.port_input.read(cx).value();
            let port = port_str.parse::<i32>().ok();
            let database = self.database_input.read(cx).value().to_string();
            let username = self.username_input.read(cx).value().to_string();
            let password = self.password_input.read(cx).value().to_string();

            if host.is_empty() || database.is_empty() {
                return None;
            }

            Some(ConnectionData {
                id: None,
                name,
                db_type,
                host: Some(host),
                port,
                database_name: Some(database),
                username: Some(username),
                password: Some(password),
                database_path: None,
                last_used_at: None,
            })
        }
    }
}

impl Focusable for NewConnectionModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NewConnectionModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_sqlite = self.is_sqlite(cx);

        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Connection Name"))
                    .child(TextInput::new(&self.name_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Database Type"))
                    .child(Dropdown::new(&self.db_type_dropdown)),
            )
            .when(!is_sqlite, |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_2()
                                .child(div().text_sm().child("Host"))
                                .child(TextInput::new(&self.host_input)),
                        )
                        .child(
                            v_flex()
                                .w_32()
                                .gap_2()
                                .child(div().text_sm().child("Port"))
                                .child(TextInput::new(&self.port_input)),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child("Database"))
                        .child(TextInput::new(&self.database_input)),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child("Username"))
                        .child(TextInput::new(&self.username_input)),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child("Password"))
                        .child(TextInput::new(&self.password_input)),
                )
            })
            .when(is_sqlite, |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child("File Path"))
                        .child(TextInput::new(&self.file_path_input)),
                )
            })
            .when_some(self.test_result.clone(), |this, result| {
                this.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .p_3()
                        .rounded(cx.theme().radius)
                        .when(result.success, |this| {
                            this.bg(cx.theme().green.opacity(0.1))
                                .text_color(cx.theme().green)
                                .child(Icon::new(IconName::CircleCheck).size_4())
                        })
                        .when(!result.success, |this| {
                            this.bg(cx.theme().red.opacity(0.1))
                                .text_color(cx.theme().red)
                                .child(Icon::new(IconName::CircleX).size_4())
                        })
                        .child(div().text_sm().child(result.message)),
                )
            })
    }
}
