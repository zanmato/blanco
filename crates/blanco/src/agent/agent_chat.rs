use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Window, actions, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::Scrollbar,
    select::{Select, SelectDelegate, SelectEvent, SelectItem, SelectState},
    spinner::Spinner,
    v_flex,
};
use std::sync::Arc;
use std::time::Duration;

use super::chat_message_view::{ApprovalEvent, ChatMessageState};
use super::chat_session::{ChatSession, ChatSessionContext};
use super::chat_types::{ApprovalState, ChatEvent, LoadingState, MessageRole};
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
    pub tool_mode_select: Entity<SelectState<ToolModeSelectDelegate>>,
    pub current_tool_mode: ToolMode,
}

impl ChatPanel {
    pub fn new(
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
                        message.content.to_string(),
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

                panel.messages.push(message_state);
                panel.scroll_to_bottom(cx);

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

        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
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
        let scroll_handle = self.scroll_handle.clone();

        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            scroll_handle.scroll_to_bottom();
        })
        .detach();
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
            .bg(cx.theme().sidebar_primary_foreground)
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
                        v_flex()
                            .id("agent-messages")
                            .p_3()
                            .gap_4()
                            .size_full()
                            .track_scroll(&self.scroll_handle)
                            .overflow_scroll()
                            .children(self.messages.iter().cloned())
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
            .child(
                div()
                    .relative()
                    .bg(self.editor_background_color(cx))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_size(px(13.0))
                    .child(
                        Input::new(&self.input_state)
                            .disabled(self.loading_state.is_loading())
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
