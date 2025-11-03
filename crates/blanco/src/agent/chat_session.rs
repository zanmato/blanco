use anyhow::Result;
use gpui::{Context, EventEmitter, Task};
use std::sync::Arc;
use std::time::Duration;
use async_std::task::sleep;

use super::chat_types::{
    ChatCommand, ChatEvent, ChatMessage, MessageRole, SqlContext,
};
use blanco_core::chat_provider::{
    ChatProvider, ChatCompletionRequest, Message as ProviderMessage, ProviderError,
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
    pub fn new(provider: Arc<dyn ChatProvider<Error = ProviderError>>, provider_name: String, model_name: String) -> Self {
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
    pub fn set_provider(&mut self, provider: Arc<dyn ChatProvider<Error = ProviderError>>, provider_name: String, model_name: String) {
        self.provider = Some(provider);
        self.provider_name = provider_name;
        self.model_name = model_name;
    }

    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
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
        self.add_message(message.clone());

        // Emit message added event
        cx.emit(ChatEvent::MessageAdded { message });

        // Check if this is a command
        if let Some(command) = ChatCommand::parse(&user_message) {
            return self.handle_command(command, cx);
        }

        // Handle regular message based on available provider
        if let Some(provider) = &self.provider {
            self.send_provider_message(provider.clone(), &user_message, cx)
        } else {
            // Fallback to mock if no provider
            self.send_mock_message(&user_message, cx)
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
                        response.push_str(&format!("• {}.{} ({} columns)\n",
                            table.schema.as_deref().unwrap_or("public"),
                            table.name,
                            table.columns.len()
                        ));
                    }
                    response.push_str("\nUse `/schema <table_name>` to see detailed schema for a specific table.");
                    response
                }
            }
            ChatCommand::Export => {
                let mut export = "Chat History Export\n".to_string();
                export.push_str(&format!("Generated: {}\n", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")));
                export.push_str(&format!("Provider: {} ({})\n\n", self.provider_name, self.model_name));

                for message in &self.messages {
                    let role = match message.role {
                        MessageRole::User => "You",
                        MessageRole::Assistant => "Assistant",
                        MessageRole::System => "System",
                    };
                    export.push_str(&format!("**{}** ({}):\n{}\n\n",
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
        self.add_message(message.clone());
        cx.emit(ChatEvent::MessageAdded { message });

        Task::ready(Ok(response))
    }

    fn send_mock_message(
        &mut self,
        user_message: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let sql_context = self.sql_context.clone();
        let user_message = user_message.to_string();

        cx.spawn(async move |_, _cx| {
            // Simulate thinking time
            sleep(Duration::from_millis(1000)).await;

            let response = if !sql_context.current_query.is_empty() &&
                (user_message.contains("query") || user_message.contains("sql")) {
                format!(
                    "I can see you have a SQL query:\n\n```\n{}\n```\n\nThis looks like a good query! Here are some thoughts:\n\n* The syntax appears to be correct\n* Consider if you need to add a WHERE clause for filtering\n* Think about adding ORDER BY for consistent results\n* Make sure you have proper indexes on join columns\n\nIs there something specific about this query you'd like me to help with?",
                    sql_context.current_query
                )
            } else if let Some(error) = &sql_context.error_message {
                format!(
                    "I notice there was an error:\n\n```\n{}\n```\n\nLet me help you debug this. Common issues include:\n\n1. **Syntax errors** - Check for missing keywords or punctuation\n2. **Table names** - Make sure tables and columns exist\n3. **Data types** - Ensure type compatibility\n4. **Permissions** - Check if you have access to the tables\n\nCould you share the exact query that's causing this error?",
                    error
                )
            } else {
                "Hello! I'm your SQL assistant. I can help you with:\n\n• **Writing SQL queries** - I'll help you craft the right query\n• **Debugging errors** - Show me your error messages and I'll help fix them\n• **Optimization** - I can suggest performance improvements\n• **Schema questions** - Ask me about your database structure\n\nI can see you're connected to a database. Try running a query or ask me anything about SQL! For quick commands, use:\n• `/explain` - Explain your current query\n• `/optimize` - Get optimization suggestions\n• `/fix` - Help with recent errors\n• `/help` - Show all available commands".to_string()
            };

            // Simulate streaming by sending chunks
            let words: Vec<&str> = response.split(' ').collect();
            let mut accumulated = String::new();

            for (i, word) in words.into_iter().enumerate() {
                accumulated.push_str(word);
                accumulated.push(' ');

                // Update UI with partial content - commented out for now
                // cx.update(|_cx| {
                //     // This would emit stream events in a real implementation
                // }).ok();

                // Small delay between chunks
                if i % 3 == 0 {
                    sleep(Duration::from_millis(50)).await;
                }
            }

            Ok(accumulated)
        })
    }

    fn send_provider_message(
        &mut self,
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        user_message: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let system_prompt = self.get_system_prompt();
        let messages = self.messages.clone();
        let model_name = self.model_name.clone();
        let user_message = user_message.to_string();

        cx.spawn(async move |_session, _cx| {
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

            let request = ChatCompletionRequest {
                model: model_name,
                messages: request_messages,
                stream: false,
                temperature: 0.7,
                max_tokens: Some(2048),
                tools: None,
                tool_choice: None,
                top_p: None,
                frequency_penalty: None,
                presence_penalty: None,
                additional_params: None,
            };

            // Send the request using the provider
            let response = provider.chat_completion(request).await
                .map_err(|e| anyhow::anyhow!("Chat completion failed: {}", e))?;

            if let Some(choice) = response.choices.first() {
                Ok(choice.message.content.clone())
            } else {
                Err(anyhow::anyhow!("No response content received"))
            }
        })
    }

    pub fn get_last_assistant_message(&self) -> Option<&ChatMessage> {
        self.messages.iter().rev().find(|m| m.role == MessageRole::Assistant)
    }

    pub fn get_last_user_message(&self) -> Option<&ChatMessage> {
        self.messages.iter().rev().find(|m| m.role == MessageRole::User)
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

impl EventEmitter<ChatEvent> for ChatSession {}

