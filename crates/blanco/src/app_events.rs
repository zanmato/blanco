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
        connection_id: String,
        connection_type: String,
        database_name: Option<String>,
    },
    ConnectionLost {
        connection_id: String,
        error: String,
    },
    ConnectionHealthCheck {
        connection_id: String,
        is_healthy: bool,
    },
    ConnectionsLoaded {
        count: usize,
    },

    /// Query execution events
    QueryExecutionStarted {
        connection_id: String,
        query: String,
    },
    QueryExecutionCompleted {
        connection_id: String,
        success: bool,
        execution_time: Duration,
        rows_affected: Option<u64>,
        error_message: Option<String>,
    },

    /// Schema events
    SchemaChanged {
        connection_id: String,
        schema_name: String,
    },
    TablesRefreshed {
        connection_id: String,
        table_count: usize,
    },

    /// Table operations events
    TableChangesCommitted {
        table_name: String,
        connection_id: String,
        changes_count: usize,
    },
    TableChangesRollback {
        table_name: String,
        connection_id: String,
        changes_count: usize,
    },
    TableOperationCompleted {
        table_name: String,
        connection_id: String,
        success: bool,
        rows_affected: Option<u64>,
        error_message: Option<String>,
        operations_executed: usize,
    },

    /// UI events
    ThemeChanged(gpui_component::ThemeMode),
    SidebarToggled { collapsed: bool },
    TabChanged { tab_id: usize },

    /// SQL completion events
    CompletionTriggered {
        tab_id: usize,
        position: crate::sql_completion::Position,
        trigger_character: Option<char>,
    },
    CompletionSelected {
        tab_id: usize,
        item: crate::sql_completion::CompletionItem,
        position: crate::sql_completion::Position,
    },
    CompletionCancelled {
        tab_id: usize,
    },
    HoverRequested {
        tab_id: usize,
        position: crate::sql_completion::Position,
        word: String,
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
        connection_id: String,
        query_time: Duration,
        connection_pool_size: usize,
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