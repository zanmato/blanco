use gpui::{AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div};
use gpui_component::{
    IconName, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    v_flex,
};

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
        prompt: Some("Select file".into()),
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
            "SSL Key Path",
            ssl_key_input,
            cx,
        ))
        .child(file_picker_input(
            &format!("{prefix}-ssl-cert-picker"),
            "SSL Cert Path",
            ssl_cert_input,
            cx,
        ))
        .child(file_picker_input(
            &format!("{prefix}-ssl-ca-cert-picker"),
            "SSL CA Cert Path",
            ssl_ca_cert_input,
            cx,
        ))
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
                        .child(div().text_sm().child("Host"))
                        .child(Input::new(ssh_host_input)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child("Port"))
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
                        .child(div().text_sm().child("User"))
                        .child(Input::new(ssh_user_input)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child("Password"))
                        .child(Input::new(ssh_password_input).mask_toggle()),
                ),
        )
        .child(
            h_flex()
                .gap_3()
                .child(v_flex().flex_1().child(file_picker_input(
                    &format!("{prefix}-ssh-key-picker"),
                    "Private key path",
                    ssh_private_key_input,
                    cx,
                )))
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(div().text_sm().child("Private key password"))
                        .child(Input::new(ssh_private_key_password_input).mask_toggle()),
                ),
        )
}
