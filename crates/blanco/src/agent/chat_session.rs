use anyhow::Result;
use async_std::channel::Sender;
use futures::StreamExt;
use gpui::{Context, EventEmitter, Task};
use std::sync::Arc;

use super::chat_types::{ChatCommand, ChatEvent, ChatMessage, MessageRole, SqlContext};
use blanco_core::chat_provider::{ChatCompletionRequest, ChatProvider, ProviderError};

#[derive(Clone)]
pub struct ChatSession {
    pub messages: Vec<ChatMessage>,
    pub provider: Option<Arc<dyn ChatProvider<Error = ProviderError>>>,
    pub model_name: String,
    pub provider_name: String,
    #[allow(dead_code)]
    pub is_loading: bool,
    pub sql_context: SqlContext,
    #[allow(dead_code)]
    pub streaming_message_id: Option<String>,
}

#[allow(dead_code)]
impl ChatSession {
    /// Create a new ChatSession with a provider
    pub fn new(
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        provider_name: String,
        model_name: String,
    ) -> Self {
        Self {
            messages: Vec::new(),
            provider: Some(provider),
            provider_name,
            model_name,
            is_loading: false,
            sql_context: SqlContext::empty(),
            streaming_message_id: None,
        }
    }

    /// Create a mock ChatSession for testing
    pub fn with_mock() -> Self {
        Self {
            messages: Vec::new(),
            provider: None,
            provider_name: "Mock".to_string(),
            model_name: "mock-gpt-4".to_string(),
            is_loading: false,
            sql_context: SqlContext::empty(),
            streaming_message_id: None,
        }
    }

    /// Create a ChatSession without a provider (for deferred initialization)
    pub fn new_empty() -> Self {
        Self {
            messages: Vec::new(),
            provider: None,
            provider_name: "Unknown".to_string(),
            model_name: "unknown".to_string(),
            is_loading: false,
            sql_context: SqlContext::empty(),
            streaming_message_id: None,
        }
    }

    /// Set the provider after creation
    pub fn set_provider(
        &mut self,
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        provider_name: String,
        model_name: String,
    ) {
        self.provider = Some(provider);
        self.provider_name = provider_name;
        self.model_name = model_name;
    }

    pub fn add_message(&mut self, message: ChatMessage, cx: &mut Context<Self>) {
        self.messages.push(message.clone());

        // Emit message added event
        cx.emit(ChatEvent::MessageAdded { message });
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.streaming_message_id = None;
    }

    pub fn update_sql_context(&mut self, context: SqlContext) {
        self.sql_context = context;
    }

    pub fn get_system_prompt(&self) -> String {
        self.sql_context.to_system_prompt()
    }

    pub fn send_message(
        &mut self,
        user_message: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let message = ChatMessage::user(user_message.clone());
        self.add_message(message.clone(), cx);

        // Check if this is a command
        if let Some(command) = ChatCommand::parse(&user_message) {
            return self.handle_command(command, cx);
        }

        // Handle regular message based on available provider
        if let Some(provider) = &self.provider {
            self.send_provider_message(provider.clone(), &user_message, cx)
        } else {
            Task::ready(Err(anyhow::anyhow!("no provider")))
        }
    }

    fn handle_command(
        &mut self,
        command: ChatCommand,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let response = match command {
            ChatCommand::Help => ChatCommand::help_text().to_string(),
            ChatCommand::Clear => {
                self.clear_messages();
                cx.emit(ChatEvent::SessionCleared);
                "Chat history cleared.".to_string()
            }
            ChatCommand::Explain => {
                if self.sql_context.current_query.is_empty() {
                    "No SQL query to explain. Please write a query first.".to_string()
                } else {
                    format!(
                        "Analyzing your SQL query:\n```\n{}\n```\n\nThis query appears to be a valid SQL statement. Here's what I can tell you:\n\n* The query is syntactically correct\n* It targets the current database context\n* Execution would depend on your table structure\n\nWould you like me to suggest optimizations or help debug any issues?",
                        self.sql_context.current_query
                    )
                }
            }
            ChatCommand::Optimize => {
                if self.sql_context.current_query.is_empty() {
                    "No SQL query to optimize. Please write a query first.".to_string()
                } else {
                    format!(
                        r"Here are some optimization suggestions for your query:

```
{}
```

**Performance Tips:**

1. **Indexes**: Consider adding indexes on frequently filtered columns
2. **SELECT ***: Avoid selecting all columns - specify only what you need
3. **WHERE clauses**: Add proper filtering to reduce result sets
4. **JOINs**: Ensure join columns are indexed
5. **EXPLAIN**: Use `EXPLAIN` to analyze the query execution plan

Would you like me to help you implement any of these optimizations?",
                        self.sql_context.current_query
                    )
                }
            }
            ChatCommand::Fix => {
                if let Some(error) = &self.sql_context.error_message {
                    format!(
                        "I see there's an error in your SQL query:\n\n**Error:** {}\n\n**Current Query:**\n```\n{}\n```\n\n**Common Issues to Check:**\n\n1. **Syntax errors** - Check for missing commas, parentheses, or keywords\n2. **Table/Column names** - Verify they exist in your schema\n3. **Data types** - Ensure compatible data types in operations\n4. **Reserved words** - Some words need to be quoted\n\nWould you like me to help you fix this specific error?",
                        error, self.sql_context.current_query
                    )
                } else {
                    "No recent error found. Please run your query first so I can help debug any issues.".to_string()
                }
            }
            ChatCommand::Schema(table_name) => {
                if let Some(table) = table_name {
                    format!("Showing schema for table: {}\n\n*Schema information would be displayed here with columns, data types, and constraints.*\n\nThis will be implemented once database schema introspection is available.", table)
                } else if self.sql_context.tables.is_empty() {
                    "No table information available. Connect to a database first to see available tables.".to_string()
                } else {
                    let mut response = "Available tables:\n\n".to_string();
                    for table in &self.sql_context.tables {
                        response.push_str(&format!(
                            "• {}.{} ({} columns)\n",
                            table.schema.as_deref().unwrap_or("public"),
                            table.name,
                            table.columns.len()
                        ));
                    }
                    response.push_str(
                        "\nUse `/schema <table_name>` to see detailed schema for a specific table.",
                    );
                    response
                }
            }
            ChatCommand::Export => {
                let mut export = "Chat History Export\n".to_string();
                export.push_str(&format!(
                    "Generated: {}\n",
                    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
                ));
                export.push_str(&format!(
                    "Provider: {} ({})\n\n",
                    self.provider_name, self.model_name
                ));

                for message in &self.messages {
                    let role = match message.role {
                        MessageRole::User => "You",
                        MessageRole::Assistant => "Assistant",
                        MessageRole::System => "System",
                        MessageRole::Tool => "Tool",
                    };
                    export.push_str(&format!(
                        "**{}** ({}):\n{}\n\n",
                        role,
                        message.timestamp.format("%H:%M:%S"),
                        message.content
                    ));
                }

                export.push_str("---\nExported from Blanco SQL Editor");
                export
            }
        };

        let message = ChatMessage::assistant(response.clone(), self.model_name.clone());
        self.add_message(message.clone(), cx);
        cx.emit(ChatEvent::MessageAdded { message });

        Task::ready(Ok(response))
    }

    fn send_provider_message(
        &mut self,
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        user_message: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let system_prompt = self.sql_context.to_system_prompt();
        let messages = self.messages.clone();
        let model_name = self.model_name.clone();
        let user_message = user_message.to_string();
        let provider_clone = provider.clone();

        cx.spawn(async move |chat_session_handle, async_cx| {
            // Create an async channel for real-time UI updates
            let (tx, mut rx) = async_std::channel::unbounded::<ChatMessage>();

            // Spawn a task to listen for UI updates
            let handle_clone = chat_session_handle.clone();
            async_cx
                .spawn(async move |cx| {
                    while let Some(message) = rx.next().await {
                        if let Ok(_) = handle_clone.update(cx, |chat_session, cx| {
                            chat_session.add_message(message.clone(), cx);
                        }) {
                            // State updated successfully
                        }
                    }
                })
                .detach();

            // Process the message loop with real-time UI updates
            let response = Self::process_message_loop_with_realtime_ui(
                provider_clone,
                system_prompt,
                messages,
                model_name.clone(),
                user_message,
                tx,
            )
            .await?;

            Ok(response)
        })
    }

    /// Process messages in a loop to handle dynamic tool call sequences with real-time UI updates
    async fn process_message_loop_with_realtime_ui(
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        system_prompt: String,
        initial_messages: Vec<ChatMessage>,
        model_name: String,
        user_message: String,
        ui_sender: Sender<ChatMessage>,
    ) -> Result<String> {
        // Build initial request messages with conversation history
        let mut request_messages =
            Self::build_request_messages(&system_prompt, &initial_messages, &user_message);

        let mut loop_count = 0;
        const MAX_LOOP_ITERATIONS: usize = 10; // Prevent infinite loops

        loop {
            // Safety check to prevent infinite loops
            if loop_count >= MAX_LOOP_ITERATIONS {
                return Err(anyhow::anyhow!(
                    "Maximum tool call iterations ({}) exceeded. This may indicate a loop in tool calls.",
                    MAX_LOOP_ITERATIONS
                ));
            }
            loop_count += 1;

            log::debug!("Starting message loop iteration {}", loop_count);

            // Create and send request
            let request = ChatCompletionRequest {
                model: model_name.clone(),
                messages: request_messages.clone(),
                stream: false,
                temperature: 0.7,
                max_tokens: Some(2048),
                tools: provider.get_tools(),
                tool_choice: None,
                top_p: None,
                frequency_penalty: None,
                presence_penalty: None,
                additional_params: None,
            };

            let response = provider.chat_completion(request).await.map_err(|e| {
                anyhow::anyhow!("Chat completion failed on iteration {}: {}", loop_count, e)
            })?;

            let choice = response.choices.first().ok_or_else(|| {
                anyhow::anyhow!("No response content received on iteration {}", loop_count)
            })?;

            log::debug!(
                "Response received on iteration {}, finish reason: {:?}",
                loop_count,
                choice.finish_reason
            );

            // Check if we need to process tool calls
            match choice.finish_reason {
                blanco_core::chat_provider::FinishReason::ToolCalls => {
                    let tool_calls_count = choice
                        .message
                        .tool_calls
                        .as_ref()
                        .map(|t| t.len())
                        .unwrap_or(0);

                    log::debug!(
                        "Tool calls detected on iteration {}, processing {} tool calls",
                        loop_count,
                        tool_calls_count
                    );

                    if tool_calls_count == 0 {
                        return Err(anyhow::anyhow!(
                            "Tool calls indicated but no tool calls found on iteration {}",
                            loop_count
                        ));
                    }

                    // Process tool calls with real-time UI updates
                    request_messages = Self::process_tool_calls_with_realtime_ui(
                        &provider,
                        &choice.message,
                        request_messages,
                        &model_name,
                        &ui_sender,
                    )
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Failed to process tool calls on iteration {}: {}",
                            loop_count,
                            e
                        )
                    })?;

                    log::debug!(
                        "Completed processing {} tool calls on iteration {}, total messages: {}",
                        tool_calls_count,
                        loop_count,
                        request_messages.len()
                    );
                }
                blanco_core::chat_provider::FinishReason::Stop => {
                    // Normal completion
                    log::debug!("Normal completion received on iteration {}", loop_count);
                    let final_message =
                        ChatMessage::assistant(choice.message.content.clone(), model_name);
                    let _ = ui_sender.send(final_message).await;
                    return Ok(choice.message.content.clone());
                }
                blanco_core::chat_provider::FinishReason::Length => {
                    return Err(anyhow::anyhow!(
                        "Response too long on iteration {}. Consider breaking down the request.",
                        loop_count
                    ));
                }
                blanco_core::chat_provider::FinishReason::ContentFilter => {
                    return Err(anyhow::anyhow!(
                        "Content filtered by provider on iteration {}",
                        loop_count
                    ));
                }
                _ => {
                    // Handle any other finish reasons
                    log::debug!(
                        "Unknown finish reason {:?} on iteration {}, treating as completion",
                        choice.finish_reason,
                        loop_count
                    );
                    let final_message =
                        ChatMessage::assistant(choice.message.content.clone(), model_name);
                    let _ = ui_sender.send(final_message).await;
                    return Ok(choice.message.content.clone());
                }
            }
        }
    }

    /// Process tool calls with real-time UI updates
    async fn process_tool_calls_with_realtime_ui(
        provider: &Arc<dyn ChatProvider<Error = ProviderError>>,
        assistant_message: &blanco_core::chat_provider::Message,
        mut current_messages: Vec<blanco_core::chat_provider::Message>,
        model_name: &str,
        ui_sender: &Sender<ChatMessage>,
    ) -> Result<Vec<blanco_core::chat_provider::Message>> {
        let tool_calls = assistant_message
            .tool_calls
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Tool calls indicated but no tool calls found"))?;

        if tool_calls.is_empty() {
            return Err(anyhow::anyhow!("Tool calls array is empty"));
        }

        let mut successful_tool_calls = 0;
        let mut failed_tool_calls = 0;

        // Create tool call data for UI display
        let tool_call_data: Vec<crate::agent::chat_types::ToolCallData> = tool_calls
            .iter()
            .map(|tc| {
                crate::agent::chat_types::ToolCallData {
                    id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                    result: None, // Will be filled after execution
                }
            })
            .collect();

        // Create and immediately emit assistant message with tool calls
        let assistant_ui_message =
            ChatMessage::assistant(assistant_message.content.clone(), model_name.to_string())
                .with_tool_calls(tool_call_data.clone());
        let _ = ui_sender.send(assistant_ui_message).await;

        // Add the assistant's tool call message to the conversation
        current_messages.push(blanco_core::chat_provider::Message {
            role: "assistant".to_string(),
            content: assistant_message.content.clone(),
            tool_call_id: None,
            tool_calls: Some(tool_calls.clone()),
            additional_data: None,
        });

        // Execute each tool call individually with real-time updates
        for (index, tool_call) in tool_calls.iter().enumerate() {
            log::debug!(
                "Executing tool call {}/{}: {} with args: {}",
                index + 1,
                tool_calls.len(),
                tool_call.function.name,
                tool_call.function.arguments
            );

            let tool_call = tool_call.clone();
            let tool_call_clone = tool_call.clone();

            match provider.call_tool(tool_call).await {
                Ok(result) => {
                    log::debug!(
                        "Tool call {}/{} succeeded - success: {}, content length: {}",
                        index + 1,
                        tool_calls.len(),
                        result.success,
                        result.content.len()
                    );

                    // Create and immediately emit tool result message for UI
                    let tool_ui_message = ChatMessage::tool(
                        result.content.clone(),
                        result.tool_call_id.clone(),
                        model_name.to_string(),
                    );
                    let _ = ui_sender.send(tool_ui_message).await;

                    // Add tool result as a message to conversation
                    current_messages.push(blanco_core::chat_provider::Message {
                        role: "tool".to_string(),
                        content: result.content,
                        tool_call_id: Some(result.tool_call_id),
                        tool_calls: None,
                        additional_data: None,
                    });

                    successful_tool_calls += 1;
                }
                Err(e) => {
                    log::error!("Tool call {}/{} failed: {}", index + 1, tool_calls.len(), e);

                    // Create and immediately emit error message for UI
                    let error_content = format!("Tool call failed: {}", e);
                    let tool_ui_message = ChatMessage::tool(
                        error_content.clone(),
                        tool_call_clone.id.clone(),
                        model_name.to_string(),
                    );
                    let _ = ui_sender.send(tool_ui_message).await;

                    // Add error result as a message to conversation
                    current_messages.push(blanco_core::chat_provider::Message {
                        role: "tool".to_string(),
                        content: error_content,
                        tool_call_id: Some(tool_call_clone.id.clone()),
                        tool_calls: None,
                        additional_data: None,
                    });

                    failed_tool_calls += 1;
                }
            }
        }

        log::debug!(
            "Processed {} tool calls: {} successful, {} failed, total messages: {}",
            tool_calls.len(),
            successful_tool_calls,
            failed_tool_calls,
            current_messages.len()
        );

        // If all tool calls failed, return an error
        if failed_tool_calls == tool_calls.len() {
            return Err(anyhow::anyhow!(
                "All {} tool calls failed",
                tool_calls.len()
            ));
        }

        // If some tool calls failed, log a warning but continue
        if failed_tool_calls > 0 {
            log::warn!(
                "{} out of {} tool calls failed",
                failed_tool_calls,
                tool_calls.len()
            );
        }

        Ok(current_messages)
    }

    /// Build initial request messages from conversation history
    fn build_request_messages(
        system_prompt: &str,
        messages: &[ChatMessage],
        user_message: &str,
    ) -> Vec<blanco_core::chat_provider::Message> {
        let mut request_messages = vec![blanco_core::chat_provider::Message {
            role: "system".to_string(),
            content: system_prompt.to_string(),
            tool_call_id: None,
            tool_calls: None,
            additional_data: None,
        }];

        // Add conversation history
        for message in messages {
            let provider_message = match message.role {
                MessageRole::User => blanco_core::chat_provider::Message {
                    role: "user".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: None,
                    tool_calls: None,
                    additional_data: None,
                },
                MessageRole::Assistant => blanco_core::chat_provider::Message {
                    role: "assistant".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: None,
                    tool_calls: None,
                    additional_data: None,
                },
                MessageRole::System => blanco_core::chat_provider::Message {
                    role: "system".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: None,
                    tool_calls: None,
                    additional_data: None,
                },
                MessageRole::Tool => blanco_core::chat_provider::Message {
                    role: "tool".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: message.tool_call_id.clone(),
                    tool_calls: None,
                    additional_data: None,
                },
            };
            request_messages.push(provider_message);
        }

        // Add the current user message
        request_messages.push(blanco_core::chat_provider::Message {
            role: "user".to_string(),
            content: user_message.to_string(),
            tool_call_id: None,
            tool_calls: None,
            additional_data: None,
        });

        request_messages
    }

    pub fn get_last_assistant_message(&self) -> Option<&ChatMessage> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
    }

    pub fn get_last_user_message(&self) -> Option<&ChatMessage> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

impl EventEmitter<ChatEvent> for ChatSession {}
