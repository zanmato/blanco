//! The dialog `DatabaseService` asks through for a secret that is not stored:
//! a connection set to ask on connect, or a server that wants a password the
//! connection does not have.
//!
//! The service runs on the tokio runtime, so like `KeyringSecretStore` this
//! bridges to one foreground task, which shows one dialog at a time in the
//! active window and replies once it is answered or dismissed.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gpui::{
    App, AppContext as _, Entity, Focusable as _, ParentElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputEvent, InputState},
    v_flex,
};

use database::{SecretKind, SecretPrompt, SecretPromptRequest};

struct PendingPrompt {
    request: SecretPromptRequest,
    reply: smol::channel::Sender<Option<String>>,
}

/// The dialog-backed [`SecretPrompt`]. Cheap to clone and `Send`; the dialog
/// itself is opened by the foreground task [`DialogSecretPrompt::install`]
/// spawns.
#[derive(Clone)]
pub struct DialogSecretPrompt {
    sender: smol::channel::Sender<PendingPrompt>,
}

impl DialogSecretPrompt {
    pub fn install(cx: &mut App) -> Arc<Self> {
        let (sender, receiver) = smol::channel::unbounded::<PendingPrompt>();
        cx.spawn(async move |cx| {
            while let Ok(pending) = receiver.recv().await {
                let (answer_sender, answer) = smol::channel::bounded(1);
                let window = cx.update(|cx| {
                    cx.active_window()
                        .or_else(|| cx.windows().into_iter().next())
                });
                let shown = window.is_some_and(|window| {
                    window
                        .update(cx, |_, window, cx| {
                            open_prompt(pending.request.clone(), answer_sender.clone(), window, cx)
                        })
                        .is_ok()
                });
                // Wait for this dialog before showing the next, so prompts
                // never stack.
                let value = if shown {
                    answer.recv().await.ok().flatten()
                } else {
                    None
                };
                pending.reply.try_send(value).ok();
            }
        })
        .detach();
        Arc::new(Self { sender })
    }
}

#[async_trait]
impl SecretPrompt for DialogSecretPrompt {
    async fn ask(&self, request: SecretPromptRequest) -> Result<Option<String>> {
        let (reply, receiver) = smol::channel::bounded(1);
        self.sender
            .send(PendingPrompt { request, reply })
            .await
            .map_err(|_| anyhow::anyhow!("Blanco's window is not running"))?;
        Ok(receiver.recv().await.ok().flatten())
    }
}

/// The dialog's title and the sentence under it.
pub(crate) fn prompt_text(request: &SecretPromptRequest) -> (String, String) {
    let name = &request.connection_name;
    let account = &request.account;
    match request.kind {
        SecretKind::Password => (
            format!("Password for {name}"),
            format!("Enter the password for {account}."),
        ),
        SecretKind::SshPassword => (
            format!("SSH Password for {name}"),
            format!("Enter the SSH password for {account}."),
        ),
        SecretKind::SshPrivateKeyPassword => (
            format!("Key Passphrase for {name}"),
            format!("Enter the passphrase for the key {account}."),
        ),
    }
}

fn open_prompt(
    request: SecretPromptRequest,
    answer: smol::channel::Sender<Option<String>>,
    window: &mut Window,
    cx: &mut App,
) {
    let input: Entity<InputState> = cx.new(|cx| {
        InputState::new(window, cx)
            .masked(true)
            .placeholder(capitalized(request.kind.label()))
    });
    let (title, description) = prompt_text(&request);
    let retry_reason = request.retry_reason.clone();
    let submitted = Rc::new(Cell::new(false));

    // Enter in the field confirms, as the Connect button does.
    let enter_subscription = Rc::new(window.subscribe(&input, cx, {
        let answer = answer.clone();
        let submitted = submitted.clone();
        move |input, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                submitted.set(true);
                answer
                    .try_send(Some(input.read(cx).value().to_string()))
                    .ok();
                window.close_dialog(cx);
            }
        }
    }));

    window.open_dialog(cx, {
        let input = input.clone();
        move |dialog, _, cx| {
            let _keep_subscribed = &enter_subscription;
            let answer_ok = answer.clone();
            let answer_close = answer.clone();
            let submitted_ok = submitted.clone();
            let submitted_close = submitted.clone();
            let input_ok = input.clone();
            dialog
                .title(title.clone())
                .w(px(420.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(div().text_sm().child(description.clone()))
                        .when_some(retry_reason.clone(), |this, reason| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(format!("Not accepted: {reason}")),
                            )
                        })
                        .child(Input::new(&input).mask_toggle())
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Kept in memory until you disconnect."),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(
                                Button::new("secret-prompt-cancel")
                                    .label("Cancel")
                                    .outline(),
                            ),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("secret-prompt-connect")
                                    .primary()
                                    .label("Connect"),
                            ),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    submitted_ok.set(true);
                    answer_ok
                        .try_send(Some(input_ok.read(cx).value().to_string()))
                        .ok();
                    true
                })
                .on_close(move |_, _, _| {
                    if !submitted_close.get() {
                        answer_close.try_send(None).ok();
                    }
                })
        }
    });
    input.focus_handle(cx).focus(window, cx);
}

fn capitalized(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_names_the_connection_and_account() {
        let request = SecretPromptRequest {
            connection_id: 1,
            connection_name: "Billing (prod)".to_string(),
            kind: SecretKind::Password,
            account: "alice@db.example.com".to_string(),
            retry_reason: None,
        };
        assert_eq!(
            prompt_text(&request),
            (
                "Password for Billing (prod)".to_string(),
                "Enter the password for alice@db.example.com.".to_string()
            )
        );
        assert_eq!(
            capitalized(SecretKind::SshPrivateKeyPassword.label()),
            "SSH key passphrase"
        );
    }
}
