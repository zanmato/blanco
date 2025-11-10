use gpui::{
    actions, div, prelude::FluentBuilder, px, App, AppContext, Axis, Context, Entity, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, Styled, Subscription, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    v_flex, ActiveTheme, Disableable, Icon, Sizable, StyledExt as _,
};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::chat_types::MessageRole;

use super::chat_message_view::ChatMessageView;
use super::chat_session::ChatSession;
use super::chat_types::{ChatEvent, ChatMessage, SqlContext};
use blanco_core::chat_provider::{ChatProvider, ProviderError};
use blanco_ui::IconName;

actions!(agent_chat, [SendMessage, ClearChat]);

pub struct ChatPanel {
    pub focus_handle: FocusHandle,
    pub session: Entity<ChatSession>,
    pub input_state: Entity<InputState>,
    pub messages: Vec<ChatMessageView>,
    pub _subscriptions: Vec<Subscription>,
    pub is_loading: bool,
    pub tab_id: usize,
}

impl ChatPanel {
    pub fn new(
        tab_id: usize,
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        provider_name: String,
        model_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = cx.new(|_cx| ChatSession::new(provider, provider_name, model_name));

        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line()
                .rows(3)
                .auto_grow(2, 6) // Auto-grow between 2 and 6 rows
                .placeholder("Ask me anything about your SQL query...")
        });

        let mut subscriptions = Vec::new();

        // Subscribe to session events
        let subscription = cx.subscribe(&session, |panel, _session, event, cx| {
            match event {
                ChatEvent::MessageAdded { message } => {
                    log::info!("Message added! {:?}", message);
                    let message = message.clone();
                    let message_view = ChatMessageView::new(
                        panel.messages.len(),
                        match message.role {
                            MessageRole::Tool => message.tool_call_id.unwrap_or("tool".to_owned()),
                            _ => message.content,
                        },
                        message.role.clone(),
                    );
                    panel.messages.push(message_view);
                    panel.scroll_to_bottom(cx);

                    cx.notify();
                }
                ChatEvent::StreamStarted { message_id } => {
                    // Handle stream start
                    log::debug!("Chat stream started: {}", message_id);
                    panel.is_loading = true;
                    cx.notify();
                }
                ChatEvent::StreamUpdate {
                    message_id: _,
                    content: _,
                } => {
                    // Handle stream updates - scroll to show new content
                    panel.scroll_to_bottom(cx);
                    cx.notify();
                }
                ChatEvent::StreamCompleted {
                    message_id: _,
                    final_content: _,
                } => {
                    // Handle stream completion
                    panel.is_loading = false;
                    panel.scroll_to_bottom(cx);
                    cx.notify();
                }
                ChatEvent::Error { message } => {
                    log::error!("Chat error: {}", message);
                    panel.is_loading = false;
                    cx.notify();
                }
                ChatEvent::SessionStarted { provider, model } => {
                    log::info!("Chat session started: {} ({})", provider, model);
                    cx.notify();
                }
                ChatEvent::SessionCleared => {
                    panel.messages.clear();
                    cx.notify();
                }
            }
        });

        subscriptions.push(subscription);

        Self {
            focus_handle: cx.focus_handle(),
            session,
            input_state,
            messages: Vec::new(),
            _subscriptions: subscriptions,
            is_loading: false,
            tab_id,
        }
    }

    pub fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Get the current input text and convert to string early to avoid borrow issues
        let input_text = self.input_state.read(cx).text().to_string();

        if input_text.trim().is_empty() {
            return;
        }

        // Clear the input
        self.input_state.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });

        // Add user message to session
        self.session.update(cx, |session, cx| {
            session.send_message(input_text.clone(), cx).detach();
        });
    }

    pub fn clear_chat(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.clear_messages();
            cx.emit(ChatEvent::SessionCleared);
        });
    }

    pub fn update_sql_context(&mut self, context: SqlContext, cx: &mut Context<Self>) {
        self.session.update(cx, |session, _cx| {
            session.update_sql_context(context);
        });
    }

    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        // Schedule scroll to bottom after render
        cx.spawn(async move |_, _cx| {
            // Small delay to ensure content is rendered
            gpui::Timer::after(Duration::from_millis(50)).await;
            // Note: In a real implementation, you'd use the scroll handle
            // self.scroll_handle.scroll_to(ScrollPosition { offset: px(f32::MAX), anchor: Anchor::End });
        })
        .detach();
    }

    fn on_send_message(&mut self, _: &SendMessage, window: &mut Window, cx: &mut Context<Self>) {
        self.send_message(window, cx);
    }

    fn on_clear_chat(&mut self, _: &ClearChat, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_chat(window, cx);
    }
}

impl Focusable for ChatPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .border_t_1()
            .border_color(cx.theme().border)
            // Header
            .child(
                h_flex()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.3))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Bot)
                                    .size(px(16.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(div().text_sm().font_medium().child("SQL Assistant")),
                    )
                    .child(
                        h_flex().items_center().gap_2().child(
                            Button::new("clear-chat")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.on_clear_chat(&ClearChat, window, cx)
                                })),
                        ),
                    ),
            )
            // Messages area
            .child(
                div().flex_1().min_h_0().child(
                    v_flex()
                        .p_3()
                        .gap_4()
                        .size_full()
                        // Messages
                        .children(
                            self.messages
                                .iter()
                                .map(|message_view| message_view.clone().into_any_element()),
                        )
                        // Loading indicator
                        .when(self.is_loading, |this| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Thinking..."),
                            )
                        })
                        .scrollable(Axis::Vertical),
                ),
            )
            // Input area
            .child(
                v_flex()
                    .p_3()
                    .bg(cx
                        .theme()
                        .highlight_theme
                        .style
                        .editor_background
                        .unwrap_or(cx.theme().background))
                    .gap_3()
                    .child(
                        // Text input container with no borders
                        div()
                            .relative()
                            .bg(cx
                                .theme()
                                .highlight_theme
                                .style
                                .editor_background
                                .unwrap_or(cx.theme().background))
                            .rounded_lg()
                            .border_0()
                            .text_size(px(13.0)) // Smaller font size for the input text
                            .child(
                                Input::new(&self.input_state)
                                    .disabled(self.is_loading)
                                    .bordered(false)
                                    .p_0()
                                    .bg(cx
                                        .theme()
                                        .highlight_theme
                                        .style
                                        .editor_background
                                        .unwrap_or(cx.theme().background)),
                            )
                            // Send button positioned further to bottom right corner
                            .child(
                                div().absolute().bottom_1().right_1().child(
                                    Button::new("send-message")
                                        .icon(IconName::ArrowUp)
                                        .primary()
                                        .xsmall()
                                        .disabled(self.is_loading)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.send_message(window, cx);
                                        })),
                                ),
                            ),
                    ),
            )
    }
}
