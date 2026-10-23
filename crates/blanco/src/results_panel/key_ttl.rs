//! Setting and removing a Redis key's time to live from the key inspector.

use database::{DatabaseService, DatabaseServiceTrait as _};
use gpui::{AppContext as _, Context, ParentElement as _, SharedString, Styled as _, Window};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
    notification::NotificationType,
    v_flex,
};

use super::ResultsPanel;
use crate::result_ext::ResultExt as _;

impl ResultsPanel {
    /// Ask for a time to live in seconds and give it to `key`.
    pub(super) fn open_ttl_dialog(
        &mut self,
        key: String,
        current: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Seconds")
                .default_value(current.map(|ttl| ttl.to_string()).unwrap_or_default())
        });
        let panel = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let input = input.clone();
            let panel = panel.clone();
            let key = key.clone();
            dialog
                .title(format!("Set TTL of {key}"))
                .child(
                    v_flex()
                        .gap_2()
                        .child("Seconds until the key expires.")
                        .child(Input::new(&input)),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("ttl-cancel").label("Cancel").outline()),
                        )
                        .child(DialogAction::new().child(
                            Button::new("ttl-set").primary().label("Set TTL").on_click(
                                move |_, window, cx| {
                                    let value = input.read(cx).value();
                                    // EXPIRE deletes the key at once for zero or less.
                                    let Some(seconds) = value
                                        .trim()
                                        .parse::<i64>()
                                        .ok()
                                        .filter(|seconds| *seconds > 0)
                                    else {
                                        window.push_notification(
                                            (
                                                NotificationType::Error,
                                                "The TTL must be a whole number of seconds above 0",
                                            ),
                                            cx,
                                        );
                                        return;
                                    };
                                    let key = key.clone();
                                    panel
                                        .update(cx, |panel, cx| {
                                            panel.set_key_ttl(key, Some(seconds), window, cx)
                                        })
                                        .log_err();
                                },
                            ),
                        )),
                )
        });
    }

    /// Set `key`'s TTL, or remove it with `None`, then show the key again.
    pub(super) fn set_key_ttl(
        &mut self,
        key: String,
        ttl: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let db_service = DatabaseService::global(cx).clone();
        let connection_id = self.connection_id;
        let database_name = self.database_name.clone();
        cx.spawn_in(window, async move |panel, cx| {
            let result = async {
                let connection = db_service
                    .get_or_create_connection_by_id(connection_id, Some(&database_name))
                    .await?;
                connection.set_key_ttl(&key, ttl).await?;
                connection.inspect_key(Some(&database_name), &key).await
            }
            .await;
            panel
                .update_in(cx, |panel, window, cx| match result {
                    Ok(inspected) => {
                        for tab in &mut panel.result_tabs {
                            if tab.key_value.as_ref().is_some_and(|kv| kv.key == key) {
                                tab.key_value = Some(inspected.clone());
                            }
                        }
                        cx.notify();
                    }
                    Err(error) => window.push_notification(
                        (
                            NotificationType::Error,
                            SharedString::from(format!("Could not change the TTL: {error:#}")),
                        ),
                        cx,
                    ),
                })
                .log_err();
        })
        .detach();
    }
}
