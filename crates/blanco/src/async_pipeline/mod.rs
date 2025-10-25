// Async Event Processing Pipeline
//
// This module provides a centralized async processing pipeline that handles
// database operations, LSP communications, and other async tasks without
// blocking the UI thread.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, RwLock};
use std::collections::{HashMap, VecDeque};
use anyhow::{Result, anyhow};
use log::{debug, info, error};
use gpui::Global;

use crate::events::AppEvent;
use crate::connection_trait::QueryResult;
use crate::table_operations::TableChangeOperation;
use crate::db_service::DbService;

/// Maximum number of async events to queue
const ASYNC_QUEUE_SIZE: usize = 1000;

/// Task priority levels for async operations
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Async events that can be processed by the pipeline
#[derive(Debug)]
#[allow(dead_code)]
pub enum AsyncEvent {
    /// Database query execution
    ExecuteQuery {
        connection_string: String,
        sql: String,
        response_tx: oneshot::Sender<Result<QueryResult>>,
        priority: TaskPriority,
    },

    /// Database table operations (INSERT, UPDATE, DELETE)
    ExecuteTableOperations {
        connection_string: String,
        operations: Vec<TableChangeOperation>,
        response_tx: oneshot::Sender<Result<TableOperationResult>>,
        priority: TaskPriority,
    },

    /// Connection health check
    CheckConnectionHealth {
        connection_string: String,
        response_tx: oneshot::Sender<Result<ConnectionHealth>>,
        priority: TaskPriority,
    },

    /// Schema information refresh
    RefreshSchema {
        connection_string: String,
        schema_name: Option<String>,
        response_tx: oneshot::Sender<Result<Vec<SchemaInfo>>>,
        priority: TaskPriority,
    },

    /// LSP diagnostics request
    RequestLspDiagnostics {
        connection_string: String,
        document_uri: String,
        response_tx: oneshot::Sender<Result<Vec<gpui_component::highlighter::Diagnostic>>>,
        priority: TaskPriority,
    },

    /// Performance metrics collection
    CollectPerformanceMetrics {
        connection_string: String,
        response_tx: oneshot::Sender<Result<PerformanceMetrics>>,
        priority: TaskPriority,
    },

    }

/// Result from table operations
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TableOperationResult {
    pub rows_affected: u64,
    pub operations_executed: usize,
    pub execution_time: Duration,
    pub errors: Vec<String>,
}

/// Connection health information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConnectionHealth {
    pub is_healthy: bool,
    pub last_check: Instant,
    pub response_time: Option<Duration>,
    pub error_message: Option<String>,
}

/// Schema information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SchemaInfo {
    pub name: String,
    pub table_count: usize,
    pub tables: Vec<TableInfo>,
}

/// Table information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TableInfo {
    pub name: String,
    pub schema: Option<String>,
    pub row_count_estimate: Option<u64>,
    pub columns: Vec<ColumnInfo>,
}

/// Column information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
}

/// Performance metrics
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PerformanceMetrics {
    pub connection_string: String,
    pub query_count: u64,
    pub avg_query_time: Duration,
    pub connection_pool_size: usize,
    pub active_connections: usize,
    pub last_activity: Instant,
}

/// Async event processor that handles queued async operations
pub struct AsyncEventProcessor {
    /// Event receiver
    event_rx: mpsc::Receiver<AsyncEvent>,

    /// Priority queues for different task priorities
    task_queues: Arc<RwLock<HashMap<TaskPriority, VecDeque<AsyncEvent>>>>,

    /// Database service handle
    db_service: DbService,

    /// Event bus for emitting results
    event_bus: Arc<crate::events::EventBus>,

    /// Performance tracking
    performance_metrics: Arc<RwLock<HashMap<String, PerformanceMetrics>>>,

    /// Debounced queries cache (simplified for future use)
    debounced_queries: Arc<RwLock<HashMap<String, Instant>>>,

    /// Processor state
    is_running: Arc<RwLock<bool>>,
}

impl AsyncEventProcessor {
    /// Create a new async event processor
    pub fn new(db_service: DbService, event_bus: Arc<crate::events::EventBus>) -> (Self, AsyncEventSender) {
        let (event_tx, event_rx) = mpsc::channel(ASYNC_QUEUE_SIZE);
        let wrapped_sender = AsyncEventSender::new(event_tx);

        let processor = Self {
            event_rx,
            task_queues: Arc::new(RwLock::new(HashMap::new())),
            db_service,
            event_bus,
            performance_metrics: Arc::new(RwLock::new(HashMap::new())),
            debounced_queries: Arc::new(RwLock::new(HashMap::new())),
            is_running: Arc::new(RwLock::new(false)),
        };

        (processor, wrapped_sender)
    }

    /// Start the async event processor
    pub async fn start(&mut self) -> Result<()> {
        {
            let mut is_running = self.is_running.write().await;
            if *is_running {
                return Ok(()); // Already running
            }
            *is_running = true;
        }

        info!("Starting async event processor");

        // Initialize task queues
        {
            let mut queues = self.task_queues.write().await;
            queues.insert(TaskPriority::Critical, VecDeque::new());
            queues.insert(TaskPriority::High, VecDeque::new());
            queues.insert(TaskPriority::Normal, VecDeque::new());
            queues.insert(TaskPriority::Low, VecDeque::new());
        }

        // Start the main processing loop
        let event_rx = std::mem::replace(&mut self.event_rx, mpsc::channel(1).1);
        let task_queues = self.task_queues.clone();
        let db_service = self.db_service.clone();
        let event_bus = self.event_bus.clone();
        let performance_metrics = self.performance_metrics.clone();
        let debounced_queries = self.debounced_queries.clone();
        let is_running = self.is_running.clone();

        tokio::spawn(async move {
            let mut event_rx = event_rx;
            
            loop {
                {
                    let running = is_running.read().await;
                    if !*running {
                        info!("Async event processor stopping");
                        break;
                    }
                }

                tokio::select! {
                    // Handle incoming events
                    event = event_rx.recv() => {
                        match event {
                            Some(async_event) => {
                                if let Err(e) = Self::handle_incoming_event(
                                    async_event,
                                    &task_queues,
                                    &debounced_queries
                                ).await {
                                    error!("Failed to handle incoming event: {}", e);
                                }
                            }
                            None => {
                                info!("Event channel closed, stopping processor");
                                break;
                            }
                        }
                    }
                }

                // Process queued tasks by priority
                if let Err(e) = Self::process_task_queue(
                    &task_queues,
                    &db_service,
                    &event_bus,
                    &performance_metrics
                ).await {
                    error!("Failed to process task queue: {}", e);
                }
            }
        });

        Ok(())
    }

    /// Stop the async event processor
    #[allow(dead_code)]
    pub async fn stop(&self) -> Result<()> {
        let mut is_running = self.is_running.write().await;
        *is_running = false;
        info!("Async event processor stop requested");
        Ok(())
    }

    /// Handle incoming events and route them to appropriate queues
    async fn handle_incoming_event(
        event: AsyncEvent,
        task_queues: &Arc<RwLock<HashMap<TaskPriority, VecDeque<AsyncEvent>>>>,
        _debounced_queries: &Arc<RwLock<HashMap<String, Instant>>>,
    ) -> Result<()> {
        // Regular events - queue by priority
        Self::queue_event_by_priority(event, task_queues).await?;

        Ok(())
    }

    
    /// Queue an event by its priority
    async fn queue_event_by_priority(
        event: AsyncEvent,
        task_queues: &Arc<RwLock<HashMap<TaskPriority, VecDeque<AsyncEvent>>>>,
    ) -> Result<()> {
        let priority = Self::get_event_priority(&event);
        let mut queues = task_queues.write().await;

        if let Some(queue) = queues.get_mut(&priority) {
            queue.push_back(event);
            debug!("Queued event with priority {:?}", priority);
        } else {
            return Err(anyhow!("No queue found for priority {:?}", priority));
        }

        Ok(())
    }

    /// Get the priority of an event
    fn get_event_priority(event: &AsyncEvent) -> TaskPriority {
        match event {
            AsyncEvent::ExecuteQuery { priority, .. } => priority.clone(),
            AsyncEvent::ExecuteTableOperations { priority, .. } => priority.clone(),
            AsyncEvent::CheckConnectionHealth { priority, .. } => priority.clone(),
            AsyncEvent::RefreshSchema { priority, .. } => priority.clone(),
            AsyncEvent::RequestLspDiagnostics { priority, .. } => priority.clone(),
            AsyncEvent::CollectPerformanceMetrics { priority, .. } => priority.clone(),
        }
    }

    /// Process the task queue, handling events by priority
    async fn process_task_queue(
        task_queues: &Arc<RwLock<HashMap<TaskPriority, VecDeque<AsyncEvent>>>>,
        db_service: &DbService,
        event_bus: &Arc<crate::events::EventBus>,
        performance_metrics: &Arc<RwLock<HashMap<String, PerformanceMetrics>>>,
    ) -> Result<()> {
        let priorities = [
            TaskPriority::Critical,
            TaskPriority::High,
            TaskPriority::Normal,
            TaskPriority::Low,
        ];

        for priority in priorities {
            let mut queues = task_queues.write().await;
            if let Some(queue) = queues.get_mut(&priority) {
                if let Some(event) = queue.pop_front() {
                    // Release the lock before processing
                    drop(queues);

                    // Process the event
                    let result = Self::process_event(event, db_service, event_bus, performance_metrics).await;
                    if let Err(e) = result {
                        error!("Failed to process event: {}", e);
                    }

                    // Continue to next priority level
                    continue;
                }
            }
        }

        Ok(())
    }

    /// Process a single async event
    async fn process_event(
        event: AsyncEvent,
        db_service: &DbService,
        event_bus: &Arc<crate::events::EventBus>,
        performance_metrics: &Arc<RwLock<HashMap<String, PerformanceMetrics>>>,
    ) -> Result<()> {
        let start_time = Instant::now();

        match event {
            AsyncEvent::ExecuteQuery { connection_string, sql, response_tx, .. } => {
                let result = Self::execute_query_operation(&connection_string, &sql, db_service).await;
                let execution_time = start_time.elapsed();

                // Emit query execution event
                let success = result.is_ok();
                let rows_affected = result.as_ref().ok().map(|r| r.rows_affected);

                if let Err(e) = event_bus.emit(AppEvent::QueryExecuted {
                    connection_id: connection_string.clone(),
                    query: sql.clone(),
                    duration: execution_time,
                    rows_affected,
                    success,
                }).await {
                    error!("Failed to emit query executed event: {}", e);
                }

                // Send response
                let _ = response_tx.send(result);

                // Update performance metrics
                Self::update_performance_metrics(&connection_string, execution_time, performance_metrics).await;
            }

            AsyncEvent::ExecuteTableOperations { connection_string, operations, response_tx, .. } => {
                let result = Self::execute_table_operations(&connection_string, operations, db_service).await;
                let _execution_time = start_time.elapsed();

                if let Ok(ref operation_result) = result {
                    // Emit table changes committed event
                    if let Err(e) = event_bus.emit(AppEvent::TableChangesCommitted {
                        table_name: "table".to_string(), // TODO: Get actual table name
                        connection_id: connection_string.clone(),
                        changes_count: operation_result.operations_executed,
                        rows_affected: operation_result.rows_affected,
                        success: true,
                    }).await {
                        error!("Failed to emit table changes event: {}", e);
                    }
                }

                let _ = response_tx.send(result);
            }

            AsyncEvent::CheckConnectionHealth { connection_string, response_tx, .. } => {
                let result = Self::check_connection_health(&connection_string, db_service).await;

                if let Ok(ref health) = result {
                    if let Err(e) = event_bus.emit(AppEvent::ConnectionHealthCheck {
                        connection_id: connection_string.clone(),
                        is_healthy: health.is_healthy,
                    }).await {
                        error!("Failed to emit connection health event: {}", e);
                    }
                }

                let _ = response_tx.send(result);
            }

            AsyncEvent::RefreshSchema { connection_string, schema_name, response_tx, .. } => {
                let result = Self::refresh_schema(&connection_string, schema_name.as_deref(), db_service).await;

                if let Ok(ref schema_info) = result {
                    if let Err(e) = event_bus.emit(AppEvent::TablesRefreshed {
                        connection_id: connection_string.clone(),
                        table_count: schema_info.first().map(|s| s.table_count).unwrap_or(0),
                    }).await {
                        error!("Failed to emit tables refreshed event: {}", e);
                    }
                }

                let _ = response_tx.send(result);
            }

            AsyncEvent::RequestLspDiagnostics { response_tx, .. } => {
                // TODO: Implement LSP diagnostics request
                let _ = response_tx.send(Ok(Vec::new()));
            }

            AsyncEvent::CollectPerformanceMetrics { connection_string, response_tx, .. } => {
                let metrics = Self::collect_performance_metrics(&connection_string, performance_metrics).await;
                let _ = response_tx.send(Ok(metrics));
            }

                    }

        Ok(())
    }

    /// Execute a database query
    async fn execute_query_operation(
        connection_string: &str,
        sql: &str,
        db_service: &DbService,
    ) -> Result<QueryResult> {
        debug!("Executing query: {}", sql);
        db_service.execute_query_unified(connection_string, sql).await
    }

    /// Execute table operations
    async fn execute_table_operations(
        connection_string: &str,
        operations: Vec<TableChangeOperation>,
        db_service: &DbService,
    ) -> Result<TableOperationResult> {
        debug!("Executing {} table operations", operations.len());

        let connection = db_service.get_or_create_unified_connection(connection_string).await?;
        let result = connection.execute_table_changes(&operations).await?;

        Ok(TableOperationResult {
            rows_affected: result.row_count() as u64,
            operations_executed: operations.len(),
            execution_time: Duration::from_millis(0), // TODO: Track actual execution time
            errors: Vec::new(), // TODO: Collect actual errors
        })
    }

    /// Check connection health
    async fn check_connection_health(
        connection_string: &str,
        db_service: &DbService,
    ) -> Result<ConnectionHealth> {
        let start_time = Instant::now();

        match db_service.get_or_create_unified_connection(connection_string).await {
            Ok(connection) => {
                // Simple health check - try to execute a basic query
                match connection.execute_query("SELECT 1").await {
                    Ok(_) => Ok(ConnectionHealth {
                        is_healthy: true,
                        last_check: Instant::now(),
                        response_time: Some(start_time.elapsed()),
                        error_message: None,
                    }),
                    Err(e) => Ok(ConnectionHealth {
                        is_healthy: false,
                        last_check: Instant::now(),
                        response_time: Some(start_time.elapsed()),
                        error_message: Some(e.to_string()),
                    }),
                }
            }
            Err(e) => Ok(ConnectionHealth {
                is_healthy: false,
                last_check: Instant::now(),
                response_time: Some(start_time.elapsed()),
                error_message: Some(e.to_string()),
            }),
        }
    }

    /// Refresh schema information
    async fn refresh_schema(
        _connection_string: &str,
        _schema_name: Option<&str>,
        _db_service: &DbService,
    ) -> Result<Vec<SchemaInfo>> {
        // TODO: Implement schema refresh logic
        Ok(Vec::new())
    }

    /// Collect performance metrics
    async fn collect_performance_metrics(
        connection_string: &str,
        performance_metrics: &Arc<RwLock<HashMap<String, PerformanceMetrics>>>,
    ) -> PerformanceMetrics {
        let metrics = performance_metrics.read().await;
        metrics.get(connection_string).cloned().unwrap_or(PerformanceMetrics {
            connection_string: connection_string.to_string(),
            query_count: 0,
            avg_query_time: Duration::from_millis(0),
            connection_pool_size: 0,
            active_connections: 0,
            last_activity: Instant::now(),
        })
    }

    /// Update performance metrics
    async fn update_performance_metrics(
        connection_string: &str,
        execution_time: Duration,
        performance_metrics: &Arc<RwLock<HashMap<String, PerformanceMetrics>>>,
    ) {
        let mut metrics = performance_metrics.write().await;
        let metric = metrics.entry(connection_string.to_string()).or_insert(PerformanceMetrics {
            connection_string: connection_string.to_string(),
            query_count: 0,
            avg_query_time: Duration::from_millis(0),
            connection_pool_size: 0,
            active_connections: 0,
            last_activity: Instant::now(),
        });

        metric.query_count += 1;
        metric.last_activity = Instant::now();

        // Update average query time
        let total_time = metric.avg_query_time * (metric.query_count - 1) as u32 + execution_time;
        metric.avg_query_time = total_time / metric.query_count as u32;
    }
}

// Wrapper type for the async event sender to implement Global
pub struct AsyncEventSenderWrapper {
    sender: mpsc::Sender<AsyncEvent>,
}

impl AsyncEventSenderWrapper {
    pub fn new(sender: mpsc::Sender<AsyncEvent>) -> Self {
        Self { sender }
    }

    pub fn try_send(&self, event: AsyncEvent) -> Result<(), mpsc::error::TrySendError<AsyncEvent>> {
        self.sender.try_send(event)
    }
}

impl Global for AsyncEventSenderWrapper {}

// Type alias for convenience
pub type AsyncEventSender = AsyncEventSenderWrapper;