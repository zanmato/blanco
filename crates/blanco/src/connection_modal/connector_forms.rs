use gpui::{App, Entity, FocusHandle, Focusable};
use gpui_component::input::InputState;
use gpui_component::select::SelectState;

use crate::app_database::{ConnectionData, EnvironmentType};
use blanco_core::connection_trait::DriverType;

/// Represents a database connector type
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ConnectorType {
    SQLite,
    PostgreSQL,
    MySQL,
}

impl ConnectorType {
    pub(super) fn from_str(s: &str) -> Self {
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
pub(super) struct TestResult {
    pub success: bool,
    pub message: String,
}

/// SQLite connector form
pub(super) struct SqliteForm {
    pub file_path_input: Entity<InputState>,
}

impl SqliteForm {
    pub fn new(file_path_input: Entity<InputState>) -> Self {
        Self { file_path_input }
    }

    pub fn get_connection_data(
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
}

/// PostgreSQL connector form
pub(super) struct PostgresForm {
    pub host_input: Entity<InputState>,
    pub port_input: Entity<InputState>,
    pub database_input: Entity<InputState>,
    pub username_input: Entity<InputState>,
    pub password_input: Entity<InputState>,
    // SSH tunnel configuration
    pub ssh_enabled: bool,
    pub ssh_host_input: Entity<InputState>,
    pub ssh_port_input: Entity<InputState>,
    pub ssh_user_input: Entity<InputState>,
    pub ssh_password_input: Entity<InputState>,
    pub ssh_private_key_input: Entity<InputState>,
    pub ssh_private_key_password_input: Entity<InputState>,
    // SSL/TLS configuration
    pub ssl_mode_select: Entity<SelectState<Vec<String>>>,
    pub ssl_key_input: Entity<InputState>,
    pub ssl_cert_input: Entity<InputState>,
    pub ssl_ca_cert_input: Entity<InputState>,
    pub ssl_advanced_expanded: bool,
}

impl PostgresForm {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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

    pub fn render(&self, _cx: &App) -> gpui::AnyElement {
        use gpui::{IntoElement, ParentElement, Styled};
        use gpui_component::{h_flex, input::Input, v_flex};

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(gpui::div().text_sm().child("Host"))
                            .child(Input::new(&self.host_input)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(gpui::div().text_sm().child("Port"))
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
                            .child(gpui::div().text_sm().child("User"))
                            .child(Input::new(&self.username_input)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(gpui::div().text_sm().child("Password"))
                            .child(Input::new(&self.password_input)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(gpui::div().text_sm().child("Database name"))
                    .child(Input::new(&self.database_input)),
            )
            .into_any_element()
    }

    pub fn validate(&self, cx: &App) -> Option<String> {
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

    pub fn get_connection_data(
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

    #[allow(dead_code)]
    pub fn first_input_focus_handle(&self, cx: &App) -> FocusHandle {
        self.host_input.focus_handle(cx)
    }
}

/// MySQL connector form
pub(super) struct MysqlForm {
    pub host_input: Entity<InputState>,
    pub port_input: Entity<InputState>,
    pub database_input: Entity<InputState>,
    pub username_input: Entity<InputState>,
    pub password_input: Entity<InputState>,
    // SSH tunnel configuration
    pub ssh_enabled: bool,
    pub ssh_host_input: Entity<InputState>,
    pub ssh_port_input: Entity<InputState>,
    pub ssh_user_input: Entity<InputState>,
    pub ssh_password_input: Entity<InputState>,
    pub ssh_private_key_input: Entity<InputState>,
    pub ssh_private_key_password_input: Entity<InputState>,
    // SSL/TLS configuration
    pub ssl_mode_select: Entity<SelectState<Vec<String>>>,
    pub ssl_key_input: Entity<InputState>,
    pub ssl_cert_input: Entity<InputState>,
    pub ssl_ca_cert_input: Entity<InputState>,
    pub ssl_advanced_expanded: bool,
}

impl MysqlForm {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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

    pub fn render(&self, _cx: &App) -> gpui::AnyElement {
        use gpui::{IntoElement, ParentElement, Styled};
        use gpui_component::{h_flex, input::Input, v_flex};

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(gpui::div().text_sm().child("Host"))
                            .child(Input::new(&self.host_input)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(gpui::div().text_sm().child("Port"))
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
                            .child(gpui::div().text_sm().child("User"))
                            .child(Input::new(&self.username_input)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(gpui::div().text_sm().child("Password"))
                            .child(Input::new(&self.password_input)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(gpui::div().text_sm().child("Database name"))
                    .child(Input::new(&self.database_input)),
            )
            .into_any_element()
    }

    pub fn build_connection_data(&self, name: &str, cx: &App) -> Option<ConnectionData> {
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

    pub fn validate(&self, cx: &App) -> Option<String> {
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

    #[allow(dead_code)]
    pub fn first_input_focus_handle(&self, cx: &App) -> FocusHandle {
        self.host_input.focus_handle(cx)
    }
}
