use gpui::{
    actions, div, prelude::FluentBuilder, px, App, AppContext, ClipboardItem, Context, Entity, EventEmitter,
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
use blanco_core::chat_provider::{ChatProvider, ProviderError};

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

    pub fn new_with_provider(
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

        // Note: Don't add user message here directly
        // It will be added through the session's MessageAdded event
        // This prevents duplication of user messages

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

    /// Parse markdown content preserving order using pulldown-cmark
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
        let mut pending_text = String::new();

        for event in parser {
            match event {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                    // Process any pending text before starting a code block
                    if !pending_text.trim().is_empty() {
                        elements.extend(self.process_inline_code(&pending_text, cx));
                        pending_text.clear();
                    }

                    in_code_block = true;
                    let lang_str = lang.to_string();
                    if lang_str.is_empty() || lang_str.trim().is_empty() {
                        code_language = "plain".to_string(); // Use "plain" for empty language
                    } else {
                        code_language = lang_str.to_lowercase();
                    }
                    code_content.clear();
                }
                Event::Start(Tag::CodeBlock(_)) => {
                    // Process any pending text before starting a code block
                    if !pending_text.trim().is_empty() {
                        elements.extend(self.process_inline_code(&pending_text, cx));
                        pending_text.clear();
                    }

                    in_code_block = true;
                    code_language = "plain".to_string(); // Use "plain" for indented code blocks
                    code_content.clear();
                }
                Event::Code(code) => {
                    // Inline code - append to pending text with markers for processing
                    pending_text.push_str(&format!("`{}`", code));
                }
                Event::End(Tag::CodeBlock(_)) => {
                    if in_code_block {
                        let code = code_content.join("\n").trim_end().to_string();

                        // Render as SQL code block with syntax highlighting
                        let shared_text = gpui::SharedString::from(code.clone());
                        let text_len = shared_text.len();
                        let range = std::ops::Range::<usize> {
                            start: 0,
                            end: text_len,
                        };

                        // Create syntax highlighter (or use plain styling for "plain" language)
                        let highlights = if code_language == "plain" {
                            // For plain code blocks, create simple monochrome highlights
                            vec![(
                                range.clone(),
                                gpui::HighlightStyle {
                                    color: Some(cx.theme().foreground.into()),
                                    background_color: None,
                                    font_weight: None,
                                    font_style: None,
                                    underline: None,
                                    strikethrough: None,
                                    fade_out: Some(0.0),
                                },
                            )]
                        } else {
                            // For language-specific code blocks, use syntax highlighting
                            let mut highlighter =
                                gpui_component::highlighter::SyntaxHighlighter::new(&code_language);
                            highlighter.update(None, &Rope::from(code.clone()));
                            let theme = cx.theme().highlight_theme.clone();
                            highlighter.styles(&range, &theme)
                        };

                        // Create unique ID for this code block's copy button
                        let copy_button_id = elements.len();

                        elements.push(
                            div()
                                .relative() // Make this the positioning context
                                .child(
                                    // Code block content
                                    div()
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
                                )
                                .child(
                                    // Copy button positioned at top right
                                    Button::new(copy_button_id)
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Copy)
                                        .absolute()
                                        .top_2()
                                        .right_2()
                                        .on_click(cx.listener(move |_this, _event, _window, cx| {
                                            // Copy the code content to clipboard
                                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                        })),
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
                        pending_text.push_str(&text);
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if in_code_block {
                        code_content.push("\n".to_string());
                    } else {
                        pending_text.push(' ');
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

        // Process any remaining text with inline code
        if !pending_text.trim().is_empty() {
            // Process inline code in the remaining text
            let inline_elements = self.process_inline_code(&pending_text, cx);
            elements.extend(inline_elements);
        }

        elements
    }

    /// Process text to find and style inline code segments using StyledText for true inline rendering
    fn process_inline_code(&self, text: &str, cx: &mut Context<Self>) -> Vec<gpui::Div> {
        use regex::Regex;

        // Regex to match inline code: `code`
        let inline_code_regex = Regex::new(r"`([^`]+)`").unwrap();

        let mut last_end = 0;
        let mut styled_segments = Vec::new();

        // Find all inline code matches
        for caps in inline_code_regex.captures_iter(text) {
            let match_obj = caps.get(0).unwrap();
            let code_match = caps.get(1).unwrap();

            // Add text before the inline code
            if match_obj.start() > last_end {
                let text_segment = &text[last_end..match_obj.start()];
                if !text_segment.is_empty() {
                    styled_segments.push((text_segment.to_string(), TextStyleType::Normal));
                }
            }

            // Add the inline code content (without backticks)
            let code_content = code_match.as_str();
            styled_segments.push((code_content.to_string(), TextStyleType::InlineCode));

            last_end = match_obj.end();
        }

        // Add any remaining text after the last inline code
        if last_end < text.len() {
            let text_segment = &text[last_end..];
            if !text_segment.is_empty() {
                styled_segments.push((text_segment.to_string(), TextStyleType::Normal));
            }
        }

        // If no inline code was found, just return the text as-is
        if styled_segments.is_empty() {
            return vec![div().text_sm().text_left().child(text.to_string())];
        }

        // Create a single div with StyledText that combines all segments
        let styled_text = self.create_styled_text(styled_segments, cx);

        vec![div().text_sm().text_left().child(styled_text)]
    }

    /// Create StyledText from segments with different styles
    fn create_styled_text(
        &self,
        segments: Vec<(String, TextStyleType)>,
        cx: &mut Context<Self>,
    ) -> gpui::StyledText {
        let mut result_text = String::new();
        let mut highlights = Vec::new();

        for (text, style_type) in segments {
            let start_pos = result_text.len();

            match style_type {
                TextStyleType::Normal => {
                    result_text.push_str(&text);
                }
                TextStyleType::InlineCode => {
                    // Add inline code text without backticks
                    result_text.push_str(&text);

                    // Add highlight for the inline code segment
                    let end_pos = result_text.len();
                    let range = start_pos..end_pos;

                    // Create highlight with inline code styling using theme colors
                    let highlight = gpui::HighlightStyle {
                        color: Some(cx.theme().primary_foreground.into()), // Use theme foreground
                        background_color: Some(cx.theme().muted.into()), // Use theme muted background
                        font_weight: None,
                        font_style: None,
                        underline: None,
                        strikethrough: None,
                        fade_out: Some(0.0), // No fade out
                    };

                    highlights.push((range, highlight));
                }
            }
        }

        // Create StyledText with highlights for inline code
        let shared_text = gpui::SharedString::from(result_text);
        if highlights.is_empty() {
            gpui::StyledText::new(shared_text)
        } else {
            gpui::StyledText::new(shared_text).with_highlights(highlights)
        }
    }
}

/// Types of text styles for different segments
#[derive(Debug, Clone, PartialEq)]
enum TextStyleType {
    Normal,
    InlineCode,
}

#[cfg(test)]
mod tests {
    use super::TextStyleType;
    use regex::Regex;

    #[test]
    fn test_inline_code_regex_patterns() {
        // Test inline code regex pattern
        let inline_code_regex = Regex::new(r"`([^`]+)`").unwrap();

        let test_cases = vec![
            ("Here is `simple` code", vec!["simple"]),
            ("Here is `code with spaces` in it", vec!["code with spaces"]),
            (
                "Multiple `inline` code `segments` here",
                vec!["inline", "segments"],
            ),
            ("Edge `case` at start", vec!["case"]),
            ("Edge case at `end`", vec!["end"]),
            ("`code` at start", vec!["code"]),
            ("code at `end`", vec!["end"]),
        ];

        for (input, expected_matches) in test_cases {
            let matches: Vec<_> = inline_code_regex
                .captures_iter(input)
                .map(|caps| caps.get(1).unwrap().as_str())
                .collect();

            assert_eq!(
                matches.len(),
                expected_matches.len(),
                "Should find {} matches in: {}",
                expected_matches.len(),
                input
            );
            for (i, expected_code) in expected_matches.iter().enumerate() {
                assert_eq!(
                    matches[i], *expected_code,
                    "Match {} should be correct in: {}",
                    i, input
                );
            }
        }
    }

    #[test]
    fn test_inline_code_replacement() {
        let inline_code_regex = Regex::new(r"`([^`]+)`").unwrap();

        let input = "Here is `inline code` that should be styled.";
        let result = inline_code_regex.replace_all(input, "[$1]");

        assert_eq!(result, "Here is [inline code] that should be styled.");
        println!("Original: {}", input);
        println!("Replaced: {}", result);
    }

    #[test]
    fn test_mixed_content_parsing() {
        let content = "Here is `inline code` and a code block:\n\n```sql\nSELECT * FROM users;\n```\n\nMore text with `more inline code`.";

        // Test that we can distinguish between inline code and code blocks
        // Inline code: `inline code`, `more inline code`
        // Code block: ```sql\nSELECT * FROM users;\n```

        let has_inline_code =
            content.contains("`inline code`") && content.contains("`more inline code`");
        let has_code_block = content.contains("```sql") && content.contains("SELECT * FROM users;");

        assert!(has_inline_code, "Should contain inline code");
        assert!(has_code_block, "Should contain code block");

        println!("Content has both inline code and code blocks: {}", content);
    }

    #[test]
    fn test_inline_code_backtick_removal() {
        let inline_code_regex = Regex::new(r"`([^`]+)`").unwrap();

        let input = "Here is `inline code` that should have backticks removed.";
        let result = inline_code_regex.replace_all(input, "$1");

        assert_eq!(
            result,
            "Here is inline code that should have backticks removed."
        );
        println!("Original with backticks: {}", input);
        println!("Cleaned text: {}", result);
    }

    #[test]
    fn test_styled_text_segments() {
        // Test that we can create segments for StyledText
        let segments = vec![
            ("Here is ".to_string(), TextStyleType::Normal),
            ("inline".to_string(), TextStyleType::InlineCode),
            (" code in text.".to_string(), TextStyleType::Normal),
        ];

        let mut expected_result = String::new();
        for (text, _style) in &segments {
            expected_result.push_str(text);
        }

        assert_eq!(expected_result, "Here is inline code in text.");
        println!("Styled segments result: {}", expected_result);
    }

    #[test]
    fn test_mixed_inline_code_processing() {
        let inline_code_regex = Regex::new(r"`([^`]+)`").unwrap();
        let input = "Use `SELECT` to query and `INSERT` to add data.";

        // Simulate the segment processing
        let mut segments = Vec::new();
        let mut last_end = 0;

        for caps in inline_code_regex.captures_iter(input) {
            let match_obj = caps.get(0).unwrap();
            let code_match = caps.get(1).unwrap();

            // Add text before inline code
            if match_obj.start() > last_end {
                let text_segment = &input[last_end..match_obj.start()];
                if !text_segment.is_empty() {
                    segments.push((text_segment.to_string(), TextStyleType::Normal));
                }
            }

            // Add inline code
            segments.push((code_match.as_str().to_string(), TextStyleType::InlineCode));
            last_end = match_obj.end();
        }

        // Add remaining text
        if last_end < input.len() {
            let text_segment = &input[last_end..];
            if !text_segment.is_empty() {
                segments.push((text_segment.to_string(), TextStyleType::Normal));
            }
        }

        // Verify segments
        assert_eq!(segments.len(), 5); // "Use ", "SELECT", " to query and ", "INSERT", " to add data."

        let reconstructed: String = segments.iter().map(|(text, _)| text.clone()).collect();

        assert_eq!(reconstructed, "Use SELECT to query and INSERT to add data.");
        println!("Processed segments: {:?}", segments);
        println!("Reconstructed text: {}", reconstructed);
    }

    #[test]
    fn test_plain_code_block_detection() {
        use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

        // Test plain code block detection (no language specified)
        let plain_content = "```\nhello world\n```";
        let parser = Parser::new(plain_content);

        let mut found_plain_block = false;
        let mut code_content = Vec::new();

        for event in parser {
            match event {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                    let lang_str = lang.to_string();
                    let code_language = if lang_str.is_empty() || lang_str.trim().is_empty() {
                        "plain".to_string()
                    } else {
                        lang_str.to_lowercase()
                    };
                    assert_eq!(
                        code_language, "plain",
                        "Empty language should be treated as 'plain'"
                    );
                    found_plain_block = true;
                }
                Event::Text(text) => {
                    code_content.push(text.to_string());
                }
                Event::End(Tag::CodeBlock(_)) => {
                    // End of code block
                }
                _ => {}
            }
        }

        assert!(found_plain_block, "Should detect plain code block");
        assert_eq!(
            code_content.join(""),
            "hello world\n",
            "Should capture code content correctly"
        );
        println!("Plain code block detected and content: {:?}", code_content);
    }

    #[test]
    fn test_mixed_code_content() {
        // Test content with both inline code and plain code blocks
        let content =
            "Here is `inline code` and a plain block:\n\n```\nplain code block\n```\n\nMore text.";

        let has_inline_code = content.contains("`inline code`");
        let has_plain_code_block = content.contains("```\nplain code block\n```");

        assert!(has_inline_code, "Should contain inline code");
        assert!(has_plain_code_block, "Should contain plain code block");

        println!("Mixed content test passed - contains both inline code and plain code blocks");
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
                                                                        "I notice your `query` is doing a \n\n```\nfull table scan\n```\n. Here's a more efficient version:\n\n```sql\nSELECT u.id, u.name, u.email, COUNT(o.id) as order_count\nFROM users u\nLEFT JOIN orders o ON u.id = o.user_id\nWHERE u.created_at >= '2024-01-01'\nGROUP BY u.id, u.name, u.email\nHAVING COUNT(o.id) > 5\nORDER BY order_count DESC\nLIMIT 10;\n```\n\nThis adds an index on `created_at` and uses proper JOIN syntax for better performance.",
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
                            // Message content
                            v_flex()
                                .flex_1()
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
                        )
                    }))
                    // Loading indicator
                    .when(self.is_loading, |this| {
                        this.child(
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
