//! Configuration management for PostgreSQL LSP
//!
//! This module handles configuration generation and management for the PostgreSQL language server,
//! including workspace configuration and connection-specific settings.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// PostgreSQL LSP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresLspConfig {
    pub connection: ConnectionConfig,
    pub database: DatabaseConfig,
    pub sql: SqlConfig,
    pub workspace: WorkspaceConfig,
}

/// Connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub connection_string: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub schema: Option<String>,
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub max_connections: u32,
    pub connection_timeout: u64,
    pub query_timeout: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            connection_timeout: 30,
            query_timeout: 60,
        }
    }
}

/// SQL configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlConfig {
    pub dialect: String,
    pub completion: CompletionConfig,
    pub diagnostics: DiagnosticsConfig,
    pub formatting: FormattingConfig,
}

impl Default for SqlConfig {
    fn default() -> Self {
        Self {
            dialect: "postgresql".to_string(),
            completion: CompletionConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            formatting: FormattingConfig::default(),
        }
    }
}

/// Completion configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionConfig {
    pub enabled: bool,
    pub trigger_characters: Vec<String>,
    pub auto_complete: bool,
    pub suggest_schemas: bool,
    pub suggest_tables: bool,
    pub suggest_columns: bool,
    pub suggest_functions: bool,
    pub suggest_keywords: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger_characters: vec![
                ".".to_string(),
                " ".to_string(),
                "(".to_string(),
                ",".to_string(),
            ],
            auto_complete: true,
            suggest_schemas: true,
            suggest_tables: true,
            suggest_columns: true,
            suggest_functions: true,
            suggest_keywords: true,
        }
    }
}

/// Diagnostics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsConfig {
    pub enabled: bool,
    pub syntax_checking: bool,
    pub semantic_checking: bool,
    pub schema_validation: bool,
    pub linting: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            syntax_checking: true,
            semantic_checking: true,
            schema_validation: true,
            linting: true,
        }
    }
}

/// Formatting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormattingConfig {
    pub enabled: bool,
    pub keyword_case: String, // "upper", "lower", "preserve"
    pub identifier_case: String,
    pub indent_size: usize,
    pub line_width: usize,
}

impl Default for FormattingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keyword_case: "upper".to_string(),
            identifier_case: "preserve".to_string(),
            indent_size: 2,
            line_width: 80,
        }
    }
}

/// Workspace configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    pub config_file: String,
    pub cache_enabled: bool,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            config_file: "postgrestools.jsonc".to_string(),
            cache_enabled: true,
        }
    }
}

/// Configuration errors
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to serialize configuration: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Failed to write configuration file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

impl Default for PostgresLspConfig {
    fn default() -> Self {
        Self {
            connection: ConnectionConfig {
                connection_string: "postgresql://localhost:5432/postgres".to_string(),
                host: "localhost".to_string(),
                port: 5432,
                database: "postgres".to_string(),
                username: "postgres".to_string(),
                schema: Some("public".to_string()),
            },
            database: DatabaseConfig::default(),
            sql: SqlConfig::default(),
            workspace: WorkspaceConfig::default(),
        }
    }
}

impl PostgresLspConfig {
    /// Create a new configuration with the given connection details
    pub fn new(
        connection_string: String,
        host: String,
        port: u16,
        database: String,
        username: String,
        schema: Option<String>,
    ) -> Self {
        Self {
            connection: ConnectionConfig {
                connection_string,
                host,
                port,
                database,
                username,
                schema,
            },
            database: DatabaseConfig::default(),
            sql: SqlConfig::default(),
            workspace: WorkspaceConfig::default(),
        }
    }

    /// Generate postgrestools.jsonc configuration
    pub fn generate_postgrestools_config(&self) -> Result<serde_json::Value, ConfigError> {
        let config = serde_json::json!({
            "database": {
                "connectionString": self.connection.connection_string,
                "database": self.connection.database,
                "schema": self.connection.schema.clone().unwrap_or_else(|| "public".to_string())
            },
            "sql": {
                "dialect": self.sql.dialect,
                "completion": {
                    "enableSchemas": self.sql.completion.suggest_schemas,
                    "enableTables": self.sql.completion.suggest_tables,
                    "enableColumns": self.sql.completion.suggest_columns,
                    "enableFunctions": self.sql.completion.suggest_functions,
                    "enableKeywords": self.sql.completion.suggest_keywords,
                    "triggerCharacters": self.sql.completion.trigger_characters
                },
                "diagnostics": {
                    "enableSyntax": self.sql.diagnostics.syntax_checking,
                    "enableSemantic": self.sql.diagnostics.semantic_checking,
                    "enableSchema": self.sql.diagnostics.schema_validation,
                    "enableLinting": self.sql.diagnostics.linting
                },
                "formatting": {
                    "enabled": self.sql.formatting.enabled,
                    "keywordCase": self.sql.formatting.keyword_case,
                    "identifierCase": self.sql.formatting.identifier_case,
                    "indentSize": self.sql.formatting.indent_size,
                    "lineWidth": self.sql.formatting.line_width
                }
            },
            "workspace": {
                "root": self.workspace.root,
                "cacheEnabled": self.workspace.cache_enabled
            }
        });

        Ok(config)
    }

    /// Create workspace configuration file
    pub fn create_workspace_config(&self, workspace_root: &Path) -> Result<(), ConfigError> {
        let config_path = workspace_root.join(&self.workspace.config_file);
        let config_content = self.generate_postgrestools_config()?;

        // Write with JSONC format (with comments)
        let jsonc_content = format!(
            r#"// PostgreSQL Language Server Configuration
// Generated automatically by Blanco

{}

// For more configuration options, see:
// https://github.com/supabase-community/postgres-language-server
"#,
            serde_json::to_string_pretty(&config_content)?
        );

        std::fs::write(config_path, jsonc_content)?;
        Ok(())
    }

    /// Update connection schema
    pub fn with_schema(mut self, schema: Option<String>) -> Self {
        self.connection.schema = schema;
        self
    }

    /// Update completion settings
    pub fn with_completion(mut self, completion: CompletionConfig) -> Self {
        self.sql.completion = completion;
        self
    }

    /// Update diagnostics settings
    pub fn with_diagnostics(mut self, diagnostics: DiagnosticsConfig) -> Self {
        self.sql.diagnostics = diagnostics;
        self
    }

    /// Update formatting settings
    pub fn with_formatting(mut self, formatting: FormattingConfig) -> Self {
        self.sql.formatting = formatting;
        self
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.connection.connection_string.is_empty() {
            return Err(ConfigError::Invalid(
                "Connection string cannot be empty".to_string(),
            ));
        }

        if self.connection.database.is_empty() {
            return Err(ConfigError::Invalid(
                "Database name cannot be empty".to_string(),
            ));
        }

        if self.connection.username.is_empty() {
            return Err(ConfigError::Invalid("Username cannot be empty".to_string()));
        }

        if self.connection.port == 0 {
            return Err(ConfigError::Invalid(
                "Port must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_creation() {
        let config = PostgresLspConfig::new(
            "postgresql://user@localhost:5432/testdb".to_string(),
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "user".to_string(),
            Some("public".to_string()),
        );

        assert_eq!(config.connection.host, "localhost");
        assert_eq!(config.connection.port, 5432);
        assert_eq!(config.connection.database, "testdb");
        assert_eq!(config.connection.username, "user");
        assert_eq!(config.connection.schema, Some("public".to_string()));
    }

    #[test]
    fn test_config_validation() {
        let mut config = PostgresLspConfig::new(
            "postgresql://user@localhost:5432/testdb".to_string(),
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "user".to_string(),
            None,
        );

        assert!(config.validate().is_ok());

        // Test invalid config
        config.connection.database = "".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_generation() {
        let config = PostgresLspConfig::new(
            "postgresql://user@localhost:5432/testdb".to_string(),
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "user".to_string(),
            Some("public".to_string()),
        );

        let json = config.generate_postgrestools_config();
        assert!(json.is_ok());

        let json_value = json.unwrap();
        assert_eq!(json_value["database"]["database"], "testdb");
        assert_eq!(json_value["database"]["schema"], "public");
        assert_eq!(json_value["sql"]["dialect"], "postgresql");
    }

    #[test]
    fn test_workspace_config_creation() {
        let config = PostgresLspConfig::new(
            "postgresql://user@localhost:5432/testdb".to_string(),
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "user".to_string(),
            None,
        );

        let temp_dir = TempDir::new().unwrap();
        let workspace_root = temp_dir.path().to_path_buf();

        let result = config.create_workspace_config(&workspace_root);
        assert!(result.is_ok());

        let config_path = workspace_root.join("postgrestools.jsonc");
        assert!(config_path.exists());

        let content = std::fs::read_to_string(config_path).unwrap();
        assert!(content.contains("PostgreSQL Language Server Configuration"));
        assert!(content.contains("testdb"));
    }

    #[test]
    fn test_default_configs() {
        let completion = CompletionConfig::default();
        assert!(completion.enabled);
        assert!(completion.suggest_tables);
        assert!(completion.suggest_columns);
        assert!(completion.trigger_characters.contains(&".".to_string()));

        let diagnostics = DiagnosticsConfig::default();
        assert!(diagnostics.enabled);
        assert!(diagnostics.syntax_checking);
        assert!(diagnostics.schema_validation);

        let formatting = FormattingConfig::default();
        assert!(formatting.enabled);
        assert_eq!(formatting.keyword_case, "upper");
        assert_eq!(formatting.indent_size, 2);
    }
}
