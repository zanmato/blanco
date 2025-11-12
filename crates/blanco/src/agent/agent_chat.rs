use gpui::{
    App, AppContext, Axis, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeybindingKeystroke, Keystroke, ParentElement, Render, Styled, Subscription,
    Window, actions, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    spinner::Spinner,
    v_flex,
};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::chat_types::MessageRole;

use super::chat_message_view::ChatMessageState;
use super::chat_session::ChatSession;
use super::chat_types::{ChatEvent, LoadingState, SqlContext};
use blanco_core::chat_provider::{ChatProvider, ProviderError};
use blanco_ui::IconName;

actions!(agent_chat, [SendMessage, ClearChat]);

pub struct ChatPanel {
    pub focus_handle: FocusHandle,
    pub session: Entity<ChatSession>,
    pub input_state: Entity<InputState>,
    pub messages: Vec<Entity<ChatMessageState>>,
    pub _subscriptions: Vec<Subscription>,
    pub loading_state: LoadingState,
    pub tab_id: usize,
    pub send_message_keystroke: KeybindingKeystroke,
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
                    let message_state = cx.new(|cx| {
                        ChatMessageState::new(
                            panel.messages.len(),
                            match message.role {
                                MessageRole::Tool => {
                                    message.tool_call_id.unwrap_or("tool".to_owned())
                                }
                                _ => message.content.into(),
                            },
                            message.role.clone(),
                            cx,
                        )
                    });
                    panel.messages.push(message_state);
                    panel.scroll_to_bottom(cx);

                    cx.notify();
                }
                ChatEvent::StreamStarted { message_id } => {
                    // Handle stream start
                    log::debug!("Chat stream started: {}", message_id);
                    panel.loading_state = LoadingState::Streaming;
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
                    panel.loading_state = LoadingState::Idle;
                    panel.scroll_to_bottom(cx);
                    cx.notify();
                }
                ChatEvent::Error { message } => {
                    log::error!("Chat error: {}", message);
                    panel.loading_state = LoadingState::Error(message.clone());
                    cx.notify();
                }
                ChatEvent::SessionStarted { provider, model } => {
                    log::info!("Chat session started: {} ({})", provider, model);
                    cx.notify();
                }
                ChatEvent::SessionCleared => {
                    panel.messages.clear();
                    panel.loading_state = LoadingState::Idle;
                    cx.notify();
                }
                ChatEvent::LoadingStateChanged { new_state, .. } => {
                    panel.loading_state = new_state.clone();
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
            loading_state: LoadingState::Idle,
            tab_id,
            send_message_keystroke: KeybindingKeystroke::from_keystroke(
                Keystroke::parse("shift-enter").unwrap(),
            ),
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

        // Set loading state to connecting when sending message
        self.loading_state = LoadingState::Connecting;
        cx.notify();

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
                        .children(self.messages.iter().cloned())
                        // Loading indicator
                        .when(self.loading_state.is_loading(), |this| {
                            this.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.loading_state.message())
                                    .when(self.loading_state.show_spinner(), |this| {
                                        this.child(
                                            Spinner::new()
                                                .icon(IconName::LoaderCircle)
                                                .small()
                                                .color(cx.theme().muted_foreground),
                                        )
                                    })
                                    .when(
                                        matches!(self.loading_state, LoadingState::Error(_)),
                                        |this| {
                                            this.child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(
                                                        Icon::new(IconName::TriangleAlert)
                                                            .size(px(14.))
                                                            .text_color(gpui::red()),
                                                    )
                                                    .child("Error"),
                                            )
                                        },
                                    ),
                            )
                        })
                        .scrollable(Axis::Vertical),
                ),
            )
            // Input area
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
                    .on_key_down(cx.listener(|this, evt: &gpui::KeyDownEvent, window, cx| {
                        if evt.keystroke.should_match(&this.send_message_keystroke) {
                            this.send_message(window, cx);
                        }
                    }))
                    .child(
                        Input::new(&self.input_state)
                            .disabled(self.loading_state.is_loading())
                            .bordered(false)
                            .p_3()
                            .bg(cx
                                .theme()
                                .highlight_theme
                                .style
                                .editor_background
                                .unwrap_or(cx.theme().background)),
                    )
                    // Send button positioned further to bottom right corner
                    .child(
                        div().absolute().bottom_3().right_3().child(
                            Button::new("send-message")
                                .icon(IconName::ArrowUp)
                                .primary()
                                .xsmall()
                                .disabled(self.loading_state.is_loading())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.send_message(window, cx);
                                })),
                        ),
                    ),
            )
    }
}
