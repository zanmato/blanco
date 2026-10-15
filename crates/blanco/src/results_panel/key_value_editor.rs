//! The editor an hstore cell expands into: one row per key and value, where
//! pairs are added, edited and removed and a value may be NULL.

use std::collections::HashSet;

use blanco_core::hstore::{self, Pair};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme, Disableable as _, Sizable as _, h_flex, v_flex};

use blanco_ui::IconName;

pub enum KeyValueEditorEvent {
    /// The pairs changed; the value is their hstore text.
    Changed(String),
    Close,
}

struct PairRow {
    key: Entity<InputState>,
    value: Entity<InputState>,
    is_null: bool,
}

pub struct KeyValueEditor {
    rows: Vec<PairRow>,
    editable: bool,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<KeyValueEditorEvent> for KeyValueEditor {}

impl KeyValueEditor {
    pub fn new(
        pairs: Vec<Pair>,
        editable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self {
            rows: Vec::new(),
            editable,
            focus_handle: cx.focus_handle(),
            _subscriptions: Vec::new(),
        };
        for (key, value) in pairs {
            editor.push_row(&key, value.as_deref(), window, cx);
        }
        editor
    }

    fn push_row(
        &mut self,
        key: &str,
        value: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Key")
                .default_value(key)
        });
        let value_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(value.unwrap_or_default())
        });
        for input in [&key, &value_input] {
            self._subscriptions
                .push(cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        this.changed(cx);
                    }
                }));
        }
        self.rows.push(PairRow {
            key,
            value: value_input,
            is_null: value.is_none(),
        });
    }

    /// The pairs as the rows hold them.
    pub fn pairs(&self, cx: &gpui::App) -> Vec<Pair> {
        self.rows
            .iter()
            .map(|row| {
                let key = row.key.read(cx).value().to_string();
                let value = (!row.is_null).then(|| row.value.read(cx).value().to_string());
                (key, value)
            })
            .collect()
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        if self.editable {
            cx.emit(KeyValueEditorEvent::Changed(hstore::format(
                &self.pairs(cx),
            )));
        }
        cx.notify();
    }

    pub fn add_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push_row("", Some(""), window, cx);
        if let Some(row) = self.rows.last() {
            row.key.focus_handle(cx).focus(window, cx);
        }
        self.changed(cx);
    }

    pub fn remove_pair(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.rows.len() {
            self.rows.remove(ix);
            self.changed(cx);
        }
    }

    pub fn set_null(&mut self, ix: usize, is_null: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(ix) {
            row.is_null = is_null;
            self.changed(cx);
        }
    }

    /// The keys that appear more than once. The server keeps only one of
    /// them.
    fn duplicate_keys(&self, cx: &gpui::App) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut duplicates = Vec::new();
        for row in &self.rows {
            let key = row.key.read(cx).value().to_string();
            if !seen.insert(key.clone()) && !duplicates.contains(&key) {
                duplicates.push(key);
            }
        }
        duplicates
    }
}

impl Focusable for KeyValueEditor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KeyValueEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editable = self.editable;
        let duplicates = self.duplicate_keys(cx);
        let summary = match self.rows.len() {
            1 => "1 pair".to_string(),
            count => format!("{count} pairs"),
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .font_family(cx.theme().font_family.clone())
            .child(
                h_flex()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(summary),
                    )
                    .when(editable, |this| {
                        this.child(
                            Button::new("key-value-add")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Plus)
                                .label("Add Pair")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_pair(window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("key-value-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Minimize)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(KeyValueEditorEvent::Close);
                            })),
                    ),
            )
            .when(!duplicates.is_empty(), |this| {
                this.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(format!(
                            "Repeated keys, of which the server keeps one: {}",
                            duplicates
                                .iter()
                                .map(|key| format!("\"{key}\""))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .gap_1()
                    .overflow_y_scrollbar()
                    .when(self.rows.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No pairs"),
                        )
                    })
                    .children(self.rows.iter().enumerate().map(|(ix, row)| {
                        h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&row.key).small().disabled(!editable)),
                            )
                            .child(
                                div().flex_1().child(
                                    Input::new(&row.value)
                                        .small()
                                        .disabled(!editable || row.is_null),
                                ),
                            )
                            .child(
                                Checkbox::new(("key-value-null", ix))
                                    .label("NULL")
                                    .checked(row.is_null)
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        this.set_null(ix, *checked, cx);
                                    })),
                            )
                            .when(editable, |this| {
                                this.child(
                                    Button::new(("key-value-remove", ix))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Remove Pair")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_pair(ix, cx);
                                        })),
                                )
                            })
                            .w_full()
                            .min_w(px(0.))
                    })),
            )
    }
}

#[cfg(test)]
#[path = "key_value_editor_test.rs"]
mod tests;
