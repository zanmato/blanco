use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, IndexPath, StyledExt, h_flex,
    input::{Input, InputState},
    select::{Select, SelectState},
    switch::Switch,
    v_flex,
};

use crate::app_database::ConnectionData;
use blanco_core::connection_trait::DriverType;

/// Represents a database connector type
#[derive(Clone, Debug, PartialEq)]
enum ConnectorType {
    SQLite,
    PostgreSQL,
    MySQL,
}

impl ConnectorType {
    fn from_str(s: &str) -> Self {
        match s {
            "PostgreSQL" => ConnectorType::PostgreSQL,
            "MySQL" => ConnectorType::MySQL,
            _ => ConnectorType::SQLite,
        }
    }

    fn to_string(&self) -> &'static str {
        match self {
            ConnectorType::SQLite => "SQLite",
            ConnectorType::PostgreSQL => "PostgreSQL",
            ConnectorType::MySQL => "MySQL",
        }
    }
}

impl From<DriverType> for ConnectorType {
    fn from(driver_type: DriverType) -> Self {
        match driver_type {
            DriverType::SQLite => ConnectorType::SQLite,
            DriverType::PostgreSQL => ConnectorType::PostgreSQL,
            DriverType::MySQL => ConnectorType::MySQL,
        }
    }
}

impl From<ConnectorType> for DriverType {
    fn from(connector_type: ConnectorType) -> Self {
        match connector_type {
            ConnectorType::SQLite => DriverType::SQLite,
            ConnectorType::PostgreSQL => DriverType::PostgreSQL,
            ConnectorType::MySQL => DriverType::MySQL,
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
            .child(Input::new(&self.file_path_input))
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
    // SSH tunnel configuration
    ssh_enabled: bool,
    ssh_host_input: Entity<InputState>,
    ssh_port_input: Entity<InputState>,
    ssh_user_input: Entity<InputState>,
    ssh_password_input: Entity<InputState>,
    ssh_private_key_input: Entity<InputState>,
    ssh_private_key_password_input: Entity<InputState>,
}

impl PostgresForm {
    #[allow(clippy::too_many_arguments)]
    fn new(
        host_input: Entity<InputState>,
        port_input: Entity<InputState>,
        database_input: Entity<InputState>,
        username_input: Entity<InputState>,
        password_input: Entity<InputState>,
        ssh_host_input: Entity<InputState>,
        ssh_port_input: Entity<InputState>,
        ssh_user_input: Entity<InputState>,
        ssh_password_input: Entity<InputState>,
        ssh_private_key_input: Entity<InputState>,
        ssh_private_key_password_input: Entity<InputState>,
    ) -> Self {
        Self {
            host_input,
            port_input,
            database_input,
            username_input,
            password_input,
            ssh_enabled: false,
            ssh_host_input,
            ssh_port_input,
            ssh_user_input,
            ssh_password_input,
            ssh_private_key_input,
            ssh_private_key_password_input,
        }
    }

    fn render(&self, cx: &App) -> gpui::AnyElement {
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
                            .child(Input::new(&self.host_input)),
                    )
                    .child(
                        v_flex()
                            .w_32()
                            .gap_2()
                            .child(div().text_sm().child("Port"))
                            .child(Input::new(&self.port_input)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Database"))
                    .child(Input::new(&self.database_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Username"))
                    .child(Input::new(&self.username_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Password"))
                    .child(Input::new(&self.password_input)),
            )
            // SSH Tunnel Configuration Section
            .child(
                div().mt_4().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child("SSH Tunnel Configuration"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("(Optional - Connect through SSH bastion host)"),
                        ),
                ),
            )
            .when(self.ssh_enabled, |this| {
                this.child(
                    v_flex()
                        .gap_3()
                        .mt_2()
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_2()
                                        .child(div().text_sm().child("SSH Host"))
                                        .child(Input::new(&self.ssh_host_input)),
                                )
                                .child(
                                    v_flex()
                                        .w_32()
                                        .gap_2()
                                        .child(div().text_sm().child("SSH Port"))
                                        .child(Input::new(&self.ssh_port_input)),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(div().text_sm().child("SSH Username"))
                                .child(Input::new(&self.ssh_user_input)),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_2()
                                        .child(div().text_sm().child("SSH Password"))
                                        .child(Input::new(&self.ssh_password_input)),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_2()
                                        .child(div().text_sm().child("Private Key Path"))
                                        .child(Input::new(&self.ssh_private_key_input)),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(div().text_sm().child("Private Key Password"))
                                .child(Input::new(&self.ssh_private_key_password_input)),
                        ),
                )
            })
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

        if self.ssh_enabled {
            // SSH is enabled, collect SSH configuration
            let ssh_host = self.ssh_host_input.read(cx).value();
            let ssh_port_str = self.ssh_port_input.read(cx).value();
            let ssh_user = self.ssh_user_input.read(cx).value();
            let ssh_password = self.ssh_password_input.read(cx).value();
            let ssh_private_key_path = self.ssh_private_key_input.read(cx).value();
            let ssh_private_key_password = self.ssh_private_key_password_input.read(cx).value();

            // Validate required SSH fields
            if ssh_host.is_empty() || ssh_user.is_empty() {
                return None;
            }

            let ssh_port = if ssh_port_str.is_empty() {
                22
            } else {
                ssh_port_str.parse::<i32>().ok()?
            };

            let ssh_password = if ssh_password.is_empty() {
                None
            } else {
                Some(ssh_password.to_string())
            };

            let ssh_private_key_path = if ssh_private_key_path.is_empty() {
                None
            } else {
                Some(ssh_private_key_path.to_string())
            };

            let ssh_private_key_password = if ssh_private_key_password.is_empty() {
                None
            } else {
                Some(ssh_private_key_password.to_string())
            };

            Some(ConnectionData::new_postgres_with_ssh(
                name,
                host,
                port,
                database,
                username,
                password,
                ssh_host.to_string(),
                ssh_port,
                ssh_user.to_string(),
                ssh_password,
                ssh_private_key_path,
                ssh_private_key_password,
            ))
        } else {
            // SSH is disabled, create regular PostgreSQL connection
            Some(ConnectionData::new_postgres(
                name, host, port, database, username, password,
            ))
        }
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

/// MySQL connector form
struct MysqlForm {
    host_input: Entity<InputState>,
    port_input: Entity<InputState>,
    database_input: Entity<InputState>,
    username_input: Entity<InputState>,
    password_input: Entity<InputState>,
    // SSH tunnel configuration
    ssh_enabled: bool,
    ssh_host_input: Entity<InputState>,
    ssh_port_input: Entity<InputState>,
    ssh_user_input: Entity<InputState>,
    ssh_password_input: Entity<InputState>,
    ssh_private_key_input: Entity<InputState>,
    ssh_private_key_password_input: Entity<InputState>,
}

impl MysqlForm {
    #[allow(clippy::too_many_arguments)]
    fn new(
        host_input: Entity<InputState>,
        port_input: Entity<InputState>,
        database_input: Entity<InputState>,
        username_input: Entity<InputState>,
        password_input: Entity<InputState>,
        ssh_host_input: Entity<InputState>,
        ssh_port_input: Entity<InputState>,
        ssh_user_input: Entity<InputState>,
        ssh_password_input: Entity<InputState>,
        ssh_private_key_input: Entity<InputState>,
        ssh_private_key_password_input: Entity<InputState>,
    ) -> Self {
        Self {
            host_input,
            port_input,
            database_input,
            username_input,
            password_input,
            ssh_enabled: false,
            ssh_host_input,
            ssh_port_input,
            ssh_user_input,
            ssh_password_input,
            ssh_private_key_input,
            ssh_private_key_password_input,
        }
    }

    fn render(&self, cx: &App) -> gpui::AnyElement {
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
                            .child(Input::new(&self.host_input)),
                    )
                    .child(
                        v_flex()
                            .w_32()
                            .gap_2()
                            .child(div().text_sm().child("Port"))
                            .child(Input::new(&self.port_input)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Database"))
                    .child(Input::new(&self.database_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Username"))
                    .child(Input::new(&self.username_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Password"))
                    .child(Input::new(&self.password_input)),
            )
            // SSH Tunnel Configuration Section
            .child(
                div().mt_4().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child("SSH Tunnel Configuration"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("(Optional)"),
                        ),
                ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("SSH Host"))
                    .child(Input::new(&self.ssh_host_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("SSH Port"))
                    .child(Input::new(&self.ssh_port_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("SSH User"))
                    .child(Input::new(&self.ssh_user_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("SSH Password"))
                    .child(Input::new(&self.ssh_password_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("SSH Private Key"))
                    .child(Input::new(&self.ssh_private_key_input)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Private Key Password"))
                    .child(Input::new(&self.ssh_private_key_password_input)),
            )
            .into_any_element()
    }

    fn build_connection_data(&self, name: &str, cx: &App) -> Option<ConnectionData> {
        let host = self.host_input.read(cx).value();
        let port = self.port_input.read(cx).value();
        let database = self.database_input.read(cx).value();
        let username = self.username_input.read(cx).value();
        let password = self.password_input.read(cx).value();

        if host.is_empty() || port.is_empty() || database.is_empty() || username.is_empty() {
            return None;
        }

        Some(ConnectionData::new_mysql(
            name.to_string(),
            host.to_string(),
            port.parse().unwrap_or(3306),
            database.to_string(),
            username.to_string(),
            password.to_string(),
        ))
    }

    fn validate(&self, cx: &App) -> Option<String> {
        let host = self.host_input.read(cx).value();
        let port = self.port_input.read(cx).value();
        let database = self.database_input.read(cx).value();
        let username = self.username_input.read(cx).value();

        if host.is_empty() {
            return Some("Host is required".to_string());
        }

        if port.is_empty() {
            return Some("Port is required".to_string());
        }

        if port.parse::<u16>().is_err() {
            return Some("Port must be a valid number".to_string());
        }

        if database.is_empty() {
            return Some("Database name is required".to_string());
        }

        if username.is_empty() {
            return Some("Username is required".to_string());
        }

        None
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
            "mysql://{}{}@{}:{}/{}",
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
    db_type_select: Entity<SelectState<Vec<String>>>,
    sqlite_form: SqliteForm,
    postgres_form: PostgresForm,
    mysql_form: MysqlForm,
    test_result: Option<TestResult>,
}

impl NewConnectionModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let db_types = vec![
            "SQLite".to_string(),
            "PostgreSQL".to_string(),
            "MySQL".to_string(),
        ];
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Connection Name"));
        let db_type_select =
            cx.new(|cx| SelectState::new(db_types.clone(), Some(IndexPath::new(0)), window, cx));

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

        // Create SSH tunnel input entities
        let ssh_host = cx.new(|cx| InputState::new(window, cx).placeholder("SSH Host"));
        let ssh_port = cx.new(|cx| InputState::new(window, cx).placeholder("22"));
        let ssh_user = cx.new(|cx| InputState::new(window, cx).placeholder("SSH Username"));
        let ssh_password =
            cx.new(|cx| InputState::new(window, cx).placeholder("SSH Password (optional)"));
        let ssh_private_key =
            cx.new(|cx| InputState::new(window, cx).placeholder("Private Key Path (optional)"));
        let ssh_private_key_password =
            cx.new(|cx| InputState::new(window, cx).placeholder("Private Key Password (optional)"));

        let postgres_form = PostgresForm::new(
            pg_host,
            pg_port,
            pg_database,
            pg_username,
            pg_password,
            ssh_host,
            ssh_port,
            ssh_user,
            ssh_password,
            ssh_private_key,
            ssh_private_key_password,
        );

        // Create entities for MySQL form
        let mysql_host = cx.new(|cx| InputState::new(window, cx).placeholder("localhost"));
        let mysql_port = cx.new(|cx| InputState::new(window, cx).placeholder("3306"));
        let mysql_database = cx.new(|cx| InputState::new(window, cx).placeholder("Database Name"));
        let mysql_username = cx.new(|cx| InputState::new(window, cx).placeholder("root"));
        let mysql_password = cx.new(|cx| InputState::new(window, cx).placeholder("Password"));

        // Create separate SSH tunnel input entities for MySQL
        let mysql_ssh_host = cx.new(|cx| InputState::new(window, cx).placeholder("SSH Host"));
        let mysql_ssh_port = cx.new(|cx| InputState::new(window, cx).placeholder("22"));
        let mysql_ssh_user = cx.new(|cx| InputState::new(window, cx).placeholder("SSH Username"));
        let mysql_ssh_password =
            cx.new(|cx| InputState::new(window, cx).placeholder("SSH Password (optional)"));
        let mysql_ssh_private_key =
            cx.new(|cx| InputState::new(window, cx).placeholder("Private Key Path (optional)"));
        let mysql_ssh_private_key_password =
            cx.new(|cx| InputState::new(window, cx).placeholder("Private Key Password (optional)"));

        let mysql_form = MysqlForm::new(
            mysql_host,
            mysql_port,
            mysql_database,
            mysql_username,
            mysql_password,
            mysql_ssh_host,
            mysql_ssh_port,
            mysql_ssh_user,
            mysql_ssh_password,
            mysql_ssh_private_key,
            mysql_ssh_private_key_password,
        );

        Self {
            focus_handle: cx.focus_handle(),
            name_input,
            db_type_select,
            sqlite_form,
            postgres_form,
            mysql_form,
            test_result: None,
        }
    }

    fn get_selected_connector_type(&self, cx: &App) -> ConnectorType {
        let selected = self
            .db_type_select
            .read(cx)
            .selected_value()
            .unwrap_or(&"SQLite".to_string())
            .clone();
        ConnectorType::from_str(&selected)
    }

    fn toggle_ssh_enabled(&mut self, cx: &mut Context<Self>) {
        self.postgres_form.ssh_enabled = !self.postgres_form.ssh_enabled;
        cx.notify();
    }

    fn toggle_mysql_ssh_enabled(&mut self, cx: &mut Context<Self>) {
        self.mysql_form.ssh_enabled = !self.mysql_form.ssh_enabled;
        cx.notify();
    }

    pub fn test_connection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let connector_type = self.get_selected_connector_type(cx);

        self.test_result = Some(match connector_type {
            ConnectorType::SQLite => self.sqlite_form.test_connection(cx),
            ConnectorType::PostgreSQL => self.postgres_form.test_connection(cx),
            ConnectorType::MySQL => self.mysql_form.test_connection(cx),
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
            ConnectorType::MySQL => self.mysql_form.build_connection_data(&name, cx),
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

        v_flex().gap_4().child(
            v_flex()
                .gap_4()
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child("Connection Name"))
                        .child(Input::new(&self.name_input)),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child("Database Type"))
                        .child(Select::new(&self.db_type_select)),
                )
                // Render the appropriate form based on selected type
                .child(match connector_type {
                    ConnectorType::SQLite => self.sqlite_form.render(cx),
                    ConnectorType::PostgreSQL => {
                        let form_elements = self.postgres_form.render(cx);
                        v_flex()
                            .gap_4()
                            // SSH Switch Section - above the form
                            .child(
                                div().child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            Switch::new("ssh-enabled-switch")
                                                .checked(self.postgres_form.ssh_enabled)
                                                .label("Enable SSH Tunnel")
                                                .on_click(cx.listener(
                                                    |modal: &mut Self, _checked, _window, cx| {
                                                        modal.toggle_ssh_enabled(cx);
                                                    },
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("(Connect through SSH bastion host)"),
                                        ),
                                ),
                            )
                            .child(form_elements) // Form renders SSH fields when enabled
                            .into_any_element()
                    }
                    ConnectorType::MySQL => {
                        let form_elements = self.mysql_form.render(cx);
                        v_flex()
                            .gap_4()
                            // SSH Switch Section - above the form
                            .child(
                                div().child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            Switch::new("mysql-ssh-enabled-switch")
                                                .checked(self.mysql_form.ssh_enabled)
                                                .label("Enable SSH Tunnel")
                                                .on_click(cx.listener(
                                                    |modal: &mut Self, _checked, _window, cx| {
                                                        modal.toggle_mysql_ssh_enabled(cx);
                                                    },
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("(Connect through SSH bastion host)"),
                                        ),
                                ),
                            )
                            .child(form_elements) // Form renders SSH fields when enabled
                            .into_any_element()
                    }
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
                }),
        )
    }
}
