use gpui::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use gpui_component::{
    IconName, IndexPath, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    select::{Select, SelectState},
    switch::Switch,
    v_flex,
};

use crate::app_database::{ConnectionData, EnvironmentType};

use super::NewConnectionModal;
use super::shared::{make_input, render_ssh_section, render_ssl_advanced_fields};

pub(super) struct MysqlForm {
    pub host_input: Entity<InputState>,
    pub port_input: Entity<InputState>,
    pub database_input: Entity<InputState>,
    pub username_input: Entity<InputState>,
    pub password_input: Entity<InputState>,
    pub ssh_enabled: bool,
    pub ssh_host_input: Entity<InputState>,
    pub ssh_port_input: Entity<InputState>,
    pub ssh_user_input: Entity<InputState>,
    pub ssh_password_input: Entity<InputState>,
    pub ssh_private_key_input: Entity<InputState>,
    pub ssh_private_key_password_input: Entity<InputState>,
    pub ssl_mode_select: Entity<SelectState<Vec<String>>>,
    pub ssl_key_input: Entity<InputState>,
    pub ssl_cert_input: Entity<InputState>,
    pub ssl_ca_cert_input: Entity<InputState>,
    pub ssl_advanced_expanded: bool,
}

impl MysqlForm {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<NewConnectionModal>,
        connection_data: Option<&ConnectionData>,
    ) -> Self {
        let port_initial = connection_data.and_then(|c| c.port).map(|p| p.to_string());
        let ssh_port_initial = connection_data
            .and_then(|c| c.ssh_port)
            .map(|p| p.to_string());

        let host_input = make_input(
            window,
            cx,
            "localhost",
            false,
            connection_data.and_then(|c| c.host.as_deref()),
        );
        let port_input = make_input(window, cx, "3306", false, port_initial.as_deref());
        let database_input = make_input(
            window,
            cx,
            "Database Name",
            false,
            connection_data.and_then(|c| c.database_name.as_deref()),
        );
        let username_input = make_input(
            window,
            cx,
            "root",
            false,
            connection_data.and_then(|c| c.username.as_deref()),
        );
        let password_input = make_input(
            window,
            cx,
            "Password",
            true,
            connection_data.and_then(|c| c.password.as_deref()),
        );

        let ssh_host_input = make_input(
            window,
            cx,
            "SSH Host",
            false,
            connection_data.and_then(|c| c.ssh_host.as_deref()),
        );
        let ssh_port_input = make_input(window, cx, "22", false, ssh_port_initial.as_deref());
        let ssh_user_input = make_input(
            window,
            cx,
            "SSH Username",
            false,
            connection_data.and_then(|c| c.ssh_user.as_deref()),
        );
        let ssh_password_input = make_input(
            window,
            cx,
            "SSH Password (optional)",
            true,
            connection_data.and_then(|c| c.ssh_password.as_deref()),
        );
        let ssh_private_key_input = make_input(
            window,
            cx,
            "Private Key Path (optional)",
            false,
            connection_data.and_then(|c| c.ssh_private_key_path.as_deref()),
        );
        let ssh_private_key_password_input = make_input(
            window,
            cx,
            "Private Key Password (optional)",
            true,
            connection_data.and_then(|c| c.ssh_private_key_password.as_deref()),
        );

        let ssl_modes = vec![
            "preferred".to_string(),
            "required".to_string(),
            "disabled".to_string(),
            "verify-ca".to_string(),
            "verify-full".to_string(),
        ];
        let initial_ssl_index = connection_data
            .and_then(|c| c.ssl_mode.as_ref())
            .and_then(|mode| ssl_modes.iter().position(|m| m == mode));
        let ssl_mode_select = cx.new(|cx| {
            SelectState::new(ssl_modes, initial_ssl_index.map(IndexPath::new), window, cx)
        });
        let ssl_key_input = make_input(
            window,
            cx,
            "SSL Key Path (optional)",
            false,
            connection_data.and_then(|c| c.ssl_key_path.as_deref()),
        );
        let ssl_cert_input = make_input(
            window,
            cx,
            "SSL Cert Path (optional)",
            false,
            connection_data.and_then(|c| c.ssl_cert_path.as_deref()),
        );
        let ssl_ca_cert_input = make_input(
            window,
            cx,
            "SSL CA Cert Path (optional)",
            false,
            connection_data.and_then(|c| c.ssl_ca_cert_path.as_deref()),
        );

        let ssh_enabled = connection_data
            .map(|c| c.uses_ssh_tunnel())
            .unwrap_or(false);

        Self {
            host_input,
            port_input,
            database_input,
            username_input,
            password_input,
            ssh_enabled,
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

    pub fn toggle_ssh(&mut self) {
        self.ssh_enabled = !self.ssh_enabled;
    }

    pub fn toggle_ssl_advanced(&mut self) {
        self.ssl_advanced_expanded = !self.ssl_advanced_expanded;
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

        if host.is_empty() || port_str.is_empty() || database.is_empty() || username.is_empty() {
            return None;
        }

        let port = port_str.parse::<i32>().unwrap_or(3306);

        let mut connection =
            ConnectionData::new_mysql(name, host, port, database, username, password);
        connection.environment_type = environment_type;
        Some(connection)
    }
}

fn render_base_fields(form: &MysqlForm) -> AnyElement {
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
    let form = &modal.mysql_form;
    let ssl_advanced_expanded = form.ssl_advanced_expanded;
    let ssh_enabled = form.ssh_enabled;

    v_flex()
        .gap_3()
        .child(render_base_fields(form))
        .child(
            v_flex()
                .flex_1()
                .gap_2()
                .child(div().text_sm().child("SSL Mode"))
                .child(Select::new(&form.ssl_mode_select)),
        )
        .child(
            Button::new("mysql-ssl-advanced-toggle")
                .ghost()
                .xsmall()
                .child(if ssl_advanced_expanded {
                    "Hide Advanced SSL"
                } else {
                    "Show Advanced SSL"
                })
                .icon(if ssl_advanced_expanded {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .on_click(cx.listener(|modal, _event, _window, cx| {
                    modal.mysql_form.toggle_ssl_advanced();
                    cx.notify();
                })),
        )
        .when(ssl_advanced_expanded, |this| {
            this.child(render_ssl_advanced_fields(
                "mysql",
                &form.ssl_key_input,
                &form.ssl_cert_input,
                &form.ssl_ca_cert_input,
                cx,
            ))
        })
        .child(
            div().child(
                h_flex().gap_2().items_center().child(
                    Switch::new("mysql-ssh-enabled-switch")
                        .checked(ssh_enabled)
                        .label("SSH")
                        .on_click(cx.listener(|modal, _checked, _window, cx| {
                            modal.mysql_form.toggle_ssh();
                            cx.notify();
                        })),
                ),
            ),
        )
        .when(ssh_enabled, |this| {
            this.child(render_ssh_section(
                "mysql",
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
