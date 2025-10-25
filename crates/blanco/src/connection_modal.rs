use gpui::{
    div, prelude::FluentBuilder, App, AppContext, Context, Entity, FocusHandle, Focusable,
    IntoElement, ParentElement, Render, Styled, Window,
};
use gpui_component::{
    dropdown::{Dropdown, DropdownState},
    h_flex,
    input::{InputState, TextInput},
    v_flex, ActiveTheme, Icon, IconName, IndexPath,
};

use crate::app_database::ConnectionData;

/// Represents a database connector type
#[derive(Clone, Debug, PartialEq)]
enum ConnectorType {
    SQLite,
    PostgreSQL,
}

impl ConnectorType {
    fn from_str(s: &str) -> Self {
        match s {
            "PostgreSQL" => ConnectorType::PostgreSQL,
            _ => ConnectorType::SQLite,
        }
    }
}

#[derive(Clone, Debug)]
struct TestResult {
    success: bool,
    message: String,
}

/// SQLite connector form
struct SqliteForm {
    file_path_input: Entity<InputState>,
}

impl SqliteForm {
    fn new(file_path_input: Entity<InputState>) -> Self {
        Self { file_path_input }
    }

    fn render(&self, _cx: &App) -> gpui::AnyElement {
        v_flex()
            .gap_2()
            .child(div().text_sm().child("File Path"))
            .child(TextInput::new(&self.file_path_input))
            .into_any_element()
    }

    fn get_connection_data(&self, name: String, cx: &App) -> Option<ConnectionData> {
        let file_path = self.file_path_input.read(cx).value().to_string();

        if file_path.is_empty() {
            return None;
        }

        Some(ConnectionData::new_sqlite(name, file_path))
    }

    fn test_connection(&self, cx: &App) -> TestResult {
        let file_path = self.file_path_input.read(cx).value();

        if file_path.is_empty() {
            return TestResult {
                success: false,
                message: "Please enter a file path".to_string(),
            };
        }

        // Check if the file exists or the parent directory exists (so we can create it)
        let file_path_str = file_path.to_string();
        let path = std::path::Path::new(&file_path_str);

        if path.exists() {
            TestResult {
                success: true,
                message: "Database file exists and is accessible".to_string(),
            }
        } else if let Some(parent) = path.parent() {
            if parent.exists() || parent.to_str() == Some("") {
                TestResult {
                    success: true,
                    message: "Database will be created at this location".to_string(),
                }
            } else {
                TestResult {
                    success: false,
                    message: format!("Parent directory does not exist: {}", parent.display()),
                }
            }
        } else {
            TestResult {
                success: false,
                message: "Invalid file path".to_string(),
            }
        }
    }
}

/// PostgreSQL connector form
struct PostgresForm {
    host_input: Entity<InputState>,
    port_input: Entity<InputState>,
    database_input: Entity<InputState>,
    username_input: Entity<InputState>,
    password_input: Entity<InputState>,
}

impl PostgresForm {
    fn new(
        host_input: Entity<InputState>,
        port_input: Entity<InputState>,
        database_input: Entity<InputState>,
        username_input: Entity<InputState>,
        password_input: Entity<InputState>,
    ) -> Self {
        Self {
            host_input,
            port_input,
            database_input,
            username_input,
            password_input,
        }
    }

    fn render(&self, _cx: &App) -> gpui::AnyElement {
        v_flex()
            .gap_4()
            .child(
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
            .into_any_element()
    }

    fn validate(&self, cx: &App) -> Option<String> {
        let host = self.host_input.read(cx).value();
        let port_str = self.port_input.read(cx).value();
        let database = self.database_input.read(cx).value();
        let username = self.username_input.read(cx).value();

        if host.is_empty() {
            return Some("Host is required".to_string());
        }

        if port_str.is_empty() {
            return Some("Port is required".to_string());
        }

        if port_str.parse::<u16>().is_err() {
            return Some("Port must be a valid number (1-65535)".to_string());
        }

        if database.is_empty() {
            return Some("Database name is required".to_string());
        }

        if username.is_empty() {
            return Some("Username is required".to_string());
        }

        None
    }

    fn get_connection_data(&self, name: String, cx: &App) -> Option<ConnectionData> {
        let host = self.host_input.read(cx).value().to_string();
        let port_str = self.port_input.read(cx).value();
        let database = self.database_input.read(cx).value().to_string();
        let username = self.username_input.read(cx).value().to_string();
        let password = self.password_input.read(cx).value().to_string();

        // Validate required fields
        if host.is_empty() || database.is_empty() || username.is_empty() {
            return None;
        }

        let port = port_str.parse::<i32>().ok()?;

        Some(ConnectionData::new_postgres(
            name, host, port, database, username, password,
        ))
    }

    fn test_connection(&self, cx: &App) -> TestResult {
        // Validate fields first
        if let Some(error) = self.validate(cx) {
            return TestResult {
                success: false,
                message: error,
            };
        }

        // Build connection string for testing
        let host = self.host_input.read(cx).value();
        let port = self.port_input.read(cx).value();
        let database = self.database_input.read(cx).value();
        let username = self.username_input.read(cx).value();
        let password = self.password_input.read(cx).value();

        let password_part = if password.is_empty() {
            String::new()
        } else {
            format!(":{}", password)
        };

        let _connection_string = format!(
            "postgresql://{}{}@{}:{}/{}",
            username, password_part, host, port, database
        );

        // For now, just return success if validation passed
        // In the future, we could actually test the connection here
        TestResult {
            success: true,
            message: "Connection parameters are valid".to_string(),
        }
    }

    #[allow(dead_code)]
    fn first_input_focus_handle(&self, cx: &App) -> FocusHandle {
        self.host_input.focus_handle(cx)
    }
}

pub struct NewConnectionModal {
    focus_handle: FocusHandle,
    name_input: Entity<InputState>,
    db_type_dropdown: Entity<DropdownState<Vec<String>>>,
    sqlite_form: SqliteForm,
    postgres_form: PostgresForm,
    test_result: Option<TestResult>,
}

impl NewConnectionModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let db_types = vec!["SQLite".to_string(), "PostgreSQL".to_string()];
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Connection Name"));
        let db_type_dropdown =
            cx.new(|cx| DropdownState::new(db_types.clone(), Some(IndexPath::new(0)), window, cx));

        // Create entities for SQLite form
        let sqlite_file_path =
            cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/database.db"));
        let sqlite_form = SqliteForm::new(sqlite_file_path);

        // Create entities for PostgreSQL form
        let pg_host = cx.new(|cx| InputState::new(window, cx).placeholder("localhost"));
        let pg_port = cx.new(|cx| InputState::new(window, cx).placeholder("5432"));
        let pg_database = cx.new(|cx| InputState::new(window, cx).placeholder("Database Name"));
        let pg_username = cx.new(|cx| InputState::new(window, cx).placeholder("postgres"));
        let pg_password = cx.new(|cx| InputState::new(window, cx).placeholder("Password"));
        let postgres_form =
            PostgresForm::new(pg_host, pg_port, pg_database, pg_username, pg_password);

        Self {
            focus_handle: cx.focus_handle(),
            name_input,
            db_type_dropdown,
            sqlite_form,
            postgres_form,
            test_result: None,
        }
    }

    fn get_selected_connector_type(&self, cx: &App) -> ConnectorType {
        let selected = self
            .db_type_dropdown
            .read(cx)
            .selected_value()
            .unwrap_or(&"SQLite".to_string())
            .clone();
        ConnectorType::from_str(&selected)
    }

    pub fn test_connection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let connector_type = self.get_selected_connector_type(cx);

        self.test_result = Some(match connector_type {
            ConnectorType::SQLite => self.sqlite_form.test_connection(cx),
            ConnectorType::PostgreSQL => self.postgres_form.test_connection(cx),
        });

        cx.notify();
    }

    pub fn get_connection_data(&self, cx: &App) -> Option<ConnectionData> {
        let name = self.name_input.read(cx).value().to_string();

        if name.is_empty() {
            return None;
        }

        let connector_type = self.get_selected_connector_type(cx);

        match connector_type {
            ConnectorType::SQLite => self.sqlite_form.get_connection_data(name, cx),
            ConnectorType::PostgreSQL => self.postgres_form.get_connection_data(name, cx),
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
        let connector_type = self.get_selected_connector_type(cx);

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
            // Render the appropriate form based on selected type
            .child(match connector_type {
                ConnectorType::SQLite => self.sqlite_form.render(cx),
                ConnectorType::PostgreSQL => self.postgres_form.render(cx),
            })
            // Test result display
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
