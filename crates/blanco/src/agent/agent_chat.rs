use gpui::{
    actions, div, prelude::FluentBuilder, px, App, AppContext, Context, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    Subscription, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputState, TextInput},
    v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt,
};
use ropey::Rope;
use std::sync::Arc;
use std::time::Duration;

use super::chat_session::ChatSession;
use super::chat_types::{ChatEvent, ChatMessage, MessageMetadata, MessageRole, SqlContext};

actions!(agent_chat, [SendMessage, ClearChat, ExportChat]);

pub struct ChatPanel {
    pub focus_handle: FocusHandle,
    pub session: Entity<ChatSession>,
    pub input_state: Entity<InputState>,
    pub messages: Vec<ChatMessage>,
    pub _subscriptions: Vec<Subscription>,
    pub is_loading: bool,
    pub tab_id: usize,
}

#[allow(dead_code)]
impl ChatPanel {
    pub fn new(
        tab_id: usize,
        _http_client: Option<Arc<dyn http_client::HttpClient>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create chat session with mock provider for now
        let session = cx.new(|_cx| ChatSession::with_mock());

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
                    panel.messages.push(message.clone());
                    panel.scroll_to_bottom(cx);

                    // Emit app event for message received
                    // let role = match message.role {
                    //     MessageRole::User => "user".to_string(),
                    //     MessageRole::Assistant => "assistant".to_string(),
                    //     MessageRole::System => "system".to_string(),
                    // };

                    // cx.emit(crate::app_events::AppEvent::ChatMessageReceived {
                    //     tab_id: panel.tab_id,
                    //     message_content: message.content.clone(),
                    //     role,
                    // });

                    cx.notify();
                }
                ChatEvent::MessageUpdated {
                    message_id: _,
                    content: _,
                } => {
                    // Handle message updates for streaming
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

                    // Emit chat error event
                    // cx.emit(crate::app_events::AppEvent::ChatError {
                    //     tab_id: panel.tab_id,
                    //     error_message: message.clone(),
                    // });

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

    pub fn new_with_openai(
        tab_id: usize,
        api_key: String,
        http_client: Arc<dyn http_client::HttpClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = cx.new(|_cx| ChatSession::with_openai(api_key, http_client));

        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line()
                .rows(3)
                .auto_grow(2, 6) // Auto-grow between 2 and 6 rows
                .placeholder("Ask me anything about your SQL query...")
        });

        let mut subscriptions = Vec::new();

        // Subscribe to session events
        let subscription = cx.subscribe(&session, |panel, _session, event, cx| match event {
            ChatEvent::MessageAdded { message } => {
                panel.messages.push(message.clone());
                panel.scroll_to_bottom(cx);
                cx.notify();
            }
            ChatEvent::StreamCompleted { .. } => {
                panel.is_loading = false;
                panel.scroll_to_bottom(cx);
                cx.notify();
            }
            ChatEvent::Error { message } => {
                log::error!("Chat error: {}", message);
                panel.is_loading = false;
                cx.notify();
            }
            ChatEvent::SessionCleared => {
                panel.messages.clear();
                cx.notify();
            }
            _ => {}
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

    pub fn send_message(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_loading {
            return;
        }

        let input_text = self.input_state.read(cx).text().to_string();
        if input_text.trim().is_empty() {
            return;
        }

        // Add user message to chat
        let user_message = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: input_text.clone(),
            timestamp: chrono::Utc::now(),
            metadata: MessageMetadata::default(),
        };
        self.messages.push(user_message);

        // Emit message sent event
        cx.emit(crate::app_events::AppEvent::ChatMessageSent {
            tab_id: self.tab_id,
            message_content: input_text.clone(),
        });

        // Clear input - TODO: implement proper text clearing for InputState
        // For now, we'll leave the input as is and clear it after getting response

        // Set loading state
        self.is_loading = true;
        cx.notify();

        // Send message to session
        let session = self.session.clone();
        let message = input_text.clone();
        let tab_id = self.tab_id;

        cx.spawn(async move |handle, cx| {
            match session.update(cx, |chat_session, cx| {
                chat_session.send_message(message, cx)
            }) {
                Ok(task) => {
                    match task.await {
                        Ok(response) => {
                            handle
                                .update(cx, |chat_panel, cx| {
                                    chat_panel.is_loading = false;

                                    // Add assistant response to chat
                                    let assistant_message = ChatMessage {
                                        id: uuid::Uuid::new_v4().to_string(),
                                        role: MessageRole::Assistant,
                                        content: response.clone(),
                                        timestamp: chrono::Utc::now(),
                                        metadata: MessageMetadata::default(),
                                    };
                                    chat_panel.messages.push(assistant_message);

                                    // Emit message received event
                                    cx.emit(crate::app_events::AppEvent::ChatMessageReceived {
                                        tab_id,
                                        message_content: response,
                                        role: "assistant".to_string(),
                                    });

                                    cx.notify();
                                })
                                .ok();
                        }
                        Err(e) => {
                            handle
                                .update(cx, |chat_panel, cx| {
                                    chat_panel.is_loading = false;

                                    // Add error message
                                    let error_message = ChatMessage {
                                        id: uuid::Uuid::new_v4().to_string(),
                                        role: MessageRole::System,
                                        content: format!("Error: {}", e),
                                        timestamp: chrono::Utc::now(),
                                        metadata: MessageMetadata::default(),
                                    };
                                    chat_panel.messages.push(error_message);

                                    cx.notify();
                                })
                                .ok();
                        }
                    }
                }
                Err(_) => {
                    // Session no longer exists
                    drop(handle);
                }
            }
        })
        .detach();

        cx.notify();
    }

    pub fn clear_chat(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.clear_messages();
            cx.emit(ChatEvent::SessionCleared);
        });
    }

    pub fn export_chat(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Trigger export command
        self.session.update(cx, |session, cx| {
            let export_command = "/export".to_string();
            std::mem::drop(session.send_message(export_command, cx));
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

    fn on_export_chat(&mut self, _: &ExportChat, window: &mut Window, cx: &mut Context<Self>) {
        self.export_chat(window, cx);
    }

    /// Check if content contains SQL code
    fn contains_sql_code(&self, content: &str) -> bool {
        let sql_keywords = [
            "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "FROM", "WHERE",
            "JOIN",
        ];
        let content_upper = content.to_uppercase();

        // Check if content contains SQL keywords and is reasonably long
        sql_keywords
            .iter()
            .any(|keyword| content_upper.contains(keyword))
            && content.trim().len() > 10
    }

    /// Parse markdown content and extract SQL code blocks
    fn parse_markdown_content(
        &self,
        content: &str,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement> {
        use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

        let parser = Parser::new(content);
        let mut elements = Vec::new();
        let mut in_code_block = false;
        let mut code_content = Vec::new();
        let mut code_language = String::new();
        let mut current_text = String::new();

        for event in parser {
            match event {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                    in_code_block = true;
                    code_language = lang.to_lowercase();
                    code_content.clear();
                }
                Event::Start(Tag::CodeBlock(_)) => {
                    in_code_block = true;
                    code_language.clear();
                    code_content.clear();
                }
                Event::End(Tag::CodeBlock(_)) => {
                    if in_code_block {
                        let code = code_content.join("\n");

                        // Render as SQL code block with syntax highlighting
                        let shared_text = gpui::SharedString::from(code.clone());
                        let text_len = shared_text.len();
                        let range = std::ops::Range::<usize> {
                            start: 0,
                            end: text_len,
                        };

                        // Create syntax highlighter for SQL
                        let mut highlighter =
                            gpui_component::highlighter::SyntaxHighlighter::new(&code_language);
                        highlighter.update(None, &Rope::from(code));
                        let theme = cx.theme().highlight_theme.clone();
                        let highlights = highlighter.styles(&range, &theme);

                        elements.push(
                            div()
                                .w_full()
                                .bg(cx
                                    .theme()
                                    .highlight_theme
                                    .style
                                    .editor_background
                                    .unwrap_or(cx.theme().background))
                                .border_1()
                                .border_color(cx.theme().border)
                                .rounded_lg()
                                .p_3()
                                .font_family("Fira Code")
                                .text_size(px(13.))
                                .text_color(cx.theme().foreground)
                                .text_left()
                                .child(
                                    gpui::StyledText::new(shared_text).with_highlights(highlights),
                                ),
                        );

                        in_code_block = false;
                        code_content.clear();
                        code_language.clear();
                    }
                }
                Event::Text(text) => {
                    if in_code_block {
                        code_content.push(text.to_string());
                    } else {
                        current_text.push_str(&text);
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if in_code_block {
                        code_content.push("\n".to_string());
                    } else {
                        current_text.push(' ');
                    }
                }
                Event::End(_) => {
                    // Don't add anything yet - wait for more text or a code block
                }
                _ => {
                    // Handle other events as needed
                }
            }
        }

        // Add any remaining text (left-aligned)
        if !current_text.trim().is_empty() {
            elements.push(div().text_sm().text_left().child(current_text));
        }

        elements
    }
}

impl Focusable for ChatPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<crate::app_events::AppEvent> for ChatPanel {}

impl Render for ChatPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .border_t_1()
            .border_color(cx.theme().border)
            // Header
            .child(
                h_flex()
                    .px_4()
                    .py_2()
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
                                    .size_4()
                                    .text_color(cx.theme().primary),
                            )
                            .child(div().text_sm().font_medium().child("SQL Assistant")),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("clear-chat")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::CircleX)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_chat(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("export-chat")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::File)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.export_chat(window, cx);
                                    })),
                            ),
                    ),
            )
            // Messages area
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .scrollable(gpui::Axis::Vertical)
                    .gap_3()
                    .child(
                        // Welcome message if empty
                        div().when(self.messages.is_empty(), |this| {
                            this.child(
                                v_flex().items_center().justify_center().h_full().child(
                                    div()
                                        .text_center()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    Icon::new(IconName::Bot)
                                                        .size_8()
                                                        .text_color(cx.theme().muted_foreground),
                                                )
                                                .child(div().text_sm().child(
                                                    "Ask me anything about your SQL queries!",
                                                ))
                                                .child(div().text_xs().child(
                                                    "Try: /explain, /optimize, /fix, /schema",
                                                ))
                                                // Example message to demonstrate markdown parsing
                                                .child(
                                                    div()
                                                        .mt_4()
                                                        .p_3()
                                                        .bg(cx.theme().muted.opacity(0.3))
                                                        .rounded_lg()
                                                        .border_1()
                                                        .border_color(cx.theme().border)
                                                        .child(
                                                            v_flex()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .font_medium()
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .child("Example Query:")
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(cx.theme().foreground)
                                                                        .child("Can you help me optimize this query?")
                                                                )
                                                                // Show example markdown as it would be rendered
                                                                .children(
                                                                    self.parse_markdown_content(
                                                                        "I notice your query is doing a full table scan. Here's a more efficient version:\n\n```sql\nSELECT u.id, u.name, u.email, COUNT(o.id) as order_count\nFROM users u\nLEFT JOIN orders o ON u.id = o.user_id\nWHERE u.created_at >= '2024-01-01'\nGROUP BY u.id, u.name, u.email\nHAVING COUNT(o.id) > 5\nORDER BY order_count DESC\nLIMIT 10;\n```\n\nThis adds an index on `created_at` and uses proper JOIN syntax for better performance.",
                                                                        cx
                                                                    )
                                                                )
                                                        )
                                                ),
                                        ),
                                ),
                            )
                        }),
                    )
                    // Messages
                    .children(self.messages.iter().enumerate().map(|(ix, message)| {
                        let is_user = message.role == MessageRole::User;

                        div().id(("chat-message", ix)).w_full().child(
                            h_flex()
                                .gap_3()
                                .when(is_user, |h_flex| {
                                    h_flex.flex_row_reverse() // User messages on the right
                                })
                                .child(
                                    // Avatar
                                    div()
                                        .w(px(32.))
                                        .h(px(32.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(if is_user {
                                            cx.theme().primary
                                        } else {
                                            cx.theme().secondary
                                        })
                                        .child(
                                            Icon::new(if is_user {
                                                IconName::User
                                            } else {
                                                IconName::Bot
                                            })
                                            .size_4()
                                            .text_color(cx.theme().background),
                                        ),
                                )
                                .child(
                                    // Message content
                                    v_flex()
                                        .flex_1()
                                        .max_w(px(600.))
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(if is_user { "You" } else { "Assistant" }),
                                        )
                                        .child(
                                            div()
                                                .px_3()
                                                .py_2()
                                                .rounded_lg()
                                                .bg(if is_user {
                                                    cx.theme().primary
                                                } else {
                                                    cx.theme().muted
                                                })
                                                .text_color(if is_user {
                                                    cx.theme().primary_foreground
                                                } else {
                                                    cx.theme().foreground
                                                })
                                                .when(!is_user, |div| {
                                                    div.border_1().border_color(cx.theme().border)
                                                })
                                                // Parse and render markdown content
                                                .children(
                                                    self.parse_markdown_content(&message.content, cx)
                                                ),
                                        ),
                                ),
                        )
                    }))
                    // Loading indicator
                    .when(self.is_loading, |this| {
                        this.child(
                            h_flex()
                                .gap_3()
                                .child(
                                    div()
                                        .w(px(32.))
                                        .h(px(32.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(cx.theme().secondary)
                                        .child(
                                            Icon::new(IconName::Bot)
                                                .size_4()
                                                .text_color(cx.theme().background),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_3()
                                        .py_2()
                                        .rounded_lg()
                                        .bg(cx.theme().muted)
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Thinking..."),
                                        ),
                                ),
                        )
                    }),
            )
            // Input area
            .child(
                v_flex()
                    .p_4()
                    .bg(cx.theme().highlight_theme.style.editor_background.unwrap_or(cx.theme().background))
                    .gap_3()
                    .child(
                        // Text input container with no borders
                        div()
                            .relative()
                            .bg(cx.theme().highlight_theme.style.editor_background.unwrap_or(cx.theme().background))
                            .rounded_lg()
                            .border_0()
                            .p_3()
                            .text_size(px(13.0)) // Smaller font size for the input text
                            .child(
                                TextInput::new(&self.input_state)
                                    .disabled(self.is_loading)
                                    .bordered(false)
                                    .bg(cx.theme().highlight_theme.style.editor_background.unwrap_or(cx.theme().background))
                            )
                            // Send button positioned further to bottom right corner
                            .child(
                                div()
                                    .absolute()
                                    .bottom_1()
                                    .right_1()
                                    .child(
                                        Button::new("send-message")
                                            .icon(IconName::ArrowUp)
                                            .primary()
                                            .xsmall()
                                            .disabled(self.is_loading)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.send_message(window, cx);
                                            }))
                                    )
                            )
                    )
            )
    }
}

// Message component for future use
#[allow(dead_code)]
pub struct ChatMessageComponent {
    pub message: ChatMessage,
}

#[allow(dead_code)]
impl ChatMessageComponent {
    pub fn new(message: ChatMessage) -> Self {
        Self { message }
    }
}

impl Render for ChatMessageComponent {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_user = self.message.role == MessageRole::User;

        h_flex()
            .gap_3()
            .when(is_user, |h_flex| h_flex.flex_row_reverse())
            .child(
                // Avatar
                div()
                    .w(px(32.))
                    .h(px(32.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if is_user {
                        cx.theme().primary
                    } else {
                        cx.theme().secondary
                    })
                    .child(
                        Icon::new(if is_user {
                            IconName::User
                        } else {
                            IconName::Bot
                        })
                        .size_4()
                        .text_color(cx.theme().background),
                    ),
            )
            .child(
                // Message content
                v_flex()
                    .flex_1()
                    .max_w(px(600.))
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().muted_foreground)
                            .child(if is_user { "You" } else { "Assistant" }),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .bg(if is_user {
                                cx.theme().primary
                            } else {
                                cx.theme().muted
                            })
                            .text_color(if is_user {
                                cx.theme().primary_foreground
                            } else {
                                cx.theme().foreground
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .whitespace_nowrap()
                                    .child(self.message.content.clone()),
                            ),
                    ),
            )
    }
}
