use blanco_ui::IconName;
use gpui::{
    Context, ElementId, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Window, div, px,
};
use gpui_component::{
    ActiveTheme, Icon, StyledExt as _, clipboard::Clipboard, h_flex, text::TextView, v_flex,
};

use super::chat_types::MessageRole;

pub struct ChatMessageState {
    pub id: ElementId,
    pub message: SharedString,
    pub role: MessageRole,
    _subscriptions: Vec<Subscription>,
}

impl ChatMessageState {
    pub fn new(id: usize, message: String, role: MessageRole, _cx: &mut Context<Self>) -> Self {
        Self {
            id: ("chat-message-", id).into(),
            message: message.into(),
            role,
            _subscriptions: Vec::new(),
        }
    }
}

impl Render for ChatMessageState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().w_full().child(match self.role {
            MessageRole::Assistant => {
                // Assistant: No padding, no background, just markdown content
                div().child(
                    TextView::markdown(self.id.clone(), self.message.clone(), window, cx)
                        .text_sm()
                        .scrollable(false)
                        .selectable(true)
                        .code_block_actions(move |code_block, _window, _cx| {
                            let code = code_block.code();

                            h_flex()
                                .gap_1()
                                .child(Clipboard::new(code.clone()).value(code.clone()))
                        }),
                )
            }
            MessageRole::Tool => {
                // Tool: No padding, no background, icon + text (no markdown), muted colors
                h_flex()
                    .gap_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(IconName::Wrench)
                            .text_color(cx.theme().muted_foreground)
                            .size(px(16.)),
                    )
                    .child(div().text_sm().child(self.message.clone()))
            }
            MessageRole::User => {
                // User: Padding, background, border, less rounding, markdown content
                div()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx
                        .theme()
                        .highlight_theme
                        .style
                        .editor_background
                        .unwrap_or(cx.theme().background))
                    .text_color(cx.theme().foreground)
                    .child(
                        TextView::markdown(self.id.clone(), self.message.clone(), window, cx)
                            .text_sm()
                            .scrollable(false)
                            .selectable(true),
                    )
            }
            MessageRole::System => {
                // System: Default styling (can be customized if needed)
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().muted_foreground)
                            .child("System"),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .bg(cx.theme().muted)
                            .text_color(cx.theme().foreground)
                            .child(
                                TextView::markdown(
                                    self.id.clone(),
                                    self.message.clone(),
                                    window,
                                    cx,
                                )
                                .text_sm()
                                .scrollable(false)
                                .selectable(false),
                            ),
                    )
            }
        })
    }
}
