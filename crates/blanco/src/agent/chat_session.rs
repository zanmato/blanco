use anyhow::Result;
use gpui::{Context, EventEmitter, Task};
use http::{Method, Request};
use http_client::{AsyncBody, HttpClient};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use async_std::io::ReadExt;

use super::chat_types::{
    AIProvider, ChatCommand, ChatEvent, ChatMessage, MessageRole,
    OpenAIRequest, OpenAIResponse, SqlContext, ProviderType,
};

#[derive(Clone)]
#[allow(dead_code)]
pub struct ChatSession {
    pub messages: Vec<ChatMessage>,
    pub provider: AIProvider,
    pub http_client: Option<Arc<dyn HttpClient>>,
    #[allow(dead_code)]
    pub is_loading: bool,
    pub sql_context: SqlContext,
    #[allow(dead_code)]
    pub streaming_message_id: Option<String>,
}

#[allow(dead_code)]
impl ChatSession {
    pub fn new(provider: AIProvider, http_client: Option<Arc<dyn HttpClient>>) -> Self {
        Self {
            messages: Vec::new(),
            provider,
            http_client,
            is_loading: false,
            sql_context: SqlContext::empty(),
            streaming_message_id: None,
        }
    }

    pub fn with_provider(provider: AIProvider) -> Self {
        Self::new(provider, None)
    }

    pub fn with_openai(api_key: String, http_client: Arc<dyn HttpClient>) -> Self {
        Self::new(AIProvider::openai(api_key, None), Some(http_client))
    }

    pub fn with_mock() -> Self {
        Self::new(AIProvider::mock(), None)
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

        // Handle regular message based on provider type
        match &self.provider.provider_type {
            ProviderType::Mock => {
                self.send_mock_message(&user_message, cx)
            }
            ProviderType::OpenAI => {
                self.send_openai_message(&user_message, cx)
            }
            _ => {
                // Fallback to mock
                self.send_mock_message(&user_message, cx)
            }
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
                export.push_str(&format!("Provider: {} ({})\n\n", self.provider.name, self.provider.model));

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

        let message = ChatMessage::assistant(response.clone(), self.provider.model.clone());
        self.add_message(message.clone());
        cx.emit(ChatEvent::MessageAdded { message });

        Task::ready(Ok(response))
    }

    fn send_mock_message(
        &mut self,
        user_message: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let _http_client = self.http_client.clone();
        let _provider = self.provider.clone();
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

    fn send_openai_message(
        &mut self,
        user_message: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        if self.http_client.is_none() {
            return self.send_mock_message(user_message, cx);
        }

        let http_client = self.http_client.clone().unwrap();
        let provider = self.provider.clone();
        let system_prompt = self.get_system_prompt();

        // Build OpenAI request
        let mut openai_messages = vec![
            serde_json::json!({
                "role": "system",
                "content": system_prompt
            })
        ];

        // Add conversation history
        for message in &self.messages {
            let role = match message.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
            };

            openai_messages.push(serde_json::json!({
                "role": role,
                "content": message.content
            }));
        }

        let request = OpenAIRequest {
            model: provider.model.clone(),
            messages: openai_messages.into_iter()
                .map(|msg| serde_json::from_value(msg).unwrap())
                .collect(),
            stream: false,
            temperature: provider.temperature,
            max_tokens: Some(provider.max_tokens),
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
        };

        let api_key = provider.api_key.clone().unwrap_or_default();
        let base_url = provider.base_url.clone().unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        cx.spawn(async move |_, _cx| {
            // Create HTTP request
            let request_body = serde_json::to_string(&request)?;

            let http_request = Request::builder()
                .method(Method::POST)
                .uri(format!("{}/chat/completions", base_url))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", api_key))
                .body(AsyncBody::from(request_body))?;

            // Send request
            let response = http_client.send(http_request).await?;

            if response.status().is_success() {
                let mut body_bytes = Vec::new();
                response.into_body().read_to_end(&mut body_bytes).await?;
                let body = String::from_utf8(body_bytes)?;
                let openai_response: OpenAIResponse = serde_json::from_str(&body)?;

                if let Some(choice) = openai_response.choices.first() {
                    Ok(choice.message.content.clone())
                } else {
                    Err(anyhow::anyhow!("No response content received"))
                }
            } else {
                let status = response.status();
                let mut body_bytes = Vec::new();
                response.into_body().read_to_end(&mut body_bytes).await?;
                let body = String::from_utf8(body_bytes)?;
                Err(anyhow::anyhow!("API request failed: {} - {}", status, body))
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

