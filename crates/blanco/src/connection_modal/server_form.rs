//! One form for every host/port/user/password backend with SSL and SSH
//! options (PostgreSQL, MySQL, ClickHouse). The backends differ only in
//! defaults and the SSL mode list, which live in [`ServerFormSpec`], so adding
//! a similar backend is a new spec rather than a new 400 line file.

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
use super::types::host_contains_explicit_port;

pub(super) struct ServerFormSpec {
    pub db_type: database::DatabaseType,
    /// Stable prefix for element ids so the forms do not collide in one modal.
    pub id_prefix: &'static str,
    pub host_label: &'static str,
    pub default_port: i32,
    /// Used when the port field is left empty and the host carries no port.
    /// ClickHouse picks 8443 for `https://` hosts.
    pub port_for_host: fn(&str) -> i32,
    /// Whether the host may carry its own port (`host:port`, URLs), which
    /// makes the port field optional.
    pub port_optional_when_in_host: bool,
    pub database_placeholder: &'static str,
    pub username_placeholder: &'static str,
    pub ssl_modes: &'static [&'static str],
}

pub(super) const POSTGRES_SPEC: ServerFormSpec = ServerFormSpec {
    db_type: database::DatabaseType::PostgreSQL,
    id_prefix: "postgres",
    host_label: "Host",
    default_port: 5432,
    port_for_host: |_| 5432,
    port_optional_when_in_host: false,
    database_placeholder: "Database Name",
    username_placeholder: "postgres",
    ssl_modes: &[
        "preferred",
        "required",
        "disabled",
        "allow",
        "verify-ca",
        "verify-full",
    ],
};

pub(super) const MYSQL_SPEC: ServerFormSpec = ServerFormSpec {
    db_type: database::DatabaseType::MySQL,
    id_prefix: "mysql",
    host_label: "Host",
    default_port: 3306,
    port_for_host: |_| 3306,
    port_optional_when_in_host: false,
    database_placeholder: "Database Name",
    username_placeholder: "root",
    ssl_modes: &[
        "preferred",
        "required",
        "disabled",
        "verify-ca",
        "verify-full",
    ],
};

pub(super) const CLICKHOUSE_SPEC: ServerFormSpec = ServerFormSpec {
    db_type: database::DatabaseType::ClickHouse,
    id_prefix: "clickhouse",
    host_label: "Host (http:// or https://)",
    default_port: 8123,
    port_for_host: |host| {
        if host.starts_with("https://") {
            8443
        } else {
            8123
        }
    },
    port_optional_when_in_host: true,
    database_placeholder: "default",
    username_placeholder: "default",
    ssl_modes: &["disabled", "required", "preferred"],
};

pub(super) struct ServerForm {
    spec: &'static ServerFormSpec,
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

impl ServerForm {
    pub fn new(
        spec: &'static ServerFormSpec,
        window: &mut Window,
        cx: &mut Context<NewConnectionModal>,
        connection_data: Option<&ConnectionData>,
    ) -> Self {
        // Only prefill from a connection of the same backend, so editing a
        // MySQL connection does not leak its host into the PostgreSQL form.
        let conn = connection_data.filter(|c| c.db_type == spec.db_type);
        let port_initial = conn.and_then(|c| c.port).map(|p| p.to_string());
        let ssh_port_initial = conn.and_then(|c| c.ssh_port).map(|p| p.to_string());
        let default_port = spec.default_port.to_string();

        let host_input = make_input(
            window,
            cx,
            "localhost",
            false,
            conn.and_then(|c| c.host.as_deref()),
        );
        let port_input = make_input(window, cx, &default_port, false, port_initial.as_deref());
        let database_input = make_input(
            window,
            cx,
            spec.database_placeholder,
            false,
            conn.and_then(|c| c.database_name.as_deref()),
        );
        let username_input = make_input(
            window,
            cx,
            spec.username_placeholder,
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

        let ssl_modes: Vec<String> = spec.ssl_modes.iter().map(|m| m.to_string()).collect();
        let initial_ssl_index = conn
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
            conn.and_then(|c| c.ssl_key_path.as_deref()),
        );
        let ssl_cert_input = make_input(
            window,
            cx,
            "SSL Cert Path (optional)",
            false,
            conn.and_then(|c| c.ssl_cert_path.as_deref()),
        );
        let ssl_ca_cert_input = make_input(
            window,
            cx,
            "SSL CA Cert Path (optional)",
            false,
            conn.and_then(|c| c.ssl_ca_cert_path.as_deref()),
        );

        let ssh_enabled = conn.map(|c| c.uses_ssh_tunnel()).unwrap_or(false);

        Self {
            spec,
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

    fn port_is_optional(&self, host: &str) -> bool {
        self.spec.port_optional_when_in_host && host_contains_explicit_port(host)
    }

    pub fn validate(&self, cx: &App) -> Option<String> {
        let host = self.host_input.read(cx).value();
        let port_str = self.port_input.read(cx).value();
        let database = self.database_input.read(cx).value();
        let username = self.username_input.read(cx).value();

        if host.is_empty() {
            return Some("Host is required".to_string());
        }
        if port_str.is_empty() && !self.port_is_optional(&host) {
            return Some("Port is required".to_string());
        }
        if !port_str.is_empty() && port_str.parse::<u16>().is_err() {
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

        let port = if port_str.is_empty() {
            if !self.port_is_optional(&host) {
                return None;
            }
            (self.spec.port_for_host)(&host)
        } else {
            port_str.parse::<i32>().ok()?
        };

        let mut connection = ConnectionData::new_server(
            self.spec.db_type,
            name,
            host,
            port,
            database,
            username,
            password,
        );

        if self.ssh_enabled {
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
            connection = connection.with_ssh(
                ssh_host.to_string(),
                ssh_port,
                ssh_user.to_string(),
                optional_value(&self.ssh_password_input, cx),
                optional_value(&self.ssh_private_key_input, cx),
                optional_value(&self.ssh_private_key_password_input, cx),
            );
        }

        connection.environment_type = environment_type;
        connection.ssl_mode = self.ssl_mode_select.read(cx).selected_value().cloned();
        connection.ssl_key_path = optional_value(&self.ssl_key_input, cx);
        connection.ssl_cert_path = optional_value(&self.ssl_cert_input, cx);
        connection.ssl_ca_cert_path = optional_value(&self.ssl_ca_cert_input, cx);
        Some(connection)
    }
}

fn optional_value(input: &Entity<InputState>, cx: &App) -> Option<String> {
    let value = input.read(cx).value();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn render_base_fields(form: &ServerForm) -> AnyElement {
    v_flex()
        .gap_3()
        .child(
            h_flex()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child(form.spec.host_label))
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

/// Render the form for `spec`'s backend. `select` picks the matching
/// `ServerForm` out of the modal so the click listeners can mutate it.
pub(super) fn render(
    select: fn(&mut NewConnectionModal) -> &mut ServerForm,
    form: &ServerForm,
    cx: &mut Context<NewConnectionModal>,
) -> AnyElement {
    let prefix = form.spec.id_prefix;
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
            Button::new(format!("{prefix}-ssl-advanced-toggle"))
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
                .on_click(cx.listener(move |modal, _event, _window, cx| {
                    select(modal).toggle_ssl_advanced();
                    cx.notify();
                })),
        )
        .when(ssl_advanced_expanded, |this| {
            this.child(render_ssl_advanced_fields(
                prefix,
                &form.ssl_key_input,
                &form.ssl_cert_input,
                &form.ssl_ca_cert_input,
                cx,
            ))
        })
        .child(
            div().child(
                h_flex().gap_2().items_center().child(
                    Switch::new(format!("{prefix}-ssh-enabled-switch"))
                        .checked(ssh_enabled)
                        .label("SSH")
                        .on_click(cx.listener(move |modal, _checked, _window, cx| {
                            select(modal).toggle_ssh();
                            cx.notify();
                        })),
                ),
            ),
        )
        .when(ssh_enabled, |this| {
            this.child(render_ssh_section(
                prefix,
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
