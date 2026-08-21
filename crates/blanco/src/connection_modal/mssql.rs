use gpui::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use gpui_component::{
    IndexPath, h_flex,
    input::{Input, InputState},
    select::{Select, SelectState},
    switch::Switch,
    v_flex,
};

use app_database::{ConnectionData, EnvironmentType};

use super::NewConnectionModal;
use super::shared::{make_input, render_ssh_section};

pub(super) struct MssqlForm {
    pub host_input: Entity<InputState>,
    pub port_input: Entity<InputState>,
    pub database_input: Entity<InputState>,
    pub username_input: Entity<InputState>,
    pub password_input: Entity<InputState>,
    pub encrypt_select: Entity<SelectState<Vec<String>>>,
    pub trust_server_certificate: bool,
    pub ssh_enabled: bool,
    pub ssh_host_input: Entity<InputState>,
    pub ssh_port_input: Entity<InputState>,
    pub ssh_user_input: Entity<InputState>,
    pub ssh_password_input: Entity<InputState>,
    pub ssh_private_key_input: Entity<InputState>,
    pub ssh_private_key_password_input: Entity<InputState>,
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
        let ssh_port_initial = conn.and_then(|c| c.ssh_port).map(|p| p.to_string());

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
        let encrypt_initial = conn
            .and_then(|c| c.ssl_mode.as_ref())
            .and_then(|mode| encrypt_modes.iter().position(|m| m == mode))
            .unwrap_or(0);
        let encrypt_select = cx.new(|cx| {
            SelectState::new(
                encrypt_modes,
                Some(IndexPath::new(encrypt_initial)),
                window,
                cx,
            )
        });
        let trust_server_certificate = conn
            .map(|connection| connection.trust_server_certificate)
            .unwrap_or(false);

        let ssh_host_input = make_input(
            window,
            cx,
            "SSH Host",
            false,
            conn.and_then(|c| c.ssh_host.as_deref()),
        );
        let ssh_port_input = make_input(window, cx, "22", false, ssh_port_initial.as_deref());
        let ssh_user_input = make_input(
            window,
            cx,
            "SSH Username",
            false,
            conn.and_then(|c| c.ssh_user.as_deref()),
        );
        let ssh_password_input = make_input(
            window,
            cx,
            "SSH Password (optional)",
            true,
            conn.and_then(|c| c.ssh_password.as_deref()),
        );
        let ssh_private_key_input = make_input(
            window,
            cx,
            "Private Key Path (optional)",
            false,
            conn.and_then(|c| c.ssh_private_key_path.as_deref()),
        );
        let ssh_private_key_password_input = make_input(
            window,
            cx,
            "Private Key Password (optional)",
            true,
            conn.and_then(|c| c.ssh_private_key_password.as_deref()),
        );

        let ssh_enabled = conn.map(|c| c.uses_ssh_tunnel()).unwrap_or(false);

        Self {
            host_input,
            port_input,
            database_input,
            username_input,
            password_input,
            encrypt_select,
            trust_server_certificate,
            ssh_enabled,
            ssh_host_input,
            ssh_port_input,
            ssh_user_input,
            ssh_password_input,
            ssh_private_key_input,
            ssh_private_key_password_input,
        }
    }

    pub fn toggle_ssh(&mut self) {
        self.ssh_enabled = !self.ssh_enabled;
    }

    pub fn set_trust_server_certificate(&mut self, trust: bool) {
        self.trust_server_certificate = trust;
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

        let encrypt = self.encrypt_select.read(cx).selected_value().cloned();

        let mut connection = if self.ssh_enabled {
            let ssh_host = self.ssh_host_input.read(cx).value();
            let ssh_port_str = self.ssh_port_input.read(cx).value();
            let ssh_user = self.ssh_user_input.read(cx).value();

            if ssh_host.is_empty() || ssh_user.is_empty() {
                return None;
            }
            let ssh_port = if ssh_port_str.is_empty() {
                22
            } else {
                ssh_port_str.parse::<i32>().ok()?
            };
            let ssh_password = optional_path(&self.ssh_password_input, cx);
            let ssh_private_key_path = optional_path(&self.ssh_private_key_input, cx);
            let ssh_private_key_password = optional_path(&self.ssh_private_key_password_input, cx);

            ConnectionData::new_mssql(name, host, port, database, username, password).with_ssh(
                ssh_host.to_string(),
                ssh_port,
                ssh_user.to_string(),
                ssh_password,
                ssh_private_key_path,
                ssh_private_key_password,
            )
        } else {
            ConnectionData::new_mssql(name, host, port, database, username, password)
        };
        connection.environment_type = environment_type;
        connection.ssl_mode = encrypt;
        connection.trust_server_certificate = self.trust_server_certificate;
        Some(connection)
    }
}

fn optional_path(input: &Entity<InputState>, cx: &App) -> Option<String> {
    let value = input.read(cx).value();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
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
    cx: &mut Context<NewConnectionModal>,
) -> AnyElement {
    let form = &modal.mssql_form;
    let ssh_enabled = form.ssh_enabled;
    let trust_server_certificate = form.trust_server_certificate;
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
        .child(
            Switch::new("mssql-trust-server-certificate-switch")
                .checked(trust_server_certificate)
                .label("Trust server certificate (insecure)")
                .on_click(cx.listener(|modal, checked, _window, cx| {
                    modal.mssql_form.set_trust_server_certificate(*checked);
                    cx.notify();
                })),
        )
        .child(
            div().child(
                h_flex().gap_2().items_center().child(
                    Switch::new("mssql-ssh-enabled-switch")
                        .checked(ssh_enabled)
                        .label("SSH")
                        .on_click(cx.listener(|modal, _checked, _window, cx| {
                            modal.mssql_form.toggle_ssh();
                            cx.notify();
                        })),
                ),
            ),
        )
        .when(ssh_enabled, |this| {
            this.child(render_ssh_section(
                "mssql",
                &form.ssh_host_input,
                &form.ssh_port_input,
                &form.ssh_user_input,
                &form.ssh_password_input,
                &form.ssh_private_key_input,
                &form.ssh_private_key_password_input,
                cx,
            ))
        })
        .into_any_element()
}
