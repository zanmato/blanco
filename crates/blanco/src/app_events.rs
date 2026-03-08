use std::time::Duration;

use crate::app_database::ConnectionData;

/// Core application events that can be emitted and subscribed to
#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    /// Connection events
    ConnectionEstablished {
        connection_id: Option<i64>,
        connection_type: String,
        database_name: Option<String>,
    },
    ConnectionLost {
        connection_id: Option<i64>,
        error: String,
    },
    EditConnection {
        connection_id: i64,
        connection_data: Box<ConnectionData>,
    },

    /// UI events
    ToggleSidebar,

    /// Query execution events
    QueryExecutionCompleted {
        connection_id: Option<i64>,
        database_name: Option<String>,
        success: bool,
        execution_time: Duration,
        rows_affected: Option<u64>,
        error_message: Option<String>,
    },
    TableChangesRollback {
        table_name: String,
        connection_id: i64,
        changes_count: usize,
    },
    TableOperationCompleted {
        table_name: String,
        connection_id: i64,
        success: bool,
        rows_affected: Option<u64>,
        error_message: Option<String>,
        operations_executed: usize,
    },
    CreateNewQueryTab {
        connection_id: i64,
        connection_name: String,
        db_type: database::DatabaseType,
        database_name: String,
        schema_name: Option<String>,
        table_name: Option<String>,
        environment_type: Option<crate::app_database::EnvironmentType>,
    },
    OpenTableStructure {
        connection_id: i64,
        connection_name: String,
        db_type: database::DatabaseType,
        database_name: String,
        schema_name: Option<String>,
        table_name: String,
        environment_type: Option<crate::app_database::EnvironmentType>,
    },
    EditorSettingChanged {
        setting: String,
        value: String,
    },

    /// Database, Schema and Table events
    SchemasLoaded {
        connection_id: Option<i64>,
        database_name: Option<String>,
        schemas: Vec<String>,
    },

    /// Error events
    ErrorOccurred {
        context: String,
        error: String,
        severity: ErrorSeverity,
    },

    /// Tree component events
    TreeItemExpanded {
        item_id: String,
        item_type: TreeItemType,
        connection_id: Option<i64>,
    },
    TreeItemSelected {
        item_id: String,
        item_type: TreeItemType,
        connection_id: Option<i64>,
        database_name: Option<String>,
        schema_name: Option<String>,
        table_name: Option<String>,
    },

    /// Chat events
    ChatSessionStarted {
        provider: String,
        model: String,
    },
    ChatSessionEnded,
    ChatToggled {
        enabled: bool,
    },

    /// Snippet events
    SnippetDeleted {
        id: i64,
    },
    OpenSnippetEditor {
        snippet_id: Option<i64>,
    },
}

/// Error severity levels
#[derive(Clone, Debug, PartialEq)]
pub enum ErrorSeverity {
    Error,
}

/// Tree item types for different hierarchical levels
#[derive(Clone, Debug, PartialEq)]
pub enum TreeItemType {
    Connection,
    Database,
    Schema,
    Table,
}
