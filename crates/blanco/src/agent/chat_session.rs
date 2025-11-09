use anyhow::Result;
use async_std::task::sleep;
use gpui::{Context, EventEmitter, Task};
use std::sync::Arc;
use std::time::Duration;

use super::chat_types::{ChatCommand, ChatEvent, ChatMessage, MessageRole, SqlContext};
use blanco_core::chat_provider::{
    ChatCompletionRequest, ChatProvider, Message as ProviderMessage, ProviderError,
};

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

        cx.spawn(async move |chat_session_handle, async_cx| {
            // Clone model_name for use in the async closure
            let model_name_for_tools = model_name.clone();

            // Build the request using the provider
            let mut request_messages = vec![
                ProviderMessage {
                    role: "system".to_string(),
                    content: system_prompt,
                    tool_call_id: None,
                    tool_calls: None,
                    additional_data: None,
                },
            ];

            // Add conversation history
            for message in &messages {
                let provider_message = match message.role {
                    MessageRole::User => ProviderMessage {
                        role: "user".to_string(),
                        content: message.content.clone(),
                        tool_call_id: None,
                        tool_calls: None,
                        additional_data: None,
                    },
                    MessageRole::Assistant => ProviderMessage {
                        role: "assistant".to_string(),
                        content: message.content.clone(),
                        tool_call_id: None,
                        tool_calls: None,
                        additional_data: None,
                    },
                    MessageRole::System => ProviderMessage {
                        role: "system".to_string(),
                        content: message.content.clone(),
                        tool_call_id: None,
                        tool_calls: None,
                        additional_data: None,
                    },
                    MessageRole::Tool => ProviderMessage {
                        role: "tool".to_string(),
                        content: "".to_string(), // message.content.clone(),
                        tool_call_id: message.tool_call_id.clone(),
                        tool_calls: None,
                        additional_data: None,
                    },
                };
                request_messages.push(provider_message);
            }

            // Add the current user message
            request_messages.push(ProviderMessage {
                role: "user".to_string(),
                content: user_message,
                tool_call_id: None,
                tool_calls: None,
                additional_data: None,
            });

            // Get tools from the provider if available
            let tools = provider.get_tools();

            // Clone values for potential follow-up requests
            let model_name_clone = model_name.clone();
            let model_name_clone2 = model_name.clone();
            let request_messages_clone = request_messages.clone();
            let request_messages_clone2 = request_messages.clone();
            let provider_clone = provider.clone();

            let request = ChatCompletionRequest {
                model: model_name,
                messages: request_messages,
                stream: false,
                temperature: 0.7,
                max_tokens: Some(2048),
                tools,
                tool_choice: None,
                top_p: None,
                frequency_penalty: None,
                presence_penalty: None,
                additional_params: None,
            };

            // Send the request using the provider
            let response = provider_clone.chat_completion(request).await
                .map_err(|e| anyhow::anyhow!("Chat completion failed: {}", e))?;

            if let Some(choice) = response.choices.first() {
                // Handle tool calls
                match choice.finish_reason {
                    blanco_core::chat_provider::FinishReason::ToolCalls => {
                        log::debug!("Tool calls detected, processing {} tool calls",
                                   choice.message.tool_calls.as_ref().map(|t| t.len()).unwrap_or(0));

                        // Handle tool calls
                        if let Some(tool_calls) = &choice.message.tool_calls {
                            // Create tool call data for UI display
                            let tool_call_data: Vec<crate::agent::chat_types::ToolCallData> = tool_calls.iter().map(|tc| {
                                crate::agent::chat_types::ToolCallData {
                                    id: tc.id.clone(),
                                    tool_name: tc.function.name.clone(),
                                    arguments: tc.function.arguments.clone(),
                                    result: None, // Will be filled after execution
                                }
                            }).collect();

                            // Add assistant message with tool calls to the chat
                            let assistant_message = ChatMessage::assistant(
                                choice.message.content.clone(),
                                model_name_clone.clone()
                            ).with_tool_calls(tool_call_data.clone());

                            // Update state via the weak handle
                            if let Ok(_) = chat_session_handle.update(async_cx, |chat_session, cx| {
                                chat_session.add_message(assistant_message.clone(), cx);
                            }) {
                                // State updated successfully
                            }

                            // Execute each tool call
                            let mut tool_results = Vec::new();
                            for tool_call in tool_calls {
                                // Tool call execution without connection injection for now
                                let tool_call = tool_call.clone();

                                log::debug!("Executing tool call: {} with args: {}", tool_call.function.name, tool_call.function.arguments);
                                let result = provider_clone.call_tool(tool_call).await
                                    .map_err(|e| anyhow::anyhow!("Tool call failed: {}", e))?;
                                log::debug!("Tool call result - success: {}, content length: {}", result.success, result.content.len());

                                // Create tool result message
                                let tool_message = ChatMessage::tool(
                                    result.content.clone(),
                                    result.tool_call_id.clone(),
                                    model_name_clone.clone()
                                );

                                // Update state via the weak handle
                                if let Ok(_) = chat_session_handle.update(async_cx, |chat_session, cx| {
                                    chat_session.add_message(tool_message.clone(), cx);
                                }) {
                                    // State updated successfully
                                }

                                tool_results.push(result);
                            }

                            // Create a new request with the assistant message and tool results
                            let mut follow_up_messages = request_messages_clone2;

                            // Add the assistant's tool call message
                            follow_up_messages.push(blanco_core::chat_provider::Message {
                                role: "assistant".to_string(),
                                content: choice.message.content.clone(),
                                tool_call_id: None,
                                tool_calls: Some(tool_calls.clone()),
                                additional_data: None,
                            });

                            // Add each tool result as a message
                            for tool_result in tool_results {
                                follow_up_messages.push(blanco_core::chat_provider::Message {
                                    role: "tool".to_string(),
                                    content: tool_result.content,
                                    tool_call_id: Some(tool_result.tool_call_id),
                                    tool_calls: None,
                                    additional_data: None,
                                });
                            }

                            log::debug!("Sending follow-up request with {} messages (including tool results)", follow_up_messages.len());

                            // Send a follow-up request to get the final response
                            let follow_up_request = blanco_core::chat_provider::ChatCompletionRequest {
                                model: model_name_clone,
                                messages: follow_up_messages,
                                stream: false,
                                temperature: 0.7,
                                max_tokens: Some(2048),
                                tools: provider_clone.get_tools(),
                                tool_choice: None,
                                top_p: None,
                                frequency_penalty: None,
                                presence_penalty: None,
                                additional_params: None,
                            };

                            let follow_up_response = provider.chat_completion(follow_up_request).await
                                .map_err(|e| anyhow::anyhow!("Follow-up chat completion failed: {}", e))?;

                            log::debug!("Follow-up response received, choices: {}", follow_up_response.choices.len());

                            if let Some(follow_up_choice) = follow_up_response.choices.first() {
                                log::debug!("Follow-up finish reason: {:?}", follow_up_choice.finish_reason);

                                // Handle the case where follow-up response also contains tool calls
                                match follow_up_choice.finish_reason {
                                    blanco_core::chat_provider::FinishReason::ToolCalls => {
                                        log::debug!("Follow-up also contains tool calls, processing them...");

                                        if let Some(follow_up_tool_calls) = &follow_up_choice.message.tool_calls {
                                            // Process the new tool calls
                                            let mut follow_up_tool_results = Vec::new();
                                            for tool_call in follow_up_tool_calls {
                                                let tool_call = tool_call.clone();

                                                log::debug!("Executing follow-up tool call: {} with args: {}", tool_call.function.name, tool_call.function.arguments);
                                                let result = provider_clone.call_tool(tool_call).await
                                                    .map_err(|e| anyhow::anyhow!("Follow-up tool call failed: {}", e))?;
                                                log::debug!("Follow-up tool call result - success: {}, content length: {}", result.success, result.content.len());

                                                // Create tool result message
                                                let tool_message = ChatMessage::tool(
                                                    result.content.clone(),
                                                    result.tool_call_id.clone(),
                                                    model_name_for_tools.clone()
                                                );

                                                // Update state via the weak handle
                                                if let Ok(_) = chat_session_handle.update(async_cx, |chat_session, cx| {
                                                    chat_session.add_message(tool_message.clone(), cx);
                                                }) {
                                                    // State updated successfully
                                                }

                                                follow_up_tool_results.push(result);
                                            }

                                            // Create another follow-up request with the new tool results
                                            let mut final_follow_up_messages = request_messages_clone.clone();

                                            // Add the assistant's follow-up tool call message
                                            final_follow_up_messages.push(blanco_core::chat_provider::Message {
                                                role: "assistant".to_string(),
                                                content: follow_up_choice.message.content.clone(),
                                                tool_call_id: None,
                                                tool_calls: Some(follow_up_tool_calls.clone()),
                                                additional_data: None,
                                            });

                                            // Add each new tool result as a message
                                            for tool_result in follow_up_tool_results {
                                                final_follow_up_messages.push(blanco_core::chat_provider::Message {
                                                    role: "tool".to_string(),
                                                    content: tool_result.content,
                                                    tool_call_id: Some(tool_result.tool_call_id),
                                                    tool_calls: None,
                                                    additional_data: None,
                                                });
                                            }

                                            log::debug!("Sending final follow-up request with {} messages", final_follow_up_messages.len());

                                            // Send final follow-up request
                                            let final_follow_up_request = blanco_core::chat_provider::ChatCompletionRequest {
                                                model: model_name_clone2,
                                                messages: final_follow_up_messages,
                                                stream: false,
                                                temperature: 0.7,
                                                max_tokens: Some(2048),
                                                tools: provider_clone.get_tools(),
                                                tool_choice: None,
                                                top_p: None,
                                                frequency_penalty: None,
                                                presence_penalty: None,
                                                additional_params: None,
                                            };

                                            let final_follow_up_response = provider.chat_completion(final_follow_up_request).await
                                                .map_err(|e| anyhow::anyhow!("Final follow-up chat completion failed: {}", e))?;

                                            if let Some(final_choice) = final_follow_up_response.choices.first() {
                                                log::debug!("Final follow-up finish reason: {:?}", final_choice.finish_reason);
                                                Ok(final_choice.message.content.clone())
                                            } else {
                                                Err(anyhow::anyhow!("No final follow-up response content received"))
                                            }
                                        } else {
                                            Err(anyhow::anyhow!("Follow-up tool calls indicated but no tool calls found"))
                                        }
                                    }
                                    _ => {
                                        // Normal response, return content
                                        Ok(follow_up_choice.message.content.clone())
                                    }
                                }
                            } else {
                                Err(anyhow::anyhow!("No follow-up response content received"))
                            }
                        } else {
                            Err(anyhow::anyhow!("Tool calls indicated but no tool calls found"))
                        }
                    }
                    _ => {
                        // Handle normal responses
                        Ok(choice.message.content.clone())
                    }
                }
            } else {
                Err(anyhow::anyhow!("No response content received"))
            }
        })
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
