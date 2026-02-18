use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, IndexPath, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    select::{Select, SelectState},
    switch::Switch,
    v_flex,
};

use crate::app_database::{ConnectionData, EnvironmentType};
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

    fn get_connection_data(
        &self,
        name: String,
        environment_type: EnvironmentType,
        cx: &App,
    ) -> Option<ConnectionData> {
        let file_path = self.file_path_input.read(cx).value().to_string();

        if file_path.is_empty() {
            return None;
        }

        let mut connection = ConnectionData::new_sqlite(name, file_path);
        connection.environment_type = environment_type;
        Some(connection)
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
    // SSL/TLS configuration
    ssl_mode_select: Entity<SelectState<Vec<String>>>,
    ssl_key_input: Entity<InputState>,
    ssl_cert_input: Entity<InputState>,
    ssl_ca_cert_input: Entity<InputState>,
    ssl_advanced_expanded: bool,
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
        ssl_mode_select: Entity<SelectState<Vec<String>>>,
        ssl_key_input: Entity<InputState>,
        ssl_cert_input: Entity<InputState>,
        ssl_ca_cert_input: Entity<InputState>,
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
            ssl_mode_select,
            ssl_key_input,
            ssl_cert_input,
            ssl_ca_cert_input,
            ssl_advanced_expanded: false,
        }
    }

    fn render(&self, _cx: &App) -> gpui::AnyElement {
        v_flex()
            .gap_3()
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
                            .flex_1()
                            .gap_2()
                            .child(div().text_sm().child("Port"))
                            .child(Input::new(&self.port_input)),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(div().text_sm().child("User"))
                            .child(Input::new(&self.username_input)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(div().text_sm().child("Password"))
                            .child(Input::new(&self.password_input)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Database name"))
                    .child(Input::new(&self.database_input)),
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

    fn get_connection_data(
        &self,
        name: String,
        environment_type: EnvironmentType,
        cx: &App,
    ) -> Option<ConnectionData> {
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

        // Collect SSL configuration
        let ssl_mode = self.ssl_mode_select.read(cx).selected_value().cloned();
        let ssl_key_path = self.ssl_key_input.read(cx).value();
        let ssl_key_path = if ssl_key_path.is_empty() {
            None
        } else {
            Some(ssl_key_path.to_string())
        };
        let ssl_cert_path = self.ssl_cert_input.read(cx).value();
        let ssl_cert_path = if ssl_cert_path.is_empty() {
            None
        } else {
            Some(ssl_cert_path.to_string())
        };
        let ssl_ca_cert_path = self.ssl_ca_cert_input.read(cx).value();
        let ssl_ca_cert_path = if ssl_ca_cert_path.is_empty() {
            None
        } else {
            Some(ssl_ca_cert_path.to_string())
        };

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

            {
                let mut connection = ConnectionData::new_postgres_with_ssh(
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
                );
                connection.environment_type = environment_type;
                connection.ssl_mode = ssl_mode;
                connection.ssl_key_path = ssl_key_path;
                connection.ssl_cert_path = ssl_cert_path;
                connection.ssl_ca_cert_path = ssl_ca_cert_path;
                Some(connection)
            }
        } else {
            // SSH is disabled, create regular PostgreSQL connection
            {
                let mut connection =
                    ConnectionData::new_postgres(name, host, port, database, username, password);
                connection.environment_type = environment_type;
                connection.ssl_mode = ssl_mode;
                connection.ssl_key_path = ssl_key_path;
                connection.ssl_cert_path = ssl_cert_path;
                connection.ssl_ca_cert_path = ssl_ca_cert_path;
                Some(connection)
            }
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
    // SSL/TLS configuration
    ssl_mode_select: Entity<SelectState<Vec<String>>>,
    ssl_key_input: Entity<InputState>,
    ssl_cert_input: Entity<InputState>,
    ssl_ca_cert_input: Entity<InputState>,
    ssl_advanced_expanded: bool,
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
        ssl_mode_select: Entity<SelectState<Vec<String>>>,
        ssl_key_input: Entity<InputState>,
        ssl_cert_input: Entity<InputState>,
        ssl_ca_cert_input: Entity<InputState>,
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
            ssl_mode_select,
            ssl_key_input,
            ssl_cert_input,
            ssl_ca_cert_input,
            ssl_advanced_expanded: false,
        }
    }

    fn render(&self, _cx: &App) -> gpui::AnyElement {
        v_flex()
            .gap_3()
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
                            .flex_1()
                            .gap_2()
                            .child(div().text_sm().child("Port"))
                            .child(Input::new(&self.port_input)),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(div().text_sm().child("User"))
                            .child(Input::new(&self.username_input)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(div().text_sm().child("Password"))
                            .child(Input::new(&self.password_input)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Database name"))
                    .child(Input::new(&self.database_input)),
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
    environment_type_select: Entity<SelectState<Vec<String>>>,
    sqlite_form: SqliteForm,
    postgres_form: PostgresForm,
    mysql_form: MysqlForm,
    test_result: Option<TestResult>,
    editing_connection_id: Option<i64>,
    db_type_locked: bool,
}

impl NewConnectionModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_connection_data(window, cx, None)
    }

    pub fn with_connection_data(
        window: &mut Window,
        cx: &mut Context<Self>,
        connection_data: Option<ConnectionData>,
    ) -> Self {
        let db_types = vec![
            "SQLite".to_string(),
            "PostgreSQL".to_string(),
            "MySQL".to_string(),
        ];

        // Determine initial values and editing mode
        let (initial_name, initial_db_type, editing_connection_id, db_type_locked) =
            if let Some(conn) = &connection_data {
                let conn_id = conn.id;
                let db_type_index = match conn.db_type.as_str() {
                    "PostgreSQL" => 1,
                    "MySQL" => 2,
                    _ => 0,
                };
                (conn.name.clone(), Some(db_type_index), conn_id, true)
            } else {
                (String::new(), Some(0), None, false)
            };

        let name_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Connection Name");
            if !initial_name.is_empty() {
                input.set_value(initial_name, window, cx);
            }
            input
        });

        let db_type_select = cx.new(|cx| {
            SelectState::new(db_types.clone(), initial_db_type.map(IndexPath::new), window, cx)
        });

        // Environment type selector
        let environment_types = vec!["DEV".to_string(), "TEST".to_string(), "PROD".to_string()];
        let initial_env_index = connection_data
            .as_ref()
            .map(|c| match c.environment_type {
                crate::app_database::EnvironmentType::Test => 1,
                crate::app_database::EnvironmentType::Prod => 2,
                _ => 0,
            })
            .or(Some(0));
        let environment_type_select = cx.new(|cx| {
            SelectState::new(environment_types, initial_env_index.map(IndexPath::new), window, cx)
        });

        // Create entities for SQLite form
        let sqlite_file_path = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("/path/to/database.db");
            if let Some(conn) = &connection_data {
                if let Some(path) = &conn.database_path {
                    input.set_value(path.clone(), window, cx);
                }
            }
            input
        });
        let sqlite_form = SqliteForm::new(sqlite_file_path);

        // Create entities for PostgreSQL form
        let pg_host = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("localhost");
            if let Some(conn) = &connection_data {
                if let Some(host) = &conn.host {
                    input.set_value(host.clone(), window, cx);
                }
            }
            input
        });
        let pg_port = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("5432");
            if let Some(conn) = &connection_data {
                if let Some(port) = conn.port {
                    input.set_value(port.to_string(), window, cx);
                }
            }
            input
        });
        let pg_database = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Database Name");
            if let Some(conn) = &connection_data {
                if let Some(db) = &conn.database_name {
                    input.set_value(db.clone(), window, cx);
                }
            }
            input
        });
        let pg_username = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("postgres");
            if let Some(conn) = &connection_data {
                if let Some(user) = &conn.username {
                    input.set_value(user.clone(), window, cx);
                }
            }
            input
        });
        let pg_password = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Password");
            if let Some(conn) = &connection_data {
                if let Some(pass) = &conn.password {
                    input.set_value(pass.clone(), window, cx);
                }
            }
            input
        });

        // Create SSH tunnel input entities
        let ssh_host = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSH Host");
            if let Some(conn) = &connection_data {
                if let Some(host) = &conn.ssh_host {
                    input.set_value(host.clone(), window, cx);
                }
            }
            input
        });
        let ssh_port = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("22");
            if let Some(conn) = &connection_data {
                if let Some(port) = conn.ssh_port {
                    input.set_value(port.to_string(), window, cx);
                }
            }
            input
        });
        let ssh_user = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSH Username");
            if let Some(conn) = &connection_data {
                if let Some(user) = &conn.ssh_user {
                    input.set_value(user.clone(), window, cx);
                }
            }
            input
        });
        let ssh_password = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSH Password (optional)");
            if let Some(conn) = &connection_data {
                if let Some(pass) = &conn.ssh_password {
                    input.set_value(pass.clone(), window, cx);
                }
            }
            input
        });
        let ssh_private_key = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Private Key Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(key) = &conn.ssh_private_key_path {
                    input.set_value(key.clone(), window, cx);
                }
            }
            input
        });
        let ssh_private_key_password = cx.new(|cx| {
            let mut input =
                InputState::new(window, cx).placeholder("Private Key Password (optional)");
            if let Some(conn) = &connection_data {
                if let Some(pass) = &conn.ssh_private_key_password {
                    input.set_value(pass.clone(), window, cx);
                }
            }
            input
        });

        // Create SSL input entities for PostgreSQL
        let ssl_modes = vec![
            "preferred".to_string(),
            "required".to_string(),
            "disabled".to_string(),
            "allow".to_string(),
            "verify-ca".to_string(),
            "verify-full".to_string(),
        ];
        let initial_ssl_index = connection_data
            .as_ref()
            .and_then(|c| c.ssl_mode.as_ref())
            .and_then(|mode| ssl_modes.iter().position(|m| m == mode));
        let pg_ssl_mode_select = cx.new(|cx| {
            SelectState::new(ssl_modes.clone(), initial_ssl_index.map(IndexPath::new), window, cx)
        });
        let pg_ssl_key = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSL Key Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(path) = &conn.ssl_key_path {
                    input.set_value(path.clone(), window, cx);
                }
            }
            input
        });
        let pg_ssl_cert = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSL Cert Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(path) = &conn.ssl_cert_path {
                    input.set_value(path.clone(), window, cx);
                }
            }
            input
        });
        let pg_ssl_ca_cert = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSL CA Cert Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(path) = &conn.ssl_ca_cert_path {
                    input.set_value(path.clone(), window, cx);
                }
            }
            input
        });

        let mut postgres_form = PostgresForm::new(
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
            pg_ssl_mode_select,
            pg_ssl_key,
            pg_ssl_cert,
            pg_ssl_ca_cert,
        );

        // Set SSH enabled state if connection has SSH config
        if let Some(conn) = &connection_data {
            if conn.uses_ssh_tunnel() {
                postgres_form.ssh_enabled = true;
            }
        }

        // Create entities for MySQL form
        let mysql_host = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("localhost");
            if let Some(conn) = &connection_data {
                if let Some(host) = &conn.host {
                    input.set_value(host.clone(), window, cx);
                }
            }
            input
        });
        let mysql_port = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("3306");
            if let Some(conn) = &connection_data {
                if let Some(port) = conn.port {
                    input.set_value(port.to_string(), window, cx);
                }
            }
            input
        });
        let mysql_database = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Database Name");
            if let Some(conn) = &connection_data {
                if let Some(db) = &conn.database_name {
                    input.set_value(db.clone(), window, cx);
                }
            }
            input
        });
        let mysql_username = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("root");
            if let Some(conn) = &connection_data {
                if let Some(user) = &conn.username {
                    input.set_value(user.clone(), window, cx);
                }
            }
            input
        });
        let mysql_password = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Password");
            if let Some(conn) = &connection_data {
                if let Some(pass) = &conn.password {
                    input.set_value(pass.clone(), window, cx);
                }
            }
            input
        });

        // Create separate SSH tunnel input entities for MySQL
        let mysql_ssh_host = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSH Host");
            if let Some(conn) = &connection_data {
                if let Some(host) = &conn.ssh_host {
                    input.set_value(host.clone(), window, cx);
                }
            }
            input
        });
        let mysql_ssh_port = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("22");
            if let Some(conn) = &connection_data {
                if let Some(port) = conn.ssh_port {
                    input.set_value(port.to_string(), window, cx);
                }
            }
            input
        });
        let mysql_ssh_user = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSH Username");
            if let Some(conn) = &connection_data {
                if let Some(user) = &conn.ssh_user {
                    input.set_value(user.clone(), window, cx);
                }
            }
            input
        });
        let mysql_ssh_password = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSH Password (optional)");
            if let Some(conn) = &connection_data {
                if let Some(pass) = &conn.ssh_password {
                    input.set_value(pass.clone(), window, cx);
                }
            }
            input
        });
        let mysql_ssh_private_key = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Private Key Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(key) = &conn.ssh_private_key_path {
                    input.set_value(key.clone(), window, cx);
                }
            }
            input
        });
        let mysql_ssh_private_key_password = cx.new(|cx| {
            let mut input =
                InputState::new(window, cx).placeholder("Private Key Password (optional)");
            if let Some(conn) = &connection_data {
                if let Some(pass) = &conn.ssh_private_key_password {
                    input.set_value(pass.clone(), window, cx);
                }
            }
            input
        });

        // Create SSL input entities for MySQL (reusing same ssl_modes list)
        let mysql_ssl_mode_select = cx.new(|cx| {
            SelectState::new(
                vec![
                    "preferred".to_string(),
                    "required".to_string(),
                    "disabled".to_string(),
                    "verify-ca".to_string(),
                    "verify-full".to_string(),
                ],
                initial_ssl_index.map(IndexPath::new),
                window,
                cx,
            )
        });
        let mysql_ssl_key = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSL Key Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(path) = &conn.ssl_key_path {
                    input.set_value(path.clone(), window, cx);
                }
            }
            input
        });
        let mysql_ssl_cert = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSL Cert Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(path) = &conn.ssl_cert_path {
                    input.set_value(path.clone(), window, cx);
                }
            }
            input
        });
        let mysql_ssl_ca_cert = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("SSL CA Cert Path (optional)");
            if let Some(conn) = &connection_data {
                if let Some(path) = &conn.ssl_ca_cert_path {
                    input.set_value(path.clone(), window, cx);
                }
            }
            input
        });

        let mut mysql_form = MysqlForm::new(
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
            mysql_ssl_mode_select,
            mysql_ssl_key,
            mysql_ssl_cert,
            mysql_ssl_ca_cert,
        );

        // Set SSH enabled state if connection has SSH config
        if let Some(conn) = &connection_data {
            if conn.uses_ssh_tunnel() {
                mysql_form.ssh_enabled = true;
            }
        }

        Self {
            focus_handle: cx.focus_handle(),
            name_input,
            db_type_select,
            environment_type_select,
            sqlite_form,
            postgres_form,
            mysql_form,
            test_result: None,
            editing_connection_id,
            db_type_locked,
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

    fn get_selected_environment_type(&self, cx: &App) -> EnvironmentType {
        let selected = self
            .environment_type_select
            .read(cx)
            .selected_value()
            .unwrap_or(&"DEV".to_string())
            .clone();
        match selected.as_str() {
            "TEST" => EnvironmentType::Test,
            "PROD" => EnvironmentType::Prod,
            _ => EnvironmentType::Dev,
        }
    }

    fn toggle_ssh_enabled(&mut self, cx: &mut Context<Self>) {
        self.postgres_form.ssh_enabled = !self.postgres_form.ssh_enabled;
        cx.notify();
    }

    fn toggle_mysql_ssh_enabled(&mut self, cx: &mut Context<Self>) {
        self.mysql_form.ssh_enabled = !self.mysql_form.ssh_enabled;
        cx.notify();
    }

    fn toggle_postgres_ssl_advanced(&mut self, cx: &mut Context<Self>) {
        self.postgres_form.ssl_advanced_expanded = !self.postgres_form.ssl_advanced_expanded;
        cx.notify();
    }

    fn toggle_mysql_ssl_advanced(&mut self, cx: &mut Context<Self>) {
        self.mysql_form.ssl_advanced_expanded = !self.mysql_form.ssl_advanced_expanded;
        cx.notify();
    }

    fn pick_postgres_ssh_private_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let private_key_input = self.postgres_form.ssh_private_key_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSH private key file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        private_key_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_mysql_ssh_private_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let private_key_input = self.mysql_form.ssh_private_key_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSH private key file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        private_key_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_postgres_ssl_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ssl_key_input = self.postgres_form.ssl_key_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSL key file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        ssl_key_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_postgres_ssl_cert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ssl_cert_input = self.postgres_form.ssl_cert_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSL certificate file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        ssl_cert_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_postgres_ssl_ca_cert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ssl_ca_cert_input = self.postgres_form.ssl_ca_cert_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSL CA certificate file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        ssl_ca_cert_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_mysql_ssl_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ssl_key_input = self.mysql_form.ssl_key_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSL key file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        ssl_key_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_mysql_ssl_cert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ssl_cert_input = self.mysql_form.ssl_cert_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSL certificate file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        ssl_cert_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_mysql_ssl_ca_cert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ssl_ca_cert_input = self.mysql_form.ssl_ca_cert_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SSL CA certificate file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        ssl_ca_cert_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
    }

    fn pick_sqlite_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file_path_input = self.sqlite_form.file_path_input.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select SQLite database file".into()),
        });

        cx.spawn_in(window, async move |_, window| {
            if let Some(paths) = paths.await.ok()?.ok()?
                && let Some(path) = paths.first()
            {
                let path_str = path.to_str()?.to_string();
                window
                    .update(|window, cx| {
                        file_path_input.update(cx, |input, cx| {
                            input.set_value(path_str, window, cx);
                        });
                    })
                    .ok();
            }

            Some(())
        })
        .detach();
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
        let environment_type = self.get_selected_environment_type(cx);

        let mut connection = match connector_type {
            ConnectorType::SQLite => {
                self.sqlite_form
                    .get_connection_data(name, environment_type, cx)?
            }
            ConnectorType::PostgreSQL => {
                self.postgres_form
                    .get_connection_data(name, environment_type, cx)?
            }
            ConnectorType::MySQL => {
                let mut conn = self.mysql_form.build_connection_data(&name, cx)?;
                conn.environment_type = environment_type;
                conn
            }
        };

        // Preserve the connection ID when editing
        if let Some(editing_id) = self.editing_connection_id {
            connection.id = Some(editing_id);
        }

        Some(connection)
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
                    h_flex()
                        .gap_3()
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_2()
                                .child(div().text_sm().child("Type"))
                                .child(Select::new(&self.db_type_select).disabled(self.db_type_locked)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_2()
                                .child(div().text_sm().child("Environment"))
                                .child(Select::new(&self.environment_type_select)),
                        ),
                )
                // Render the appropriate form based on selected type
                .child(match connector_type {
                    ConnectorType::SQLite => {
                        v_flex()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(div().text_sm().child("Database path"))
                                    .child(
                                        Input::new(&self.sqlite_form.file_path_input).suffix(
                                            Button::new("sqlite-file-picker")
                                                .ghost()
                                                .icon(IconName::Folder)
                                                .xsmall()
                                                .on_click(cx.listener(
                                                    |modal: &mut Self, _event, window, cx| {
                                                        modal.pick_sqlite_file(window, cx);
                                                    },
                                                )),
                                        ),
                                    ),
                            )
                            .into_any_element()
                    }
                    ConnectorType::PostgreSQL => {
                        let base_fields = self.postgres_form.render(cx);
                        v_flex()
                            .gap_3()
                            .child(base_fields)
                            // SSL Mode selector
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_2()
                                    .child(div().text_sm().child("SSL Mode"))
                                    .child(Select::new(&self.postgres_form.ssl_mode_select)),
                            )
                            // Advanced SSL section toggle
                            .child(
                                Button::new("postgres-ssl-advanced-toggle")
                                    .ghost()
                                    .xsmall()
                                    .child(if self.postgres_form.ssl_advanced_expanded {
                                        "Hide Advanced SSL"
                                    } else {
                                        "Show Advanced SSL"
                                    })
                                    .icon(if self.postgres_form.ssl_advanced_expanded {
                                        IconName::ChevronUp
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .on_click(cx.listener(
                                        |modal: &mut Self, _event, _window, cx| {
                                            modal.toggle_postgres_ssl_advanced(cx);
                                        },
                                    )),
                            )
                            // Advanced SSL fields
                            .when(self.postgres_form.ssl_advanced_expanded, |this| {
                                this.child(
                                    v_flex()
                                        .gap_3()
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(div().text_sm().child("SSL Key Path"))
                                                .child(
                                                    Input::new(&self.postgres_form.ssl_key_input).suffix(
                                                        Button::new("postgres-ssl-key-picker")
                                                            .ghost()
                                                            .icon(IconName::Folder)
                                                            .xsmall()
                                                            .on_click(cx.listener(
                                                                |modal: &mut Self, _event, window, cx| {
                                                                    modal.pick_postgres_ssl_key(window, cx);
                                                                },
                                                            )),
                                                    ),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(div().text_sm().child("SSL Cert Path"))
                                                .child(
                                                    Input::new(&self.postgres_form.ssl_cert_input).suffix(
                                                        Button::new("postgres-ssl-cert-picker")
                                                            .ghost()
                                                            .icon(IconName::Folder)
                                                            .xsmall()
                                                            .on_click(cx.listener(
                                                                |modal: &mut Self, _event, window, cx| {
                                                                    modal.pick_postgres_ssl_cert(window, cx);
                                                                },
                                                            )),
                                                    ),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(div().text_sm().child("SSL CA Cert Path"))
                                                .child(
                                                    Input::new(&self.postgres_form.ssl_ca_cert_input).suffix(
                                                        Button::new("postgres-ssl-ca-cert-picker")
                                                            .ghost()
                                                            .icon(IconName::Folder)
                                                            .xsmall()
                                                            .on_click(cx.listener(
                                                                |modal: &mut Self, _event, window, cx| {
                                                                    modal.pick_postgres_ssl_ca_cert(window, cx);
                                                                },
                                                            )),
                                                    ),
                                                ),
                                        ),
                                )
                            })
                            .child(
                                div().child(
                                    h_flex().gap_2().items_center().child(
                                        Switch::new("ssh-enabled-switch")
                                            .checked(self.postgres_form.ssh_enabled)
                                            .label("SSH")
                                            .on_click(cx.listener(
                                                |modal: &mut Self, _checked, _window, cx| {
                                                    modal.toggle_ssh_enabled(cx);
                                                },
                                            )),
                                    ),
                                ),
                            )
                            .when(self.postgres_form.ssh_enabled, |this| {
                                this.child(
                                    v_flex()
                                        .gap_3()
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Host"))
                                                        .child(Input::new(&self.postgres_form.ssh_host_input)),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Port"))
                                                        .child(Input::new(&self.postgres_form.ssh_port_input)),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("User"))
                                                        .child(Input::new(&self.postgres_form.ssh_user_input)),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Password"))
                                                        .child(Input::new(&self.postgres_form.ssh_password_input)),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Private key path"))
                                                        .child(
                                                            Input::new(&self.postgres_form.ssh_private_key_input).suffix(
                                                                Button::new("postgres-ssh-key-picker")
                                                                    .ghost()
                                                                    .icon(IconName::Folder)
                                                                    .xsmall()
                                                                    .on_click(cx.listener(
                                                                        |modal: &mut Self, _event, window, cx| {
                                                                            modal.pick_postgres_ssh_private_key(window, cx);
                                                                        },
                                                                    )),
                                                            ),
                                                        ),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Private key password"))
                                                        .child(Input::new(&self.postgres_form.ssh_private_key_password_input)),
                                                ),
                                        ),
                                )
                            })
                            .into_any_element()
                    }
                    ConnectorType::MySQL => {
                        let base_fields = self.mysql_form.render(cx);
                        v_flex()
                            .gap_3()
                            .child(base_fields)
                            // SSL Mode selector
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_2()
                                    .child(div().text_sm().child("SSL Mode"))
                                    .child(Select::new(&self.mysql_form.ssl_mode_select)),
                            )
                            // Advanced SSL section toggle
                            .child(
                                Button::new("mysql-ssl-advanced-toggle")
                                    .ghost()
                                    .xsmall()
                                    .child(if self.mysql_form.ssl_advanced_expanded {
                                        "Hide Advanced SSL"
                                    } else {
                                        "Show Advanced SSL"
                                    })
                                    .icon(if self.mysql_form.ssl_advanced_expanded {
                                        IconName::ChevronUp
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .on_click(cx.listener(
                                        |modal: &mut Self, _event, _window, cx| {
                                            modal.toggle_mysql_ssl_advanced(cx);
                                        },
                                    )),
                            )
                            // Advanced SSL fields
                            .when(self.mysql_form.ssl_advanced_expanded, |this| {
                                this.child(
                                    v_flex()
                                        .gap_3()
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(div().text_sm().child("SSL Key Path"))
                                                .child(
                                                    Input::new(&self.mysql_form.ssl_key_input).suffix(
                                                        Button::new("mysql-ssl-key-picker")
                                                            .ghost()
                                                            .icon(IconName::Folder)
                                                            .xsmall()
                                                            .on_click(cx.listener(
                                                                |modal: &mut Self, _event, window, cx| {
                                                                    modal.pick_mysql_ssl_key(window, cx);
                                                                },
                                                            )),
                                                    ),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(div().text_sm().child("SSL Cert Path"))
                                                .child(
                                                    Input::new(&self.mysql_form.ssl_cert_input).suffix(
                                                        Button::new("mysql-ssl-cert-picker")
                                                            .ghost()
                                                            .icon(IconName::Folder)
                                                            .xsmall()
                                                            .on_click(cx.listener(
                                                                |modal: &mut Self, _event, window, cx| {
                                                                    modal.pick_mysql_ssl_cert(window, cx);
                                                                },
                                                            )),
                                                    ),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(div().text_sm().child("SSL CA Cert Path"))
                                                .child(
                                                    Input::new(&self.mysql_form.ssl_ca_cert_input).suffix(
                                                        Button::new("mysql-ssl-ca-cert-picker")
                                                            .ghost()
                                                            .icon(IconName::Folder)
                                                            .xsmall()
                                                            .on_click(cx.listener(
                                                                |modal: &mut Self, _event, window, cx| {
                                                                    modal.pick_mysql_ssl_ca_cert(window, cx);
                                                                },
                                                            )),
                                                    ),
                                                ),
                                        ),
                                )
                            })
                            .child(
                                div().child(
                                    h_flex().gap_2().items_center().child(
                                        Switch::new("mysql-ssh-enabled-switch")
                                            .checked(self.mysql_form.ssh_enabled)
                                            .label("SSH")
                                            .on_click(cx.listener(
                                                |modal: &mut Self, _checked, _window, cx| {
                                                    modal.toggle_mysql_ssh_enabled(cx);
                                                },
                                            )),
                                    ),
                                ),
                            )
                            .when(self.mysql_form.ssh_enabled, |this| {
                                this.child(
                                    v_flex()
                                        .gap_3()
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Host"))
                                                        .child(Input::new(&self.mysql_form.ssh_host_input)),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Port"))
                                                        .child(Input::new(&self.mysql_form.ssh_port_input)),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("User"))
                                                        .child(Input::new(&self.mysql_form.ssh_user_input)),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Password"))
                                                        .child(Input::new(&self.mysql_form.ssh_password_input)),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Private key path"))
                                                        .child(
                                                            Input::new(&self.mysql_form.ssh_private_key_input).suffix(
                                                                Button::new("mysql-ssh-key-picker")
                                                                    .ghost()
                                                                    .icon(IconName::Folder)
                                                                    .xsmall()
                                                                    .on_click(cx.listener(
                                                                        |modal: &mut Self, _event, window, cx| {
                                                                            modal.pick_mysql_ssh_private_key(window, cx);
                                                                        },
                                                                    )),
                                                            ),
                                                        ),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .gap_2()
                                                        .child(div().text_sm().child("Private key password"))
                                                        .child(Input::new(&self.mysql_form.ssh_private_key_password_input)),
                                                ),
                                        ),
                                )
                            })
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
