use gpui::{
    div, App, IntoElement, ParentElement, RenderOnce, Styled, Window,
};
use gpui_component::{v_flex, ActiveTheme, StyledExt as _, text::TextView};

use super::chat_types::MessageRole;

#[derive(IntoElement, Clone)]
pub struct ChatMessageView {
    pub message: String,
    pub role: MessageRole,
}

impl ChatMessageView {
    pub fn new(message: &str, role: MessageRole) -> Self {
        Self {
            message: message.to_owned(),
            role,
        }
    }
}

impl RenderOnce for ChatMessageView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(match self.role {
                        MessageRole::User => "You",
                        MessageRole::Assistant => "Agent",
                        MessageRole::System => "System",
                        MessageRole::Tool => "Tool Result",
                    }),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(match self.role {
                        MessageRole::User => cx.theme().primary,
                        MessageRole::Tool => cx.theme().accent.opacity(0.1),
                        _ => cx.theme().muted,
                    })
                    .text_color(match self.role {
                        MessageRole::User => cx.theme().primary_foreground,
                        MessageRole::Tool => cx.theme().foreground,
                        _ => cx.theme().foreground,
                    })
                    .child(
                        // Use TextView::markdown for proper markdown rendering
                        TextView::markdown(
                            "chat-message-content", // Unique ID
                            self.message,
                            window,
                            cx,
                        )
                    ),
            )
    }
}
