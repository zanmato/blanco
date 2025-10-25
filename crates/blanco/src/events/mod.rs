// Event System Module - Custom Event Emitter Pattern
//
// This module provides a decentralized event handling system that allows components
// to emit and subscribe to events without requiring all events to flow through
// the root BlancoApp entity.

use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use std::collections::HashMap;
use std::time::Duration;
use anyhow::Result;

/// Maximum number of events to keep in memory for each subscriber
const EVENT_BUFFER_SIZE: usize = 1000;

/// Core application events that can be emitted and subscribed to
#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    /// Connection events
    ConnectionEstablished {
        connection_id: String,
        connection_string: String,
        display_name: String,
    },
    ConnectionLost {
        connection_id: String,
        error: Option<String>,
    },
    ConnectionHealthCheck {
        connection_id: String,
        is_healthy: bool,
    },

    /// Query execution events
    QueryExecuted {
        connection_id: String,
        query: String,
        duration: Duration,
        rows_affected: Option<u64>,
        success: bool,
    },
    QueryExecutionStarted {
        connection_id: String,
        query: String,
    },
    QueryExecutionCompleted {
        connection_id: String,
        query: String,
        result: Option<crate::connection_trait::QueryResult>,
    },

    /// Database schema events
    SchemaChanged {
        connection_id: String,
        schema_name: Option<String>,
    },
    TablesRefreshed {
        connection_id: String,
        table_count: usize,
    },

    /// Table operation events
    TableChangesCommitted {
        table_name: String,
        connection_id: String,
        changes_count: usize,
        rows_affected: u64,
        success: bool,
    },
    TableChangesRolledBack {
        table_name: String,
        connection_id: String,
        changes_count: usize,
    },

    /// UI events
    ThemeChanged(gpui_component::ThemeMode),
    SidebarToggled { collapsed: bool },
    TabChanged { tab_id: usize },

    /// LSP events
    LspConnectionEstablished {
        connection_id: String,
        language: String,
    },
    LspDiagnosticsReceived {
        connection_id: String,
        diagnostics: Vec<gpui_component::highlighter::Diagnostic>,
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

/// Error severity levels for error events
#[derive(Clone, Debug, PartialEq)]
pub enum ErrorSeverity {
    #[allow(dead_code)]
    Info,
    #[allow(dead_code)]
    Warning,
    Error,
    #[allow(dead_code)]
    Critical,
}

/// Event subscription that can be used to cancel event listening
#[allow(dead_code)]
pub struct EventSubscription {
    pub event_type: String,
    pub subscriber_id: String,
    // In a real implementation, this would hold a cancellation handle
}

/// Generic event emitter trait that components can implement
#[allow(dead_code)]
pub trait EventEmitter<T> {
    /// Emit an event to all subscribers
    fn emit(&self, event: T) -> Result<()>;

    /// Subscribe to events of this type
    fn subscribe(&self) -> broadcast::Receiver<T>;

    /// Subscribe with a filter function
    fn subscribe_filtered<F>(&self, filter: F) -> broadcast::Receiver<T>
    where
        F: Fn(&T) -> bool + Send + 'static;
}

/// Central event bus that coordinates all event emission and subscription
pub struct EventBus {
    /// Broadcast channels for different event types
    event_senders: Arc<RwLock<HashMap<String, broadcast::Sender<AppEvent>>>>,

    /// Event routing for typed events
    _task_handles: Vec<tokio::task::JoinHandle<()>>,

    /// Configuration
    buffer_size: usize,
}

impl EventBus {
    /// Create a new event bus
    pub fn new() -> Self {
        Self {
            event_senders: Arc::new(RwLock::new(HashMap::new())),
            _task_handles: Vec::new(),
            buffer_size: EVENT_BUFFER_SIZE,
        }
    }

    /// Get or create a broadcast channel for an event type
    async fn get_or_create_sender(&self, event_type: &str) -> broadcast::Sender<AppEvent> {
        let mut senders = self.event_senders.write().await;

        if let Some(sender) = senders.get(event_type) {
            sender.clone()
        } else {
            let (sender, _) = broadcast::channel(self.buffer_size);
            senders.insert(event_type.to_string(), sender.clone());
            sender
        }
    }

    /// Emit an event to all subscribers
    pub async fn emit(&self, event: AppEvent) -> Result<()> {
        let event_type = self.get_event_type(&event);
        let sender = self.get_or_create_sender(&event_type).await;

        match sender.send(event) {
            Ok(count) => {
                log::debug!("Event '{}' emitted to {} subscribers", event_type, count);
                Ok(())
            }
            Err(broadcast::error::SendError(_)) => {
                // This happens when there are no active subscribers
                log::debug!("Event '{}' emitted but no active subscribers", event_type);
                Ok(())
            }
        }
    }

    /// Subscribe to events of a specific type
    #[allow(dead_code)]
    pub async fn subscribe(&self, event_type: &str) -> Result<broadcast::Receiver<AppEvent>> {
        let sender = self.get_or_create_sender(event_type).await;
        Ok(sender.subscribe())
    }

    /// Subscribe to all events
    #[allow(dead_code)]
    pub async fn subscribe_all(&self) -> Result<broadcast::Receiver<AppEvent>> {
        self.subscribe("*").await
    }

    /// Get the event type string for an AppEvent
    fn get_event_type(&self, event: &AppEvent) -> String {
        match event {
            AppEvent::ConnectionEstablished { .. } => "connection.established".to_string(),
            AppEvent::ConnectionLost { .. } => "connection.lost".to_string(),
            AppEvent::ConnectionHealthCheck { .. } => "connection.health_check".to_string(),
            AppEvent::QueryExecuted { .. } => "query.executed".to_string(),
            AppEvent::QueryExecutionStarted { .. } => "query.started".to_string(),
            AppEvent::QueryExecutionCompleted { .. } => "query.completed".to_string(),
            AppEvent::SchemaChanged { .. } => "schema.changed".to_string(),
            AppEvent::TablesRefreshed { .. } => "schema.tables_refreshed".to_string(),
            AppEvent::TableChangesCommitted { .. } => "table.changes_committed".to_string(),
            AppEvent::TableChangesRolledBack { .. } => "table.changes_rolled_back".to_string(),
            AppEvent::ThemeChanged { .. } => "ui.theme_changed".to_string(),
            AppEvent::SidebarToggled { .. } => "ui.sidebar_toggled".to_string(),
            AppEvent::TabChanged { .. } => "ui.tab_changed".to_string(),
            AppEvent::LspConnectionEstablished { .. } => "lsp.connection_established".to_string(),
            AppEvent::LspDiagnosticsReceived { .. } => "lsp.diagnostics_received".to_string(),
            AppEvent::ErrorOccurred { .. } => "error.occurred".to_string(),
            AppEvent::FileSaved { .. } => "file.saved".to_string(),
            AppEvent::FileOpened { .. } => "file.opened".to_string(),
            AppEvent::PerformanceMetrics { .. } => "performance.metrics".to_string(),
        }
    }

    /// Get statistics about the event bus
    #[allow(dead_code)]
    pub async fn get_stats(&self) -> EventBusStats {
        let senders = self.event_senders.read().await;
        let mut channel_counts = HashMap::new();

        for (event_type, sender) in senders.iter() {
            channel_counts.insert(event_type.clone(), sender.len());
        }

        EventBusStats {
            total_channels: senders.len(),
            channel_counts,
            buffer_size: self.buffer_size,
        }
    }
}

/// Statistics about the event bus
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EventBusStats {
    pub total_channels: usize,
    pub channel_counts: HashMap<String, usize>,
    pub buffer_size: usize,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Global event bus instance
static mut GLOBAL_EVENT_BUS: Option<Arc<EventBus>> = None;
static INIT: std::sync::Once = std::sync::Once::new();

/// Get the global event bus instance
#[allow(static_mut_refs)]
pub fn global_event_bus() -> Arc<EventBus> {
    unsafe {
        INIT.call_once(|| {
            GLOBAL_EVENT_BUS = Some(Arc::new(EventBus::new()));
        });
        GLOBAL_EVENT_BUS.as_ref().unwrap().clone()
    }
}

/// Macro for easy event emission
#[macro_export]
macro_rules! emit_event {
    ($event:expr) => {
        {
            let event_clone = $event.clone();
            tokio::spawn(async move {
                if let Err(e) = $crate::events::global_event_bus().emit(event_clone).await {
                    log::error!("Failed to emit event: {}", e);
                }
            });
        }
    };
}

/// Macro for easy event subscription
#[macro_export]
macro_rules! subscribe_to_events {
    ($event_type:expr) => {{
        $crate::events::global_event_bus().subscribe($event_type).await
    }};
}

// Re-export commonly used items
pub use emit_event;