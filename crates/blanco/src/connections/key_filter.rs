//! Filtering and paging the keys of a key/value connection in the tree.

use gpui::{AppContext as _, Context, ParentElement as _, Styled as _, Window};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
    v_flex,
};

use super::ConnectionsPanel;
use super::data_loading::KeyFilter;
use crate::result_ext::ResultExt as _;

impl ConnectionsPanel {
    /// The key filter of a connection, the default when none was set.
    pub(super) fn key_filter(&self, connection_id: i64) -> KeyFilter {
        self.key_filters
            .get(&connection_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether more keys match the connection's filter than are listed.
    pub(super) fn keys_truncated(&self, connection_id: i64) -> bool {
        self.database_metadata
            .get(&connection_id)
            .is_some_and(|metadata| metadata.keys_truncated)
    }

    /// List keys matching `pattern`, from the first page, and reload.
    pub(super) fn set_key_pattern(
        &mut self,
        connection_id: i64,
        pattern: String,
        cx: &mut Context<Self>,
    ) {
        let pattern = pattern.trim();
        let filter = KeyFilter {
            pattern: if pattern.is_empty() { "*" } else { pattern }.to_string(),
            limit: KeyFilter::PAGE,
        };
        self.key_filters.insert(connection_id, filter);
        self.refresh_connection(connection_id, cx);
    }

    /// List one more page of keys and reload.
    pub(super) fn load_more_keys(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        let mut filter = self.key_filter(connection_id);
        filter.limit += KeyFilter::PAGE;
        self.key_filters.insert(connection_id, filter);
        self.refresh_connection(connection_id, cx);
    }

    /// Ask for a key pattern and list the keys matching it.
    pub(super) fn open_key_filter_dialog(
        &mut self,
        connection_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.key_filter(connection_id).pattern;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("user:*")
                .default_value(current)
        });
        let panel = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let input = input.clone();
            let panel = panel.clone();
            dialog
                .title("Filter Keys")
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            "Keys matching this pattern are listed. * matches any text, \
                             ? one character, [abc] one of the characters.",
                        )
                        .child(Input::new(&input)),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("key-filter-cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("key-filter-apply")
                                    .primary()
                                    .label("Filter")
                                    .on_click(move |_, _, cx| {
                                        let pattern = input.read(cx).value().to_string();
                                        panel
                                            .update(cx, |panel, cx| {
                                                panel.set_key_pattern(connection_id, pattern, cx)
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }
}
