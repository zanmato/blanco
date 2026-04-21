use blanco_ui::IconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, ElementId, IntoElement, ParentElement, Render, SharedString, StyleRefinement, Styled,
    Subscription, Window, div, px, rems,
};
use gpui_component::{
    ActiveTheme, Icon, StyledExt as _,
    clipboard::Clipboard,
    h_flex,
    text::{TextView, TextViewStyle},
    v_flex,
};

use super::chat_types::{MessageMetadata, MessageRole};

pub struct ChatMessageState {
    pub id: ElementId,
    pub message: SharedString,
    pub role: MessageRole,
    pub metadata: Option<MessageMetadata>,
    _subscriptions: Vec<Subscription>,
}

impl ChatMessageState {
    pub fn new(
        id: usize,
        message: String,
        role: MessageRole,
        metadata: Option<MessageMetadata>,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: ("chat-message-", id).into(),
            message: message.into(),
            role,
            metadata,
            _subscriptions: Vec::new(),
        }
    }
}

impl Render for ChatMessageState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .font_family(cx.theme().font_family.clone())
            .child(match self.role {
                MessageRole::Assistant => {
                    // Assistant: No padding, no background, just markdown content
                    let id = self.id.clone();
                    let token_info = self.metadata.as_ref().and_then(|m| {
                        let total = m.tokens_used?;
                        let prompt = m.prompt_tokens.unwrap_or(0);
                        let completion = m.completion_tokens.unwrap_or(0);
                        Some(format!(
                            "{} tokens ({} in / {} out)",
                            total, prompt, completion
                        ))
                    });
                    div()
                        .child(
                            TextView::markdown(self.id.clone(), self.message.clone())
                                .text_sm()
                                .scrollable(false)
                                .selectable(true)
                                .style(
                                    TextViewStyle::default()
                                        .paragraph_gap(rems(1.))
                                        .heading_font_size(|level, rem_size| match level {
                                            1..=3 => rem_size * 1,
                                            4 => rem_size * 0.9,
                                            _ => rem_size * 0.8,
                                        })
                                        .code_block(StyleRefinement::default().text_size(px(11.))),
                                )
                                .code_block_actions(move |code_block, _window, _cx| {
                                    let code = code_block.code();
                                    let id = id.clone();

                                    h_flex()
                                        .gap_1()
                                        .child(Clipboard::new((id, "copy")).value(code.clone()))
                                }),
                        )
                        .when_some(token_info, |el, info| {
                            el.child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(info),
                            )
                        })
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
                            TextView::markdown(self.id.clone(), self.message.clone())
                                .text_sm()
                                .scrollable(false)
                                .selectable(true)
                                .style(
                                    TextViewStyle::default()
                                        .paragraph_gap(rems(1.))
                                        .heading_font_size(|level, rem_size| match level {
                                            1..=3 => rem_size * 1,
                                            4 => rem_size * 0.9,
                                            _ => rem_size * 0.8,
                                        })
                                        .code_block(StyleRefinement::default().text_size(px(11.))),
                                ),
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
                                    TextView::markdown(self.id.clone(), self.message.clone())
                                        .text_sm()
                                        .scrollable(false)
                                        .selectable(false),
                                ),
                        )
                }
            })
    }
}
