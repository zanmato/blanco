use gpui::{div, px, App, IntoElement, ParentElement, RenderOnce, Styled, Window};
use gpui_component::{h_flex, text::TextView, v_flex, ActiveTheme, Icon, IconName, StyledExt as _};

use super::chat_types::MessageRole;

#[derive(IntoElement, Clone)]
pub struct ChatMessageView {
    pub id: usize,
    pub message: String,
    pub role: MessageRole,
}

impl ChatMessageView {
    pub fn new(id: usize, message: String, role: MessageRole) -> Self {
        Self {
            id,
            message: message.clone(),
            role,
        }
    }
}

impl RenderOnce for ChatMessageView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex().w_full().child(match self.role {
            MessageRole::Assistant => {
                // Assistant: No padding, no background, just markdown content
                div().child(TextView::markdown(
                    ("chat-message-content", self.id),
                    self.message,
                    window,
                    cx,
                ))
            }
            MessageRole::Tool => {
                // Tool: No padding, no background, icon + text (no markdown), muted colors
                h_flex()
                    .gap_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(IconName::Info)
                            .text_color(cx.theme().muted_foreground)
                            .size(px(16.)),
                    )
                    .child(div().text_sm().child(self.message))
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
                    .child(TextView::markdown(
                        ("chat-message-content", self.id),
                        self.message,
                        window,
                        cx,
                    ))
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
                            .child(TextView::markdown(
                                ("chat-message-content", self.id),
                                self.message,
                                window,
                                cx,
                            )),
                    )
            }
        })
    }
}
