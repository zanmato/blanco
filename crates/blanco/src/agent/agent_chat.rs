use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, FollowMode, IntoElement,
    ListAlignment, ParentElement, Render, SharedString, Styled, Subscription, Window, actions, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Icon, Sizable, StyledExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    scroll::Scrollbar,
    select::{Select, SelectDelegate, SelectEvent, SelectItem, SelectState},
    spinner::Spinner,
    v_flex,
};
use std::sync::Arc;

use super::chat_message_view::{ApprovalEvent, ChatMessageState};
use super::chat_session::{ChatSession, ChatSessionContext};
use super::chat_types::{ApprovalState, ChatEvent, LoadingState, MessageRole};
use super::streaming::StreamingChatProvider;
use super::tool_handlers::ToolMode;
use blanco_ui::IconName;

actions!(agent_chat, [SendMessage, ClearChat]);

pub struct ChatPanel {
    pub focus_handle: FocusHandle,
    /// Virtualized message list: only visible items lay out, item heights are cached and
    /// re-measured when a message's content changes.
    pub list_state: gpui::ListState,
    pub session: Entity<ChatSession>,
    pub input_state: Entity<TextareaState>,
    pub messages: Vec<Entity<ChatMessageState>>,
    pub _subscriptions: Vec<Subscription>,
    pub loading_state: LoadingState,
    pub tool_mode_select: Entity<SelectState<ToolModeSelectDelegate>>,
    pub current_tool_mode: ToolMode,
}

impl ChatPanel {
    pub fn new(
        llm: Arc<dyn StreamingChatProvider>,
        provider_name: String,
        model_name: String,
        session_context: ChatSessionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session =
            cx.new(|_cx| ChatSession::new(llm, provider_name, model_name, session_context));

        let input_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .auto_grow(2, 6)
                .placeholder("Ask me anything about your query...")
        });

        let mut subscriptions = Vec::new();

        let subscription = cx.subscribe(&session, |panel, _session, event, cx| match event {
            ChatEvent::MessageAdded { message } => {
                let message = message.clone();
                let is_tool_request = message.role == MessageRole::ToolRequest;
                let message_state = cx.new(|cx| {
                    ChatMessageState::new(
                        panel.messages.len(),
                        message.id.clone(),
                        message.content.to_string(),
                        message.reasoning.as_ref().map(|r| r.to_string()),
                        message.role.clone(),
                        Some(message.metadata.clone()),
                        cx,
                    )
                });

                if message.role == MessageRole::Tool
                    && let Some(ref tool_call_id) = message.tool_call_id
                {
                    let already_resolved = panel.messages.iter().any(|entity| {
                        entity
                            .read(cx)
                            .metadata
                            .as_ref()
                            .and_then(|m| m.approval.as_ref())
                            .map(|a| {
                                a.tool_call_id == *tool_call_id
                                    && entity.read(cx).tool_result.is_some()
                            })
                            .unwrap_or(false)
                    });
                    if already_resolved {
                        message_state.update(cx, |state, cx| {
                            state.hidden = true;
                            cx.notify();
                        });
                    }
                }

                if is_tool_request {
                    let session_handle = panel.session.clone();
                    panel._subscriptions.push(cx.subscribe(
                        &message_state,
                        move |_panel, _state, event, cx| {
                            let ApprovalEvent {
                                tool_call_id,
                                approved,
                            } = event;
                            session_handle.update(cx, |session, _cx| {
                                if *approved {
                                    session.approve_tool(tool_call_id);
                                } else {
                                    session.deny_tool(tool_call_id);
                                }
                            });
                        },
                    ));
                }

                // One observer per message keeps the list's cached height fresh for any
                // change (stream deltas, tool results, approval state, thinking toggle).
                panel._subscriptions.push(cx.observe(
                    &message_state,
                    |panel, message_entity, cx| {
                        if let Some(index) = panel
                            .messages
                            .iter()
                            .position(|entity| *entity == message_entity)
                        {
                            panel.list_state.remeasure_items(index..index + 1);
                        }
                        cx.notify();
                    },
                ));

                let index = panel.messages.len();
                panel.messages.push(message_state);
                panel.list_state.splice(index..index, 1);

                cx.notify();
            }
            ChatEvent::SessionCleared => {
                panel.messages.clear();
                panel.list_state.reset(0);
                panel.loading_state = LoadingState::Idle;
                cx.notify();
            }
            ChatEvent::LoadingStateChanged { new_state, .. } => {
                panel.loading_state = new_state.clone();
                cx.notify();
            }
            ChatEvent::ToolResultReady {
                tool_call_id,
                result_summary,
            } => {
                let result_summary = result_summary.clone();
                let tool_call_id = tool_call_id.clone();
                for msg_entity in &panel.messages {
                    let is_match = msg_entity
                        .read(cx)
                        .metadata
                        .as_ref()
                        .and_then(|m| m.approval.as_ref())
                        .map(|a| a.tool_call_id == tool_call_id)
                        .unwrap_or(false);
                    if is_match {
                        msg_entity.update(cx, |state, cx| {
                            state.approval_state = Some(ApprovalState::Approved);
                            state.set_tool_result(result_summary.clone());
                            cx.notify();
                        });
                        break;
                    }
                }
                cx.notify();
            }
            ChatEvent::StreamDelta {
                message_id,
                content_delta,
                reasoning_delta,
            } => {
                if let Some(entity) = panel
                    .messages
                    .iter()
                    .find(|entity| entity.read(cx).message_id == *message_id)
                {
                    entity.update(cx, |state, cx| {
                        state.append_stream_delta(content_delta, reasoning_delta, cx);
                    });
                }
            }
            ChatEvent::StreamCompleted {
                message_id,
                prompt_tokens,
                completion_tokens,
            } => {
                if *prompt_tokens > 0 || *completion_tokens > 0 {
                    if let Some(entity) = panel
                        .messages
                        .iter()
                        .find(|entity| entity.read(cx).message_id == *message_id)
                    {
                        entity.update(cx, |state, cx| {
                            state.set_usage(*prompt_tokens, *completion_tokens);
                            cx.notify();
                        });
                    }
                }
            }
        });

        subscriptions.push(subscription);

        let tool_mode_select = cx.new(|cx| {
            SelectState::new(
                ToolModeSelectDelegate::new(),
                Some(gpui_component::IndexPath::default().row(0)),
                window,
                cx,
            )
        });

        let session_clone = session.clone();
        subscriptions.push(
            cx.subscribe(&tool_mode_select, move |panel, _select, event, cx| {
                let SelectEvent::Confirm(selected) = event;
                let mode = match selected.as_ref().map(|s| s.as_str()) {
                    Some("Allow all") => ToolMode::AllowAll,
                    _ => ToolMode::Ask,
                };

                panel.current_tool_mode = mode;

                session_clone.update(cx, |session, cx| {
                    session.set_tool_mode(mode, cx);
                });
            }),
        );

        let subscription = cx.subscribe_in(
            &input_state,
            window,
            |this, _input_state, event, window, cx| {
                if let InputEvent::PressEnter { secondary, .. } = event
                    && *secondary
                {
                    this.send_message(window, cx);
                }
            },
        );
        subscriptions.push(subscription);

        let list_state = gpui::ListState::new(0, ListAlignment::Top, px(1024.));
        list_state.set_follow_mode(FollowMode::Tail);

        Self {
            focus_handle: cx.focus_handle(),
            list_state,
            session,
            input_state,
            messages: Vec::new(),
            _subscriptions: subscriptions,
            loading_state: LoadingState::Idle,
            tool_mode_select,
            current_tool_mode: ToolMode::Ask,
        }
    }

    pub fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input_text = self.input_state.read(cx).text().to_string();

        if input_text.trim().is_empty() {
            return;
        }

        self.input_state.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });

        // A send while a turn is running queues the message for steering; the panel's
        // loading state already reflects that turn and must not be reset.
        if !self.session.read(cx).is_generating() {
            self.loading_state = LoadingState::Connecting;
        }
        // Sending snaps back to the newest messages and re-engages tail following.
        self.list_state.set_follow_mode(FollowMode::Tail);
        self.list_state.scroll_to_end();
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

    fn on_clear_chat(&mut self, _: &ClearChat, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_chat(window, cx);
    }

    fn editor_background_color(&self, cx: &App) -> gpui::Hsla {
        cx.theme()
            .highlight_theme
            .style
            .editor_background
            .unwrap_or(cx.theme().background)
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
            // No background: this panel reaches the editor card's bottom-right
            // corner, and a square fill would cover the border's arc.
            .text_color(cx.theme().foreground)
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
                            .child(div().text_sm().font_medium().child("Assistant")),
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
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        gpui::list(
                            self.list_state.clone(),
                            cx.processor(|this, index: usize, _window, _cx| {
                                match this.messages.get(index) {
                                    Some(message) => div()
                                        .px_3()
                                        .when(index == 0, |el| el.pt_3())
                                        .pb_4()
                                        .child(message.clone())
                                        .into_any_element(),
                                    None => div().into_any_element(),
                                }
                            }),
                        )
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(&self.list_state)),
                    ),
            )
            // The status strip lives outside the scroll area so the list's item count always
            // matches `messages.len()`.
            .when(
                self.loading_state.is_loading()
                    || matches!(self.loading_state, LoadingState::Error(_)),
                |this| {
                    this.child(
                        h_flex()
                            .px_3()
                            .py_1()
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
                },
            )
            .child(
                div()
                    .relative()
                    .bg(self.editor_background_color(cx))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_size(px(13.0))
                    .child(
                        Textarea::new(&self.input_state)
                            .bordered(false)
                            .p_3()
                            .bg(self.editor_background_color(cx)),
                    )
                    .child(
                        div()
                            .w_full()
                            .p_3()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .when(self.current_tool_mode == ToolMode::AllowAll, |el| {
                                el.child(
                                    div().px_3().py_1().child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::TriangleAlert)
                                                    .size(px(12.))
                                                    .text_color(cx.theme().danger),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().danger)
                                                    .child(
                                                        "The agent can execute SQL without asking",
                                                    ),
                                            ),
                                    ),
                                )
                            })
                            .child(
                                div().w(px(110.)).child(
                                    Select::new(&self.tool_mode_select)
                                        .xsmall()
                                        .appearance(false)
                                        .menu_width(px(110.)),
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
                            .child(
                                Button::new("send-message")
                                    .icon(IconName::ArrowUp)
                                    .primary()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_message(window, cx);
                                    })),
                            ),
                    ),
            )
    }
}

#[derive(Clone)]
pub struct ToolModeSelectDelegate {
    modes: Vec<SharedString>,
}

impl ToolModeSelectDelegate {
    pub fn new() -> Self {
        Self {
            modes: vec!["Ask".into(), "Allow all".into()],
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
