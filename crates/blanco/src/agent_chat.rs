use gpui::{
    div, prelude::FluentBuilder, px, App, AppContext, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, Styled, WeakEntity,
    Window,
};
use gpui_component::{button::Button, input::TextInput, text::Text, ActiveTheme};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

// Types for agent system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: std::time::SystemTime,
}

#[derive(Debug)]
pub struct MockAgent {
    pub responses: Vec<String>,
    pub current_response: usize,
}

impl MockAgent {
    pub fn new() -> Self {
        let responses = vec![
            "Hello! I'm your database assistant. How can I help you today?".to_string(),
            "I can help you write SQL queries, optimize database performance, and explain database concepts. What would you like to know?".to_string(),
            "Feel free to ask me anything about SQL databases, and I'll provide detailed assistance.".to_string(),
        ];
        Self {
            responses,
            current_response: 0,
        }
    }

    pub fn get_next_response(&mut self) -> String {
        let response = self.responses[self.current_response].clone();
        self.current_response = (self.current_response + 1) % self.responses.len();
        response
    }
}

#[derive(Clone)]
pub struct AgentChat {
    pub focus_handle: FocusHandle,
    pub input_state: gpui_component::input::InputState,
    pub messages: Vec<ChatMessage>,
    pub mock_agent: MockAgent,
    pub pending_message: Option<String>,
    pub is_typing: bool,
    pub typing_start_time: Option<Instant>,
}

impl AgentChat {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let mock_agent = MockAgent::new();
        cx.new_entity(|_| Self {
            focus_handle: cx.focus_handle(),
            input_state: gpui_component::input::InputState::new(window, cx)
                .multi_line()
                .rows(3)
                .auto_grow(2, 6),
            messages: vec![ChatMessage {
                id: "0".to_string(),
                role: "agent".to_string(),
                content: r#"Welcome to the Database Assistant! I can help you with:

* Writing SQL queries
* Optimizing database performance
* Explaining database concepts
* Debugging query issues

**Example SQL Query:**
```sql
SELECT
    u.name,
    COUNT(o.id) as order_count,
    SUM(o.total) as total_spent
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
WHERE u.created_at >= '2024-01-01'
GROUP BY u.id, u.name
HAVING COUNT(o.id) > 5
ORDER BY total_spent DESC;
```

Feel free to ask me anything about your database! I'll provide detailed explanations and help you write better queries."#.to_string(),
                timestamp: std::time::SystemTime::now(),
            }],
            mock_agent,
            pending_message: None,
            is_typing: false,
            typing_start_time: None,
        })
    }

    pub fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input_state.text().is_empty() {
            return;
        }

        let user_message = self.input_state.text().to_string();
        let message_id = (self.messages.len() + 1).to_string();

        let chat_message = ChatMessage {
            id: message_id.clone(),
            role: "user".to_string(),
            content: user_message,
            timestamp: std::time::SystemTime::now(),
        };

        self.messages.push(chat_message);
        self.input_state.replace("", window, cx);

        // Start agent typing animation
        self.is_typing = true;
        self.typing_start_time = Some(Instant::now());
        cx.notify();

        // Simulate agent response with typing delay
        let agent_response = self.mock_agent.get_next_response();
        let typing_duration = Duration::from_millis(1500);

        let task = cx.spawn({
            let message_id = message_id.clone();
            async move |handle: WeakEntity<Self>, mut cx| {
                cx.background_executor().timer(typing_duration).await;

                if let Some(chat) = handle.upgrade() {
                    chat.update(&mut cx, |chat, cx| {
                        chat.is_typing = false;
                        chat.typing_start_time = None;

                        let agent_message = ChatMessage {
                            id: format!("{}_agent", message_id),
                            role: "agent".to_string(),
                            content: agent_response,
                            timestamp: std::time::SystemTime::now(),
                        };

                        chat.messages.push(agent_message);
                        cx.notify();
                    })
                    .ok();
                }
            }
        });

        task.detach();
    }

    fn parse_markdown_content(
        &self,
        content: &str,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement> {
        let mut elements = Vec::new();
        let mut current_text = String::new();
        let parser = Parser::new(content);

        for event in parser {
            match event {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language))) => {
                    // Flush any accumulated text
                    if !current_text.is_empty() {
                        elements.push(
                            div()
                                .child(
                                    Text::new(current_text.clone())
                                        .text_color(cx.theme().foreground)
                                        .text_left(),
                                )
                                .into_any_element(),
                        );
                        current_text.clear();
                    }
                }
                Event::End(Tag::CodeBlock(CodeBlockKind::Fenced(language))) => {
                    // Add code block with syntax highlighting
                    let lang_str = language.to_string();
                    let language = if lang_str.is_empty() {
                        "text"
                    } else {
                        &lang_str
                    };

                    // Use SQL syntax highlighter for SQL or unknown blocks, otherwise use the detected language
                    let syntax_language = if language == "sql" || language == "text" {
                        "sql"
                    } else {
                        language
                    };

                    let highlighter = SyntaxHighlighter::new(syntax_language);
                    let theme = HighlightTheme::default_dark();

                    elements.push(
                        div()
                            .bg(cx.theme().editor.background)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius)
                            .p_3()
                            .overflow_x_scroll()
                            .child(
                                div().mb_2().child(
                                    Text::new(language.to_uppercase())
                                        .text_color(cx.theme().muted_foreground)
                                        .text_left()
                                        .text_sm(),
                                ),
                            )
                            .child(
                                highlighter
                                    .theme(theme.into())
                                    .content(current_text.clone())
                                    .into_element(),
                            )
                            .into_any_element(),
                    );
                    current_text.clear();
                }
                Event::Text(text) => {
                    current_text.push_str(&text);
                }
                Event::SoftBreak | Event::HardBreak => {
                    current_text.push('\n');
                }
                Event::End(tag) => {
                    // Handle paragraph breaks
                    if matches!(tag, pulldown_cmark::Tag::Paragraph) {
                        if !current_text.is_empty() {
                            elements.push(
                                div()
                                    .child(
                                        Text::new(current_text.clone())
                                            .text_color(cx.theme().foreground)
                                            .text_left(),
                                    )
                                    .into_any_element(),
                            );
                            current_text.clear();
                        }
                    }
                }
                _ => {}
            }
        }

        // Flush any remaining text
        if !current_text.is_empty() {
            elements.push(
                div()
                    .child(
                        Text::new(current_text.clone())
                            .text_color(cx.theme().foreground)
                            .text_left(),
                    )
                    .into_any_element(),
            );
        }

        elements
    }
}

impl Render for AgentChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("agent-chat")
            .bg(cx.theme().background)
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().size_4().rounded_full().bg(gpui::green()))
                                    .child(
                                        Text::new("Database Assistant")
                                            .text_color(cx.theme().foreground)
                                            .font_semibold(),
                                    ),
                            )
                            .child(Text::new("Online").text_color(gpui::green()).text_sm()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_y_scroll()
                    .p_4()
                    .children(self.messages.iter().map(|message| {
                        let is_agent = message.role == "agent";
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .mb_4()
                            .when(is_agent, |div| {
                                div.flex()
                                    .items_start()
                                    .gap_2()
                                    .child(
                                        div()
                                            .w_6()
                                            .h_6()
                                            .rounded_full()
                                            .bg(gpui::green())
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .mt_1()
                                            .child(
                                                Text::new("AI")
                                                    .text_color(gpui::white())
                                                    .text_xs()
                                                    .font_semibold(),
                                            ),
                                    )
                                    .child(div().flex_1().max_w_96().children(
                                        self.parse_markdown_content(&message.content, cx),
                                    ))
                            })
                            .when(!is_agent, |div| {
                                div.flex()
                                    .items_end()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        div().flex_1().max_w_96().child(
                                            div()
                                                .bg(cx.theme().accent)
                                                .text_color(cx.theme().accent_foreground)
                                                .p_3()
                                                .rounded_lg()
                                                .child(Text::new(&message.content).text_left()),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .w_6()
                                            .h_6()
                                            .rounded_full()
                                            .bg(cx.theme().muted_foreground)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Text::new("U")
                                                    .text_color(cx.theme().background)
                                                    .text_xs()
                                                    .font_semibold(),
                                            ),
                                    )
                            })
                    }))
                    .when(self.is_typing, |div| {
                        div.child(
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .child(
                                    div()
                                        .w_6()
                                        .h_6()
                                        .rounded_full()
                                        .bg(gpui::green())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            Text::new("AI")
                                                .text_color(gpui::white())
                                                .text_xs()
                                                .font_semibold(),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .p_2()
                                        .child(
                                            div()
                                                .w_2()
                                                .h_2()
                                                .rounded_full()
                                                .bg(cx.theme().muted_foreground),
                                        )
                                        .child(
                                            div()
                                                .w_2()
                                                .h_2()
                                                .rounded_full()
                                                .bg(cx.theme().muted_foreground),
                                        )
                                        .child(
                                            div()
                                                .w_2()
                                                .h_2()
                                                .rounded_full()
                                                .bg(cx.theme().muted_foreground),
                                        ),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .p_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .relative()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div().flex_1().child(
                                    TextInput::new("chat-input")
                                        .id("chat-input")
                                        .placeholder("Ask me about your database...")
                                        .size(px(400.))
                                        .flex_1()
                                        .state(&self.input_state)
                                        .on_key_down(
                                            gpui::Key::Enter,
                                            cx.listener(
                                                |this, event: &KeyDownEvent, window, cx| {
                                                    if event.modifiers.is_empty() {
                                                        this.send_message(window, cx);
                                                    }
                                                },
                                            ),
                                        ),
                                ),
                            )
                            .child(
                                Button::new("send-button")
                                    .label("Send")
                                    .compact()
                                    .variant(gpui_component::button::ButtonVariant::Primary)
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        this.send_message(window, cx);
                                    })),
                            ),
                    ),
            )
    }
}

// ChatPanel wrapper that's used by the editor panel
pub struct ChatPanel {
    agent_chat: Entity<AgentChat>,
    tab_id: Option<String>,
}

impl ChatPanel {
    pub fn new(
        tab_id: usize,
        _session_id: Option<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let agent_chat = AgentChat::new(window, cx);
        Self {
            agent_chat,
            tab_id: Some(tab_id.to_string()),
        }
    }

    pub fn update_sql_context(&mut self, _sql_context: String, _cx: &mut Context<Self>) {
        // TODO: Implement SQL context updates for the chat panel
    }
}

impl Render for ChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.agent_chat.clone())
    }
}
