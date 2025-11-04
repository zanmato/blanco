// Async Event Processing Pipeline
//
// This module provides a centralized async processing pipeline that handles
// database operations, LSP communications, and other async tasks without
// blocking the UI thread.

use std::sync::Arc;
use std::time::{Duration, Instant};
use async_std::sync::RwLock;
use async_std::stream::StreamExt;
use std::collections::{HashMap, VecDeque};
use anyhow::{Result, anyhow};
use log::{debug, info, error};
use gpui::Global;

use blanco_core::{QueryResult};
use blanco_core::table_operations::TableChangeOperation;
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
        connection_id: i64,
        sql: String,
        // response_tx: oneshot::Sender<Result<QueryResult>>, // TODO: Replace with async-std compatible pattern
        priority: TaskPriority,
    },

    /// Database table operations (INSERT, UPDATE, DELETE)
    ExecuteTableOperations {
        connection_id: i64,
        operations: Vec<TableChangeOperation>,
        response_tx: async_std::channel::Sender<TableOperationResponse>,
        priority: TaskPriority,
    },

    /// Connection health check
    CheckConnectionHealth {
        connection_id: i64,
        // response_tx: oneshot::Sender<Result<ConnectionHealth>>, // TODO: Replace with async-std compatible pattern
        priority: TaskPriority,
    },

    /// Schema information refresh
    RefreshSchema {
        connection_id: i64,
        schema_name: Option<String>,
        // response_tx: oneshot::Sender<Result<Vec<SchemaInfo>>>, // TODO: Replace with async-std compatible pattern
        priority: TaskPriority,
    },

    /// LSP diagnostics request
    RequestLspDiagnostics {
        connection_id: i64,
        document_uri: String,
        // response_tx: oneshot::Sender<Result<Vec<gpui_component::highlighter::Diagnostic>>>, // TODO: Replace with async-std compatible pattern
        priority: TaskPriority,
    },

    /// Performance metrics collection
    CollectPerformanceMetrics {
        connection_id: i64,
        // response_tx: oneshot::Sender<Result<PerformanceMetrics>>, // TODO: Replace with async-std compatible pattern
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

/// Response for table operations that includes all necessary information for UI handling
#[derive(Debug, Clone)]
pub struct TableOperationResponse {
    pub table_name: String,
    pub connection_id: i64,
    pub success: bool,
    pub rows_affected: Option<u64>,
    pub error_message: Option<String>,
    pub operations_executed: usize,
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
    pub connection_id: i64,
    pub query_count: u64,
    pub avg_query_time: Duration,
    pub connection_pool_size: usize,
    pub active_connections: usize,
    pub last_activity: Instant,
}

/// Async event processor that handles queued async operations
pub struct AsyncEventProcessor {
    /// Event receiver
    event_rx: async_std::channel::Receiver<AsyncEvent>,

    /// Priority queues for different task priorities
    task_queues: Arc<RwLock<HashMap<TaskPriority, VecDeque<AsyncEvent>>>>,

    /// Database service handle
    db_service: DbService,

  
    /// Performance tracking
    performance_metrics: Arc<RwLock<HashMap<i64, PerformanceMetrics>>>,

    /// Debounced queries cache (simplified for future use)
    debounced_queries: Arc<RwLock<HashMap<String, Instant>>>,

    /// Processor state
    is_running: Arc<RwLock<bool>>,
}

impl AsyncEventProcessor {
    /// Create a new async event processor
    pub fn new(db_service: DbService) -> (Self, AsyncEventSender) {
        let (event_tx, event_rx) = async_std::channel::bounded(ASYNC_QUEUE_SIZE);
        let wrapped_sender = AsyncEventSender::new(event_tx);

        let processor = Self {
            event_rx,
            task_queues: Arc::new(RwLock::new(HashMap::new())),
            db_service,
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
        let event_rx = std::mem::replace(&mut self.event_rx, async_std::channel::bounded(1).1);
        let task_queues = self.task_queues.clone();
        let db_service = self.db_service.clone();
            let performance_metrics = self.performance_metrics.clone();
        let debounced_queries = self.debounced_queries.clone();
        let is_running = self.is_running.clone();

        async_std::task::spawn(async move {
            let mut event_rx = event_rx;

            loop {
                {
                    let running = is_running.read().await;
                    if !*running {
                        info!("Async event processor stopping");
                        break;
                    }
                }

                // Handle incoming events
                match event_rx.next().await {
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

                // Process queued tasks by priority
                if let Err(e) = Self::process_task_queue(
                    &task_queues,
                    &db_service,
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
        performance_metrics: &Arc<RwLock<HashMap<i64, PerformanceMetrics>>>,
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
                    let result = Self::process_event(event, db_service, performance_metrics).await;
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
        performance_metrics: &Arc<RwLock<HashMap<i64, PerformanceMetrics>>>,
    ) -> Result<()> {
        let start_time = Instant::now();

        match event {
            AsyncEvent::ExecuteQuery { connection_id, sql, .. } => {
                let result = Self::execute_query_operation(connection_id, &sql, db_service).await;
                let execution_time = start_time.elapsed();

                let success = result.is_ok();
                let rows_affected = result.as_ref().ok().map(|r| r.rows_affected);
                let _error_message = if let Err(ref e) = result {
                    Some(e.to_string())
                } else {
                    None
                };

                // TODO: Re-enable event emission when we have a proper global event system
                // For now, UI components handle their own event emissions
                log::info!("Query execution completed: success={}, rows_affected={:?}", success, rows_affected);

                // Send response
                // TODO: Replace response_tx.send(result) with async-std compatible pattern

                // Update performance metrics
                Self::update_performance_metrics(connection_id, execution_time, performance_metrics).await;
            }

            AsyncEvent::ExecuteTableOperations { connection_id, operations, response_tx, .. } => {
                let result = Self::execute_table_operations(connection_id, operations.clone(), db_service).await;
                let _execution_time = start_time.elapsed();

                // Extract table name from operations (all operations should be for the same table)
                let table_name = operations.first()
                    .map(|op| op.table_name.clone())
                    .unwrap_or_else(|| "table".to_string());

                let (success, rows_affected, error_message, operations_executed) = match &result {
                    Ok(operation_result) => (
                        true,
                        Some(operation_result.rows_affected),
                        None,
                        operation_result.operations_executed
                    ),
                    Err(e) => (
                        false,
                        None,
                        Some(e.to_string()),
                        0
                    )
                };

                // Send response back to caller
                let response = TableOperationResponse {
                    table_name: table_name.clone(),
                    connection_id,
                    success,
                    rows_affected,
                    error_message,
                    operations_executed,
                };

                if let Err(e) = response_tx.send(response).await {
                    log::error!("Failed to send table operation response: {}", e);
                }

                log::info!("Table operations completed: success={}, operations_executed={}", success, operations_executed);
            }

            AsyncEvent::CheckConnectionHealth { connection_id, .. } => {
                let result = Self::check_connection_health(connection_id, db_service).await;

                // TODO: Re-enable connection health events when we have a proper global event system
                if let Ok(ref health) = result {
                    log::info!("Connection health check completed: is_healthy={}", health.is_healthy);
                }

                // TODO: Replace response_tx.send(result) with async-std compatible pattern
            }

            AsyncEvent::RefreshSchema { connection_id, schema_name, .. } => {
                let result = Self::refresh_schema(connection_id, schema_name.as_deref(), db_service).await;

                // TODO: Re-enable schema refresh events when we have a proper global event system
                if let Ok(ref schema_info) = result {
                    let table_count = schema_info.first().map(|s| s.table_count).unwrap_or(0);
                    log::info!("Schema refresh completed: table_count={}", table_count);
                }

                // TODO: Replace response_tx.send(result) with async-std compatible pattern
            }

            AsyncEvent::RequestLspDiagnostics { .. } => {
                // TODO: Implement LSP diagnostics request
            }

            AsyncEvent::CollectPerformanceMetrics { connection_id, .. } => {
                let _metrics = Self::collect_performance_metrics(connection_id, performance_metrics).await;
                // TODO: Replace response_tx.send(Ok(metrics)) with async-std compatible pattern
            }

                    }

        Ok(())
    }

    /// Execute a database query
    async fn execute_query_operation(
        connection_id: i64,
        sql: &str,
        db_service: &DbService,
    ) -> Result<QueryResult> {
        debug!("Executing query: {}", sql);
        db_service.execute_query_by_id(connection_id, sql).await
    }

    /// Execute table operations
    async fn execute_table_operations(
        connection_id: i64,
        operations: Vec<TableChangeOperation>,
        db_service: &DbService,
    ) -> Result<TableOperationResult> {
        debug!("Executing {} table operations", operations.len());

        let connection = db_service.get_or_create_connection(connection_id).await?;

        // Convert table operations to SQL and execute
        let mut total_rows_affected = 0;
        let mut errors = Vec::new();

        for operation in &operations {
            let sql_query = operation.to_sql_query();
            match connection.execute_query(&sql_query).await {
                Ok(result) => {
                    total_rows_affected += result.rows_affected;
                }
                Err(e) => {
                    errors.push(format!("Failed to execute '{}': {}", sql_query, e));
                }
            }
        }

        Ok(TableOperationResult {
            rows_affected: total_rows_affected,
            operations_executed: operations.len(),
            execution_time: Duration::from_millis(0), // TODO: Track actual execution time
            errors,
        })
    }

    /// Check connection health
    async fn check_connection_health(
        connection_id: i64,
        db_service: &DbService,
    ) -> Result<ConnectionHealth> {
        let start_time = Instant::now();

        match db_service.get_or_create_connection(connection_id).await {
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
        _connection_id: i64,
        _schema_name: Option<&str>,
        _db_service: &DbService,
    ) -> Result<Vec<SchemaInfo>> {
        // TODO: Implement schema refresh logic
        Ok(Vec::new())
    }

    /// Collect performance metrics
    async fn collect_performance_metrics(
        connection_id: i64,
        performance_metrics: &Arc<RwLock<HashMap<i64, PerformanceMetrics>>>,
    ) -> PerformanceMetrics {
        let metrics = performance_metrics.read().await;
        metrics.get(&connection_id).cloned().unwrap_or(PerformanceMetrics {
            connection_id,
            query_count: 0,
            avg_query_time: Duration::from_millis(0),
            connection_pool_size: 0,
            active_connections: 0,
            last_activity: Instant::now(),
        })
    }

    /// Update performance metrics
    async fn update_performance_metrics(
        connection_id: i64,
        execution_time: Duration,
        performance_metrics: &Arc<RwLock<HashMap<i64, PerformanceMetrics>>>,
    ) {
        let mut metrics = performance_metrics.write().await;
        let metric = metrics.entry(connection_id).or_insert(PerformanceMetrics {
            connection_id,
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
    sender: async_std::channel::Sender<AsyncEvent>,
}

impl AsyncEventSenderWrapper {
    pub fn new(sender: async_std::channel::Sender<AsyncEvent>) -> Self {
        Self { sender }
    }

    pub fn try_send(&self, event: AsyncEvent) -> Result<(), async_std::channel::TrySendError<AsyncEvent>> {
        self.sender.try_send(event)
    }
}

impl Global for AsyncEventSenderWrapper {}

// Type alias for convenience
pub type AsyncEventSender = AsyncEventSenderWrapper;