use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: MessageMetadata,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub tokens_used: Option<u32>,
    pub model: String,
    pub sql_context: Option<SqlContext>,
    pub execution_time: Option<std::time::Duration>,
}

impl Default for MessageMetadata {
    fn default() -> Self {
        Self {
            tokens_used: None,
            model: "unknown".to_string(),
            sql_context: None,
            execution_time: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SqlContext {
    pub current_query: String,
    pub connection_string: Option<String>,
    pub database_type: Option<String>,
    pub recent_results: Option<QueryResults>,
    pub error_message: Option<String>,
    pub schema_info: Option<SchemaInfo>,
    pub tables: Vec<TableInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryResults {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub execution_time: std::time::Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub database_name: String,
    pub schema_version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub schema: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub row_count: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

// OpenAI API types
#[derive(Clone, Debug, Serialize)]
pub struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    pub stream: bool,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIResponse {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub object: String,
    #[allow(dead_code)]
    pub created: u64,
    #[allow(dead_code)]
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    #[allow(dead_code)]
    pub usage: OpenAIUsage,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIChoice {
    #[allow(dead_code)]
    pub index: u32,
    pub message: OpenAIResponseMessage,
    #[allow(dead_code)]
    pub finish_reason: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIResponseMessage {
    #[allow(dead_code)]
    pub role: String,
    pub content: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIUsage {
    #[allow(dead_code)]
    pub prompt_tokens: u32,
    #[allow(dead_code)]
    pub completion_tokens: u32,
    #[allow(dead_code)]
    pub total_tokens: u32,
}

// Streaming response types
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamResponse {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub object: String,
    #[allow(dead_code)]
    pub created: u64,
    #[allow(dead_code)]
    pub model: String,
    #[allow(dead_code)]
    pub choices: Vec<OpenAIStreamChoice>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamChoice {
    #[allow(dead_code)]
    pub index: u32,
    #[allow(dead_code)]
    pub delta: OpenAIStreamDelta,
    #[allow(dead_code)]
    pub finish_reason: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamDelta {
    #[allow(dead_code)]
    pub role: Option<String>,
    #[allow(dead_code)]
    pub content: Option<String>,
}

// Chat session events
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum ChatEvent {
    MessageAdded {
        message: ChatMessage,
    },
    #[allow(dead_code)]
    MessageUpdated {
        message_id: String,
        content: String,
    },
    #[allow(dead_code)]
    StreamStarted {
        message_id: String,
    },
    #[allow(dead_code)]
    StreamUpdate {
        message_id: String,
        content: String,
    },
    #[allow(dead_code)]
    StreamCompleted {
        message_id: String,
        final_content: String,
    },
    #[allow(dead_code)]
    Error {
        message: String,
    },
    #[allow(dead_code)]
    SessionStarted {
        provider: String,
        model: String,
    },
    SessionCleared,
}

// AI Provider types
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum ProviderType {
    OpenAI,
    Anthropic,
    LocalLLM,
    Mock,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct AIProvider {
    pub provider_type: ProviderType,
    pub name: String,
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[allow(dead_code)]
impl AIProvider {
    pub fn openai(api_key: String, model: Option<String>) -> Self {
        Self {
            provider_type: ProviderType::OpenAI,
            name: "OpenAI".to_string(),
            model: model.unwrap_or_else(|| "gpt-4".to_string()),
            api_key: Some(api_key),
            base_url: Some("https://api.openai.com/v1".to_string()),
            max_tokens: 2048,
            temperature: 0.7,
        }
    }

    pub fn mock() -> Self {
        Self {
            provider_type: ProviderType::Mock,
            name: "Mock".to_string(),
            model: "mock-gpt-4".to_string(),
            api_key: None,
            base_url: None,
            max_tokens: 2048,
            temperature: 0.7,
        }
    }
}

// Chat commands
#[derive(Clone, Debug, PartialEq)]
pub enum ChatCommand {
    Explain,
    Optimize,
    Fix,
    Schema(Option<String>),
    Export,
    Clear,
    Help,
}

impl ChatCommand {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if !text.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = text.splitn(2, ' ').collect();
        let command = parts[0].to_lowercase();

        match command.as_str() {
            "/explain" => Some(Self::Explain),
            "/optimize" => Some(Self::Optimize),
            "/fix" => Some(Self::Fix),
            "/schema" => {
                let table_name = parts.get(1).map(|s| s.to_string());
                Some(Self::Schema(table_name))
            }
            "/export" => Some(Self::Export),
            "/clear" => Some(Self::Clear),
            "/help" => Some(Self::Help),
            _ => None,
        }
    }

    pub fn help_text() -> &'static str {
        r#"
Available commands:
/explain - Explain the current SQL query
/optimize - Suggest optimizations for the current query
/fix - Help fix errors in the current query
/schema [table] - Show schema information for entire database or specific table
/export - Export the current conversation to a file
/clear - Clear the current chat history
/help - Show this help message
"#
    }
}

// Utility functions
impl ChatMessage {
    pub fn new(role: MessageRole, content: String, model: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role,
            content,
            timestamp: Utc::now(),
            metadata: MessageMetadata {
                tokens_used: None,
                model,
                sql_context: None,
                execution_time: None,
            },
        }
    }

    #[allow(dead_code)]
    pub fn with_sql_context(mut self, sql_context: SqlContext) -> Self {
        self.metadata.sql_context = Some(sql_context);
        self
    }

    pub fn user(content: String) -> Self {
        Self::new(MessageRole::User, content, "user".to_string())
    }

    pub fn assistant(content: String, model: String) -> Self {
        Self::new(MessageRole::Assistant, content, model)
    }

    #[allow(dead_code)]
    pub fn system(content: String) -> Self {
        Self::new(MessageRole::System, content, "system".to_string())
    }
}

impl SqlContext {
    pub fn empty() -> Self {
        Self {
            current_query: String::new(),
            connection_string: None,
            database_type: None,
            recent_results: None,
            error_message: None,
            schema_info: None,
            tables: Vec::new(),
        }
    }

    pub fn with_query(query: String) -> Self {
        Self {
            current_query: query,
            ..Self::empty()
        }
    }

    #[allow(dead_code)]
    pub fn update_query(&mut self, query: String) {
        self.current_query = query;
    }

    #[allow(dead_code)]
    pub fn set_error(&mut self, error: String) {
        self.error_message = Some(error);
    }

    #[allow(dead_code)]
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }

    #[allow(dead_code)]
    pub fn set_results(&mut self, results: QueryResults) {
        self.recent_results = Some(results);
    }

    pub fn to_system_prompt(&self) -> String {
        let mut prompt = "You are a helpful SQL assistant integrated into the Blanco SQL Editor. ".to_string();

        if let Some(database_type) = &self.database_type {
            prompt.push_str(&format!("The current database type is {}. ", database_type));
        }

        if !self.current_query.is_empty() {
            prompt.push_str(&format!("Current SQL query:\n```\n{}\n```\n\n", self.current_query));
        }

        if let Some(error) = &self.error_message {
            prompt.push_str(&format!("Recent error: {}\n\n", error));
        }

        if let Some(results) = &self.recent_results {
            prompt.push_str(&format!(
                "Recent query returned {} rows in {:?}.\n\n",
                results.row_count,
                results.execution_time
            ));
        }

        if !self.tables.is_empty() {
            prompt.push_str("Available tables:\n");
            for table in &self.tables {
                prompt.push_str(&format!("- {}.{} ({} columns)\n",
                    table.schema.as_deref().unwrap_or("public"),
                    table.name,
                    table.columns.len()
                ));
            }
            prompt.push('\n');
        }

        prompt.push_str("Provide helpful SQL assistance, explain queries, suggest optimizations, and help debug issues. Be concise but thorough.");

        prompt
    }
}