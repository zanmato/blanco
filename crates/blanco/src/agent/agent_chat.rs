use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeybindingKeystroke, Keystroke, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window, actions, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    scroll::Scrollbar,
    select::{Select, SelectDelegate, SelectEvent, SelectItem, SelectState},
    spinner::Spinner,
    v_flex,
};
use std::sync::Arc;
use std::time::Duration;

use super::chat_message_view::ChatMessageState;
use super::chat_session::{ChatSession, ChatSessionContext};
use super::chat_types::{ChatEvent, LoadingState};
use super::tool_handlers::ToolMode;
use blanco_ui::IconName;
use gpui::ScrollHandle;
use llm::LLMProvider;

actions!(agent_chat, [SendMessage, ClearChat]);

pub struct ChatPanel {
    pub focus_handle: FocusHandle,
    pub scroll_handle: ScrollHandle,
    pub session: Entity<ChatSession>,
    pub input_state: Entity<InputState>,
    pub messages: Vec<Entity<ChatMessageState>>,
    pub _subscriptions: Vec<Subscription>,
    pub loading_state: LoadingState,
    #[allow(dead_code)]
    pub tab_id: usize,
    pub send_message_keystroke: KeybindingKeystroke,
    pub tool_mode_select: Entity<SelectState<ToolModeSelectDelegate>>,
}

impl ChatPanel {
    pub fn new(
        tab_id: usize,
        llm: Arc<Box<dyn LLMProvider>>,
        provider_name: String,
        model_name: String,
        session_context: ChatSessionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session =
            cx.new(|_cx| ChatSession::new(llm, provider_name, model_name, session_context));

        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(3)
                .auto_grow(2, 6) // Auto-grow between 2 and 6 rows
                .placeholder("Ask me anything about your SQL query...")
        });

        let mut subscriptions = Vec::new();

        // Subscribe to session events
        let subscription = cx.subscribe(&session, |panel, _session, event, cx| {
            match event {
                ChatEvent::MessageAdded { message } => {
                    tracing::info!("Message added! {:?}", message);
                    let message = message.clone();
                    let message_state = cx.new(|cx| {
                        ChatMessageState::new(
                            panel.messages.len(),
                            message.content.into(),
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
                    tracing::debug!("Chat stream started: {}", message_id);
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
                    tracing::error!("Chat error: {}", message);
                    panel.loading_state = LoadingState::Error(message.clone());
                    cx.notify();
                }
                ChatEvent::SessionStarted { provider, model } => {
                    tracing::info!("Chat session started: {} ({})", provider, model);
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

        // Create tool mode select (Read/Write)
        let tool_mode_select = cx.new(|cx| {
            SelectState::new(
                ToolModeSelectDelegate::new(),
                Some(gpui_component::IndexPath::default().row(0)), // Default to "Read"
                window,
                cx,
            )
        });

        // Subscribe to tool mode select changes
        let session_clone = session.clone();
        subscriptions.push(
            cx.subscribe(&tool_mode_select, move |_panel, _select, event, cx| {
                let SelectEvent::Confirm(selected) = event;
                let mode = if selected.as_ref().map(|s| s.as_str()) == Some("Write") {
                    ToolMode::Write
                } else {
                    ToolMode::Read
                };

                session_clone.update(cx, |session, cx| {
                    session.set_tool_mode(mode, cx);
                });
            }),
        );

        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            session,
            input_state,
            messages: Vec::new(),
            _subscriptions: subscriptions,
            loading_state: LoadingState::Idle,
            tab_id,
            send_message_keystroke: KeybindingKeystroke::from_keystroke(
                Keystroke::parse("shift-enter").unwrap(),
            ),
            tool_mode_select,
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
            session.send_message(input_text.clone(), window, cx);
        });
    }

    pub fn abort_message(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.abort(cx);
        });
        self.loading_state = LoadingState::Idle;
        cx.notify();
    }

    pub fn clear_chat(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.clear_messages();
            cx.emit(ChatEvent::SessionCleared);
        });
    }

    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        // Clone the scroll handle before moving into the async closure
        let scroll_handle = self.scroll_handle.clone();

        // Schedule scroll to bottom after render
        cx.spawn(async move |_, _cx| {
            // Small delay to ensure content is rendered
            gpui::Timer::after(Duration::from_millis(50)).await;
            scroll_handle.scroll_to_bottom();
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
            .bg(cx.theme().sidebar_primary_foreground)
            .text_color(cx.theme().foreground)
            // Header
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
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
                                .tooltip("Clear Chat")
                                .icon(IconName::ClearChat)
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.on_clear_chat(&ClearChat, window, cx)
                                })),
                        ),
                    ),
            )
            // Messages area
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("agent-messages")
                            .p_3()
                            .gap_4()
                            .size_full()
                            .track_scroll(&self.scroll_handle)
                            .overflow_scroll()
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
                                                    Icon::new(IconName::TriangleAlert)
                                                        .size(px(14.))
                                                        .text_color(gpui::red()),
                                                )
                                            },
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(&self.scroll_handle)),
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
                    .border_t_1()
                    .border_color(cx.theme().border)
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
                    // Tool mode select and send button in a row below the input
                    .child(
                        div()
                            .w_full()
                            .p_3()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                div().w(px(80.)).child(
                                    Select::new(&self.tool_mode_select)
                                        .xsmall()
                                        .appearance(false)
                                        .menu_width(px(80.)),
                                ),
                            )
                            .when(self.session.read(cx).is_generating(), |this| {
                                this.child(
                                    Button::new("stop-generation")
                                        .icon(IconName::SquareStop)
                                        .danger()
                                        .xsmall()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.abort_message(window, cx);
                                        })),
                                )
                            })
                            .when(!self.session.read(cx).is_generating(), |this| {
                                this.child(
                                    Button::new("send-message")
                                        .icon(IconName::ArrowUp)
                                        .primary()
                                        .xsmall()
                                        .disabled(self.loading_state.is_loading())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.send_message(window, cx);
                                        })),
                                )
                            }),
                    ),
            )
    }
}

/// Select delegate for tool mode (Read/Write)
#[derive(Clone)]
pub struct ToolModeSelectDelegate {
    modes: Vec<SharedString>,
}

impl ToolModeSelectDelegate {
    pub fn new() -> Self {
        Self {
            modes: vec!["Read".into(), "Write".into()],
        }
    }
}

impl Default for ToolModeSelectDelegate {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectDelegate for ToolModeSelectDelegate {
    type Item = SharedString;

    fn items_count(&self, _section: usize) -> usize {
        self.modes.len()
    }

    fn item(&self, ix: gpui_component::IndexPath) -> Option<&Self::Item> {
        self.modes.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<gpui_component::IndexPath>
    where
        Self::Item: gpui_component::select::SelectItem<Value = V>,
        V: PartialEq,
    {
        self.modes
            .iter()
            .position(|v| *v.value() == *value)
            .map(|ix| gpui_component::IndexPath::default().row(ix))
    }
}
