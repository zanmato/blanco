use gpui::{
    App, AppContext, Context, ElementId, Entity, IntoElement, ParentElement, SharedString, Styled,
    Window, div,
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    v_flex,
};

use database::SecretKind;

use super::NewConnectionModal;

pub(super) fn make_input(
    window: &mut Window,
    cx: &mut Context<NewConnectionModal>,
    placeholder: &str,
    masked: bool,
    initial: Option<&str>,
) -> Entity<InputState> {
    let placeholder = placeholder.to_string();
    let initial = initial.map(|s| s.to_string());
    cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder(placeholder);
        if masked {
            input = input.masked(true);
        }
        if let Some(value) = initial
            && !value.is_empty()
        {
            input.set_value(value, window, cx);
        }
        input
    })
}

pub(super) fn pick_file_for_input(
    input: Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<NewConnectionModal>,
) {
    let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t!("connection_dialog.select_file").into()),
    });

    cx.spawn_in(window, async move |_, window| {
        if let Some(paths) = paths.await.ok()?.ok()?
            && let Some(path) = paths.first()
        {
            let path_str = path.to_str()?.to_string();
            window
                .update(|window, cx| {
                    input.update(cx, |input_state, cx| {
                        input_state.set_value(path_str, window, cx);
                    });
                })
                .ok();
        }
        Some(())
    })
    .detach();
}

pub(super) fn file_picker_input(
    button_id: &str,
    label: &str,
    input: &Entity<InputState>,
    cx: &mut Context<NewConnectionModal>,
) -> impl IntoElement {
    let input_for_click = input.clone();
    v_flex()
        .gap_2()
        .child(div().text_sm().child(label.to_string()))
        .child(
            Input::new(input).suffix(
                Button::new(button_id.to_string())
                    .ghost()
                    .icon(IconName::Folder)
                    .xsmall()
                    .on_click(cx.listener(
                        move |_modal: &mut NewConnectionModal, _event, window, cx| {
                            pick_file_for_input(input_for_click.clone(), window, cx);
                        },
                    )),
            ),
        )
}

pub(super) fn render_ssl_advanced_fields(
    prefix: &str,
    ssl_key_input: &Entity<InputState>,
    ssl_cert_input: &Entity<InputState>,
    ssl_ca_cert_input: &Entity<InputState>,
    cx: &mut Context<NewConnectionModal>,
) -> impl IntoElement {
    v_flex()
        .gap_3()
        .child(file_picker_input(
            &format!("{prefix}-ssl-key-picker"),
            &t!("connection_dialog.ssl_key_path"),
            ssl_key_input,
            cx,
        ))
        .child(file_picker_input(
            &format!("{prefix}-ssl-cert-picker"),
            &t!("connection_dialog.ssl_cert_path"),
            ssl_cert_input,
            cx,
        ))
        .child(file_picker_input(
            &format!("{prefix}-ssl-ca-cert-picker"),
            &t!("connection_dialog.ssl_ca_cert_path"),
            ssl_ca_cert_input,
            cx,
        ))
}

/// A masked secret field with an Ask on connect checkbox beside its label.
/// While the box is ticked the field is disabled: the secret is not stored
/// and is asked for each time the connection opens.
pub(super) fn secret_field(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    input: &Entity<InputState>,
    kind: SecretKind,
    ask_secrets: &[SecretKind],
    cx: &mut Context<NewConnectionModal>,
) -> impl IntoElement {
    let asked = ask_secrets.contains(&kind);
    v_flex()
        .flex_1()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .justify_between()
                .child(div().text_sm().child(label.into()))
                .child(
                    Checkbox::new(id)
                        .small()
                        .label(t!("connection_dialog.ask_on_connect").into_owned())
                        .checked(asked)
                        .on_click(cx.listener(move |modal, checked: &bool, window, cx| {
                            modal.set_ask_secret(kind, *checked, window, cx);
                        })),
                ),
        )
        .child(Input::new(input).mask_toggle().disabled(asked))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_ssh_section(
    prefix: &str,
    ssh_host_input: &Entity<InputState>,
    ssh_port_input: &Entity<InputState>,
    ssh_user_input: &Entity<InputState>,
    ssh_password_input: &Entity<InputState>,
    ssh_private_key_input: &Entity<InputState>,
    ssh_private_key_password_input: &Entity<InputState>,
    ask_secrets: &[SecretKind],
    ssh_jump_hosts_input: &Entity<InputState>,
    cx: &mut Context<NewConnectionModal>,
) -> impl IntoElement {
    v_flex()
        .gap_3()
        .child(
            h_flex()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child(t!("connection_dialog.host")))
                        .child(Input::new(ssh_host_input)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child(t!("connection_dialog.port")))
                        .child(Input::new(ssh_port_input)),
                ),
        )
        .child(
            h_flex()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child(t!("connection_dialog.user")))
                        .child(Input::new(ssh_user_input)),
                )
                .child(secret_field(
                    format!("{prefix}-ssh-password-ask"),
                    t!("connection_dialog.password"),
                    ssh_password_input,
                    SecretKind::SshPassword,
                    ask_secrets,
                    cx,
                )),
        )
        .child(
            h_flex()
                .gap_3()
                .child(v_flex().flex_1().child(file_picker_input(
                    &format!("{prefix}-ssh-key-picker"),
                    &t!("connection_dialog.private_key_path"),
                    ssh_private_key_input,
                    cx,
                )))
                .child(secret_field(
                    format!("{prefix}-ssh-key-password-ask"),
                    t!("connection_dialog.private_key_password"),
                    ssh_private_key_password_input,
                    SecretKind::SshPrivateKeyPassword,
                    ask_secrets,
                    cx,
                )),
        )
        .child(
            v_flex()
                .gap_2()
                .child(div().text_sm().child(t!("connection_dialog.jump_hosts")))
                .child(Input::new(ssh_jump_hosts_input)),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t!("connection_dialog.ssh_help")),
        )
}

/// What is wrong with the SSH section of a form, if anything. The SSH host
/// is checked here so the dialog can say so, rather than only failing to
/// save.
pub(super) fn validate_ssh_section(
    ssh_host_input: &Entity<InputState>,
    ssh_jump_hosts_input: &Entity<InputState>,
    cx: &App,
) -> Option<String> {
    let jump_hosts = ssh_jump_hosts_input.read(cx).value();
    if ssh_host_input.read(cx).value().trim().is_empty() {
        return Some(if jump_hosts.trim().is_empty() {
            t!("connection_dialog.errors.ssh_host_required").into_owned()
        } else {
            t!("connection_dialog.errors.ssh_host_required_with_jump_hosts").into_owned()
        });
    }
    database::ssh_config::parse_jump_hosts(&jump_hosts).err()
}

/// What is wrong with the host, port, database and user fields of a server
/// connection, if anything. `port_optional` says, given the host, whether
/// the port may be left empty.
pub(super) fn validate_server_fields(
    host_input: &Entity<InputState>,
    port_input: &Entity<InputState>,
    database_input: &Entity<InputState>,
    username_input: &Entity<InputState>,
    port_optional: impl Fn(&str) -> bool,
    cx: &App,
) -> Option<String> {
    let host = host_input.read(cx).value();
    let port = port_input.read(cx).value();
    if host.is_empty() {
        return Some(t!("connection_dialog.errors.host_required").into_owned());
    }
    if port.is_empty() && !port_optional(&host) {
        return Some(t!("connection_dialog.errors.port_required").into_owned());
    }
    if !port.is_empty() && port.parse::<u16>().is_err() {
        return Some(t!("connection_dialog.errors.port_invalid").into_owned());
    }
    if database_input.read(cx).value().is_empty() {
        return Some(t!("connection_dialog.errors.database_name_required").into_owned());
    }
    if username_input.read(cx).value().is_empty() {
        return Some(t!("connection_dialog.errors.username_required").into_owned());
    }
    None
}

/// The text of an optional field, `None` when it is left empty.
pub(super) fn optional_value(input: &Entity<InputState>, cx: &App) -> Option<String> {
    let value = input.read(cx).value();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}
