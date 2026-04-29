use gpui::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div,
};
use gpui_component::{
    IndexPath, h_flex,
    input::{Input, InputState},
    select::{Select, SelectState},
    v_flex,
};

use crate::app_database::{ConnectionData, EnvironmentType};

use super::NewConnectionModal;
use super::shared::make_input;

pub(super) struct MssqlForm {
    pub host_input: Entity<InputState>,
    pub port_input: Entity<InputState>,
    pub database_input: Entity<InputState>,
    pub username_input: Entity<InputState>,
    pub password_input: Entity<InputState>,
    pub encrypt_select: Entity<SelectState<Vec<String>>>,
}

impl MssqlForm {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<NewConnectionModal>,
        connection_data: Option<&ConnectionData>,
    ) -> Self {
        let is_mssql = connection_data
            .map(|c| c.db_type == database::DatabaseType::MsSql)
            .unwrap_or(false);
        let conn = if is_mssql { connection_data } else { None };

        let port_initial = conn.and_then(|c| c.port).map(|p| p.to_string());

        let host_input = make_input(
            window,
            cx,
            "localhost",
            false,
            conn.and_then(|c| c.host.as_deref()),
        );
        let port_input = make_input(window, cx, "1433", false, port_initial.as_deref());
        let database_input = make_input(
            window,
            cx,
            "Database Name",
            false,
            conn.and_then(|c| c.database_name.as_deref()),
        );
        let username_input = make_input(
            window,
            cx,
            "sa",
            false,
            conn.and_then(|c| c.username.as_deref()),
        );
        let password_input = make_input(
            window,
            cx,
            "Password",
            true,
            conn.and_then(|c| c.password.as_deref()),
        );

        let encrypt_modes = vec!["Off".to_string(), "On".to_string(), "Required".to_string()];
        let encrypt_select =
            cx.new(|cx| SelectState::new(encrypt_modes, Some(IndexPath::new(0)), window, cx));

        Self {
            host_input,
            port_input,
            database_input,
            username_input,
            password_input,
            encrypt_select,
        }
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

        if host.is_empty() || database.is_empty() || username.is_empty() {
            return None;
        }
        let port = port_str.parse::<i32>().ok()?;

        let mut connection =
            ConnectionData::new_mssql(name, host, port, database, username, password);
        connection.environment_type = environment_type;
        Some(connection)
    }
}

fn render_base_fields(form: &MssqlForm) -> AnyElement {
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
                        .child(Input::new(&form.host_input)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child("Port"))
                        .child(Input::new(&form.port_input)),
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
                        .child(Input::new(&form.username_input)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child("Password"))
                        .child(Input::new(&form.password_input).mask_toggle()),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(div().text_sm().child("Database name"))
                .child(Input::new(&form.database_input)),
        )
        .into_any_element()
}

pub(super) fn render(
    modal: &NewConnectionModal,
    _cx: &mut Context<NewConnectionModal>,
) -> AnyElement {
    let form = &modal.mssql_form;
    v_flex()
        .gap_3()
        .child(render_base_fields(form))
        .child(
            v_flex()
                .flex_1()
                .gap_2()
                .child(div().text_sm().child("Encryption"))
                .child(Select::new(&form.encrypt_select)),
        )
        .into_any_element()
}
