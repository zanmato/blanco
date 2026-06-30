mod clickhouse;
mod mssql;
mod mysql;
mod postgres;
mod redis;
mod shared;
mod sqlite;
mod types;

use clickhouse::ClickhouseForm;
use mssql::MssqlForm;
use mysql::MysqlForm;
use postgres::PostgresForm;
use sqlite::SqliteForm;
use types::ConnectorType;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, IndexPath, WindowExt as _, h_flex,
    input::{Input, InputState},
    notification::NotificationType,
    select::{Select, SelectState},
    v_flex,
};

use crate::app_database::{ConnectionData, EnvironmentType};

pub struct NewConnectionModal {
    focus_handle: FocusHandle,
    name_input: Entity<InputState>,
    db_type_select: Entity<SelectState<Vec<String>>>,
    environment_type_select: Entity<SelectState<Vec<String>>>,
    sqlite_form: SqliteForm,
    postgres_form: PostgresForm,
    mysql_form: MysqlForm,
    clickhouse_form: ClickhouseForm,
    mssql_form: MssqlForm,
    redis_form: redis::RedisForm,
    is_testing: bool,
    editing_connection_id: Option<i64>,
    db_type_locked: bool,
    #[cfg(test)]
    last_test_result: Option<Result<(), String>>,
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
            "ClickHouse".to_string(),
            "SQL Server".to_string(),
            "Redis".to_string(),
        ];

        let (initial_name, initial_db_type, editing_connection_id, db_type_locked) =
            if let Some(conn) = &connection_data {
                let conn_id = conn.id;
                let db_type_index = match conn.db_type {
                    database::DatabaseType::PostgreSQL => 1,
                    database::DatabaseType::MySQL => 2,
                    database::DatabaseType::ClickHouse => 3,
                    database::DatabaseType::MsSql => 4,
                    database::DatabaseType::Redis => 5,
                    database::DatabaseType::SQLite => 0,
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

        let db_type_select = cx
            .new(|cx| SelectState::new(db_types, initial_db_type.map(IndexPath::new), window, cx));

        let environment_types = vec!["DEV".to_string(), "TEST".to_string(), "PROD".to_string()];
        let initial_env_index = connection_data
            .as_ref()
            .map(|c| match c.environment_type {
                EnvironmentType::Test => 1,
                EnvironmentType::Prod => 2,
                _ => 0,
            })
            .or(Some(0));
        let environment_type_select = cx.new(|cx| {
            SelectState::new(
                environment_types,
                initial_env_index.map(IndexPath::new),
                window,
                cx,
            )
        });

        let conn_ref = connection_data.as_ref();
        let sqlite_form = SqliteForm::new(window, cx, conn_ref);
        let postgres_form = PostgresForm::new(window, cx, conn_ref);
        let mysql_form = MysqlForm::new(window, cx, conn_ref);
        let clickhouse_form = ClickhouseForm::new(window, cx, conn_ref);
        let mssql_form = MssqlForm::new(window, cx, conn_ref);
        let redis_form = redis::RedisForm::new(window, cx, conn_ref);

        Self {
            focus_handle: cx.focus_handle(),
            name_input,
            db_type_select,
            environment_type_select,
            sqlite_form,
            postgres_form,
            mysql_form,
            clickhouse_form,
            mssql_form,
            redis_form,
            is_testing: false,
            editing_connection_id,
            db_type_locked,
            #[cfg(test)]
            last_test_result: None,
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

    fn validate(&self, connector_type: &ConnectorType, cx: &App) -> Option<String> {
        match connector_type {
            ConnectorType::SQLite => self.sqlite_form.validate(cx),
            ConnectorType::PostgreSQL => self.postgres_form.validate(cx),
            ConnectorType::MySQL => self.mysql_form.validate(cx),
            ConnectorType::ClickHouse => self.clickhouse_form.validate(cx),
            ConnectorType::MsSql => self.mssql_form.validate(cx),
            ConnectorType::Redis => self.redis_form.validate(cx),
        }
    }

    fn build_connection_data(
        &self,
        connector_type: &ConnectorType,
        name: String,
        environment_type: EnvironmentType,
        cx: &App,
    ) -> Option<ConnectionData> {
        match connector_type {
            ConnectorType::SQLite => {
                self.sqlite_form
                    .get_connection_data(name, environment_type, cx)
            }
            ConnectorType::PostgreSQL => {
                self.postgres_form
                    .get_connection_data(name, environment_type, cx)
            }
            ConnectorType::MySQL => self
                .mysql_form
                .get_connection_data(name, environment_type, cx),
            ConnectorType::ClickHouse => {
                self.clickhouse_form
                    .get_connection_data(name, environment_type, cx)
            }
            ConnectorType::MsSql => self
                .mssql_form
                .get_connection_data(name, environment_type, cx),
            ConnectorType::Redis => self
                .redis_form
                .get_connection_data(name, environment_type, cx),
        }
    }

    pub fn test_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_testing {
            return;
        }

        let connector_type = self.get_selected_connector_type(cx);

        if let Some(error) = self.validate(&connector_type, cx) {
            window.push_notification((NotificationType::Error, error), cx);
            return;
        }

        let name = self.name_input.read(cx).value().to_string();
        let name = if name.is_empty() {
            "test".to_string()
        } else {
            name
        };
        let environment_type = self.get_selected_environment_type(cx);

        let connection_data =
            match self.build_connection_data(&connector_type, name, environment_type, cx) {
                Some(data) => data,
                None => {
                    window.push_notification(
                        (
                            NotificationType::Error,
                            "Please fill in all required fields",
                        ),
                        cx,
                    );
                    return;
                }
            };

        let mut data_with_id = connection_data;
        data_with_id.id = Some(0);
        let config = match data_with_id.to_connection_config() {
            Some(c) => c,
            None => {
                window.push_notification(
                    (
                        NotificationType::Error,
                        "Failed to build connection configuration",
                    ),
                    cx,
                );
                return;
            }
        };

        self.is_testing = true;
        cx.notify();

        let db_service = database::DatabaseService::global(cx).clone();

        // `spawn_in` hands the async block an `AsyncWindowContext` so the result
        // can be surfaced as a window notification rather than inline in the
        // modal's scrollable body.
        cx.spawn_in(window, async move |this, window| {
            let result = db_service.test_connection(&config).await;

            this.update_in(window, |modal, window, cx| {
                modal.is_testing = false;
                cx.notify();

                #[cfg(test)]
                {
                    modal.last_test_result = Some(
                        result
                            .as_ref()
                            .map(|_| ())
                            .map_err(|err| format!("{err:#}")),
                    );
                }

                match result {
                    Ok(()) => window.push_notification(
                        (NotificationType::Success, "Connection successful"),
                        cx,
                    ),
                    Err(err) => window.push_notification(
                        (
                            NotificationType::Error,
                            format!("Connection failed: {err:#}"),
                        ),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn get_connection_data(&self, cx: &App) -> Option<ConnectionData> {
        let name = self.name_input.read(cx).value().to_string();
        if name.is_empty() {
            return None;
        }

        let connector_type = self.get_selected_connector_type(cx);
        let environment_type = self.get_selected_environment_type(cx);

        let mut connection =
            self.build_connection_data(&connector_type, name, environment_type, cx)?;

        if let Some(editing_id) = self.editing_connection_id {
            connection.id = Some(editing_id);
        }

        Some(connection)
    }
}

#[cfg(test)]
impl NewConnectionModal {
    /// Most recent `test_connection` outcome, consumed so a subsequent attempt
    /// starts from a clean slate. `None` until a test has completed.
    pub fn take_test_result(&mut self) -> Option<Result<(), String>> {
        self.last_test_result.take()
    }

    pub fn is_testing(&self) -> bool {
        self.is_testing
    }

    /// Set the connection name (required by `get_connection_data`).
    pub fn set_name(&self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.name_input
            .update(cx, |state, cx| state.set_value(name, window, cx));
    }

    /// Select the database type by its display label (e.g. "PostgreSQL").
    pub fn select_db_type(&self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let index = match label {
            "PostgreSQL" => 1,
            "MySQL" => 2,
            "ClickHouse" => 3,
            "SQL Server" => 4,
            "Redis" => 5,
            _ => 0,
        };
        self.db_type_select.update(cx, |state, cx| {
            state.set_selected_index(Some(IndexPath::new(index)), window, cx);
        });
    }

    /// Populate the host/port/database/username/password inputs of the form
    /// matching the currently selected connector type.
    #[allow(clippy::too_many_arguments)]
    pub fn set_network_credentials(
        &self,
        host: &str,
        port: &str,
        database: &str,
        username: &str,
        password: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inputs = match self.get_selected_connector_type(cx) {
            ConnectorType::PostgreSQL => (
                &self.postgres_form.host_input,
                &self.postgres_form.port_input,
                &self.postgres_form.database_input,
                &self.postgres_form.username_input,
                &self.postgres_form.password_input,
            ),
            ConnectorType::MySQL => (
                &self.mysql_form.host_input,
                &self.mysql_form.port_input,
                &self.mysql_form.database_input,
                &self.mysql_form.username_input,
                &self.mysql_form.password_input,
            ),
            ConnectorType::ClickHouse => (
                &self.clickhouse_form.host_input,
                &self.clickhouse_form.port_input,
                &self.clickhouse_form.database_input,
                &self.clickhouse_form.username_input,
                &self.clickhouse_form.password_input,
            ),
            ConnectorType::MsSql => (
                &self.mssql_form.host_input,
                &self.mssql_form.port_input,
                &self.mssql_form.database_input,
                &self.mssql_form.username_input,
                &self.mssql_form.password_input,
            ),
            ConnectorType::Redis => (
                &self.redis_form.host_input,
                &self.redis_form.port_input,
                &self.redis_form.database_input,
                &self.redis_form.username_input,
                &self.redis_form.password_input,
            ),
            ConnectorType::SQLite => {
                panic!("set_network_credentials called for SQLite; use set_sqlite_path")
            }
        };
        let (host_input, port_input, database_input, username_input, password_input) = inputs;
        host_input.update(cx, |state, cx| state.set_value(host, window, cx));
        port_input.update(cx, |state, cx| state.set_value(port, window, cx));
        database_input.update(cx, |state, cx| state.set_value(database, window, cx));
        username_input.update(cx, |state, cx| state.set_value(username, window, cx));
        password_input.update(cx, |state, cx| state.set_value(password, window, cx));
    }

    /// Set the SQLite database file path.
    pub fn set_sqlite_path(&self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.sqlite_form
            .file_path_input
            .update(cx, |state, cx| state.set_value(path, window, cx));
    }

    /// Enable SSH on the active form and populate its tunnel fields.
    pub fn set_ssh_credentials(
        &mut self,
        ssh_host: &str,
        ssh_port: &str,
        ssh_user: &str,
        ssh_password: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inputs = match self.get_selected_connector_type(cx) {
            ConnectorType::PostgreSQL => {
                self.postgres_form.ssh_enabled = true;
                (
                    &self.postgres_form.ssh_host_input,
                    &self.postgres_form.ssh_port_input,
                    &self.postgres_form.ssh_user_input,
                    &self.postgres_form.ssh_password_input,
                )
            }
            ConnectorType::MySQL => {
                self.mysql_form.ssh_enabled = true;
                (
                    &self.mysql_form.ssh_host_input,
                    &self.mysql_form.ssh_port_input,
                    &self.mysql_form.ssh_user_input,
                    &self.mysql_form.ssh_password_input,
                )
            }
            ConnectorType::ClickHouse => {
                self.clickhouse_form.ssh_enabled = true;
                (
                    &self.clickhouse_form.ssh_host_input,
                    &self.clickhouse_form.ssh_port_input,
                    &self.clickhouse_form.ssh_user_input,
                    &self.clickhouse_form.ssh_password_input,
                )
            }
            ConnectorType::MsSql => {
                self.mssql_form.ssh_enabled = true;
                (
                    &self.mssql_form.ssh_host_input,
                    &self.mssql_form.ssh_port_input,
                    &self.mssql_form.ssh_user_input,
                    &self.mssql_form.ssh_password_input,
                )
            }
            ConnectorType::Redis => {
                self.redis_form.ssh_enabled = true;
                (
                    &self.redis_form.ssh_host_input,
                    &self.redis_form.ssh_port_input,
                    &self.redis_form.ssh_user_input,
                    &self.redis_form.ssh_password_input,
                )
            }
            ConnectorType::SQLite => {
                panic!("set_ssh_credentials called for SQLite; SQLite has no SSH support")
            }
        };
        let (host_input, port_input, user_input, password_input) = inputs;
        host_input.update(cx, |state, cx| state.set_value(ssh_host, window, cx));
        port_input.update(cx, |state, cx| state.set_value(ssh_port, window, cx));
        user_input.update(cx, |state, cx| state.set_value(ssh_user, window, cx));
        password_input.update(cx, |state, cx| state.set_value(ssh_password, window, cx));
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
                                .child(
                                    Select::new(&self.db_type_select).disabled(self.db_type_locked),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_2()
                                .child(div().text_sm().child("Environment"))
                                .child(Select::new(&self.environment_type_select)),
                        ),
                )
                .child(match connector_type {
                    ConnectorType::SQLite => sqlite::render(self, cx),
                    ConnectorType::PostgreSQL => postgres::render(self, cx),
                    ConnectorType::MySQL => mysql::render(self, cx),
                    ConnectorType::ClickHouse => clickhouse::render(self, cx),
                    ConnectorType::MsSql => mssql::render(self, cx),
                    ConnectorType::Redis => redis::render(self, cx),
                })
                .when(self.is_testing, |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .p_3()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().blue.opacity(0.1))
                            .text_color(cx.theme().blue)
                            .child(Icon::new(IconName::LoaderCircle).size_4())
                            .child(div().text_sm().child("Testing connection...")),
                    )
                }),
        )
    }
}
