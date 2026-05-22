use blanco_ui::IconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, ElementId, EventEmitter, IntoElement, ParentElement, Render, SharedString,
    StyleRefinement, Styled, Subscription, Window, div, px, rems,
};
use gpui_component::{
    ActiveTheme, Icon, Sizable, StyledExt as _,
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    h_flex,
    text::{TextView, TextViewStyle},
    v_flex,
};

use super::chat_types::{ApprovalState, MessageMetadata, MessageRole};

fn format_tokens(count: u32) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}T", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

#[derive(Clone, Debug)]
pub struct ApprovalEvent {
    pub tool_call_id: String,
    pub approved: bool,
}

pub struct ChatMessageState {
    pub id: ElementId,
    pub message: SharedString,
    pub role: MessageRole,
    pub metadata: Option<MessageMetadata>,
    pub approval_state: Option<ApprovalState>,
    pub tool_result: Option<SharedString>,
    pub hidden: bool,
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
        let approval_state = metadata
            .as_ref()
            .and_then(|m| m.approval.as_ref())
            .map(|a| a.state.clone());
        Self {
            id: ("chat-message-", id).into(),
            message: message.into(),
            role,
            metadata,
            approval_state,
            tool_result: None,
            hidden: false,
            _subscriptions: Vec::new(),
        }
    }

    pub fn set_tool_result(&mut self, result: String) {
        self.tool_result = Some(result.into());
    }
}

impl EventEmitter<ApprovalEvent> for ChatMessageState {}

fn text_view_style() -> TextViewStyle {
    TextViewStyle::default()
        .paragraph_gap(rems(0.5))
        .heading_font_size(|level, rem_size| match level {
            1..=3 => rem_size * 1,
            4 => rem_size * 0.9,
            _ => rem_size * 0.8,
        })
        .code_block(StyleRefinement::default().text_size(px(11.)).my_0())
}

impl Render for ChatMessageState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.hidden {
            return v_flex();
        }

        v_flex()
            .w_full()
            .min_w_0()
            .font_family(cx.theme().font_family.clone())
            .child(match self.role {
                MessageRole::Assistant => {
                    let id = self.id.clone();
                    let token_info = self.metadata.as_ref().and_then(|m| {
                        let total = m.tokens_used?;
                        let prompt = m.prompt_tokens.unwrap_or(0);
                        let completion = m.completion_tokens.unwrap_or(0);
                        Some(format!(
                            "{} tokens ({} in / {} out)",
                            format_tokens(total),
                            format_tokens(prompt),
                            format_tokens(completion)
                        ))
                    });
                    div()
                        .child(
                            TextView::markdown(self.id.clone(), self.message.clone())
                                .text_sm()
                                .scrollable(false)
                                .selectable(true)
                                .style(text_view_style())
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
                MessageRole::ToolRequest => {
                    let approval = self.metadata.as_ref().and_then(|m| m.approval.clone());
                    let current_state = self.approval_state.clone();

                    let summary_text = approval
                        .as_ref()
                        .map(|a| a.arguments_preview.clone())
                        .unwrap_or_else(|| self.message.to_string());

                    let status_icon = match &current_state {
                        Some(ApprovalState::Approved) => Some(
                            Icon::new(IconName::CircleCheck)
                                .size(px(14.))
                                .text_color(cx.theme().green),
                        ),
                        Some(ApprovalState::Denied) => Some(
                            Icon::new(IconName::CircleX)
                                .size(px(14.))
                                .text_color(cx.theme().red),
                        ),
                        _ => None,
                    };

                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    Icon::new(IconName::Wrench)
                                        .text_color(cx.theme().muted_foreground)
                                        .size(px(16.)),
                                )
                                .when_some(status_icon, |el, icon| el.child(icon))
                                .child(div().text_sm().child(summary_text)),
                        )
                        .when(!self.message.is_empty(), |el| {
                            el.child(
                                div().ml_6().min_w_0().child(
                                    TextView::markdown(
                                        (self.id.clone(), "tool-req-view"),
                                        self.message.clone(),
                                    )
                                    .text_sm()
                                    .scrollable(false)
                                    .selectable(true)
                                    .style(text_view_style()),
                                ),
                            )
                        })
                        .when(
                            current_state.as_ref() == Some(&ApprovalState::Pending),
                            |el| {
                                let approval_for_approve = approval.clone();
                                let approval_for_deny = approval.clone();
                                let approve_id: ElementId = (self.id.clone(), "approve").into();
                                let deny_id: ElementId = (self.id.clone(), "deny").into();

                                el.child(
                                    h_flex()
                                        .ml_6()
                                        .mt_1()
                                        .gap_2()
                                        .child(
                                            Button::new(approve_id)
                                                .label("Allow")
                                                .success()
                                                .xsmall()
                                                .on_click(cx.listener(
                                                    move |this, _event, _window, cx| {
                                                        if let Some(a) = &approval_for_approve {
                                                            cx.emit(ApprovalEvent {
                                                                tool_call_id: a
                                                                    .tool_call_id
                                                                    .clone(),
                                                                approved: true,
                                                            });
                                                            this.approval_state =
                                                                Some(ApprovalState::Approved);
                                                            cx.notify();
                                                        }
                                                    },
                                                )),
                                        )
                                        .child(
                                            Button::new(deny_id)
                                                .label("Deny")
                                                .danger()
                                                .xsmall()
                                                .on_click(cx.listener(
                                                    move |this, _event, _window, cx| {
                                                        if let Some(a) = &approval_for_deny {
                                                            cx.emit(ApprovalEvent {
                                                                tool_call_id: a
                                                                    .tool_call_id
                                                                    .clone(),
                                                                approved: false,
                                                            });
                                                            this.approval_state =
                                                                Some(ApprovalState::Denied);
                                                            cx.notify();
                                                        }
                                                    },
                                                )),
                                        ),
                                )
                            },
                        )
                }
                MessageRole::Tool => h_flex()
                    .gap_2()
                    .items_start()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(IconName::Wrench)
                            .text_color(cx.theme().muted_foreground)
                            .size(px(16.)),
                    )
                    .child(
                        div().min_w_0().flex_1().child(
                            TextView::markdown(
                                (self.id.clone(), "tool-view"),
                                self.message.clone(),
                            )
                            .text_sm()
                            .scrollable(false)
                            .selectable(true)
                            .style(text_view_style()),
                        ),
                    ),
                MessageRole::User => div()
                    .px_3()
                    .py_2()
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
                            .style(text_view_style()),
                    ),
                MessageRole::System => v_flex()
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
                    ),
            })
    }
}
