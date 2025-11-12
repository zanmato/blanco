// Application Events Module - GPUI Native Implementation
//
// This module defines all application events using GPUI's native EventEmitter system.
// Events are emitted using cx.emit() and subscribed to using cx.subscribe().

use std::time::Duration;

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
    ConnectionHealthCheck {
        connection_id: Option<i64>,
        is_healthy: bool,
    },
    ConnectionsLoaded {
        count: usize,
    },

    /// UI events
    ToggleSidebar,

    /// Query execution events
    QueryExecutionStarted {
        connection_id: Option<i64>,
        query: String,
    },
    QueryExecutionCompleted {
        connection_id: Option<i64>,
        success: bool,
        execution_time: Duration,
        rows_affected: Option<u64>,
        error_message: Option<String>,
    },

    /// Schema events
    SchemaChanged {
        connection_id: Option<i64>,
        schema_name: String,
    },
    TablesRefreshed {
        connection_id: Option<i64>,
        table_count: usize,
    },

    /// Table operations events
    TableChangesCommitted {
        table_name: String,
        connection_id: Option<i64>,
        changes_count: usize,
    },
    TableChangesRollback {
        table_name: String,
        connection_id: Option<i64>,
        changes_count: usize,
    },
    TableOperationCompleted {
        table_name: String,
        connection_id: Option<i64>,
        success: bool,
        rows_affected: Option<u64>,
        error_message: Option<String>,
        operations_executed: usize,
    },

    /// UI events
    ThemeChanged(gpui_component::ThemeMode),
    SidebarToggled {
        collapsed: bool,
    },
    TabChanged {
        tab_id: usize,
    },
    RenameTabRequested {
        tab_index: usize,
        new_name: String,
    },
    CreateNewQueryTab {
        connection_id: i64,
    },

    /// Database, Schema and Table events
    ConnectionSelected {
        connection_id: Option<i64>,
    },
    DatabasesLoaded {
        connection_id: Option<i64>,
        databases: Vec<String>,
    },
    DatabaseSelected {
        connection_id: Option<i64>,
        database_name: String,
    },
    DatabaseExpanded {
        connection_id: Option<i64>,
        database_name: String,
    },
    SchemasLoaded {
        connection_id: Option<i64>,
        database_name: Option<String>,
        schemas: Vec<String>,
    },
    TablesLoaded {
        connection_id: Option<i64>,
        database_name: Option<String>,
        schema: Option<String>,
        tables: Vec<String>,
    },
    SchemaSelected {
        connection_id: Option<i64>,
        database_name: String,
        schema_name: String,
    },
    SchemaExpanded {
        connection_id: Option<i64>,
        database_name: String,
        schema_name: String,
    },
    TableSelected {
        connection_id: Option<i64>,
        database_name: String,
        schema_name: Option<String>,
        table_name: String,
    },

    /// Error events
    ErrorOccurred {
        context: String,
        error: String,
        severity: ErrorSeverity,
    },

    /// File operation events
    FileSaved {
        file_path: String,
        success: bool,
    },
    FileOpened {
        file_path: String,
    },

    /// Performance monitoring events
    PerformanceMetrics {
        connection_id: Option<i64>,
        query_time: Duration,
        connection_pool_size: usize,
    },

    /// Chat events
    ChatMessageSent {
        tab_id: usize,
        message_content: String,
    },
    ChatMessageReceived {
        tab_id: usize,
        message_content: String,
        role: String, // "user" | "assistant" | "system"
    },
    ChatSessionStarted {
        tab_id: usize,
        provider: String,
        model: String,
    },
    ChatSessionEnded {
        tab_id: usize,
    },
    ChatToggled {
        tab_id: usize,
        enabled: bool,
    },
    ChatError {
        tab_id: usize,
        error_message: String,
    },
}

/// Error severity levels
#[derive(Clone, Debug, PartialEq)]
pub enum ErrorSeverity {
    Info,
    Warning,
    Error,
    Critical,
}
