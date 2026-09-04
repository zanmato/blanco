//! The rmcp server: tool definitions, bearer authentication and the axum
//! listener. Everything here runs on the tokio pool. Tools that need the
//! window go through [`McpBridgeClient`]; the schema and SQL tools talk to the
//! database service directly.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use app_database::AppDatabase;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use blanco_core::{ConnectionContext, EnvironmentType, StatementAccess};
use database::{ConnectionConfig, DatabaseService, DatabaseServiceTrait};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::bridge::{McpBridgeClient, McpRequest, TabEdit, TabSelector, WriteConfirmation};
use super::tools::{
    DEFAULT_MAX_ROWS, MAX_ROWS_LIMIT, WritePolicy, contains_destructive_ddl, run_sql_json,
    write_policy,
};
use crate::editor::tab_access::WriteOperation;

/// How long a tab operation may wait on the foreground. These are immediate in
/// practice; the bound only guards against a wedged window.
const TAB_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the user gets to answer the write-confirmation dialog.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);
/// How long `run_tab` waits for a query the user can watch and abort.
const RUN_TAB_TIMEOUT: Duration = Duration::from_secs(600);

const INSTRUCTIONS: &str = "Blanco is a desktop SQL editor. Tabs are the user's open editors: query tabs hold \
SQL (or Redis commands) and script tabs hold JavaScript. `list_tabs` shows them; `read_tab` and \
`write_tab` edit their text; `run_tab` runs a query tab like its Run button so the user sees the \
results in the app. `list_connections` lists the saved database connections; `run_sql`, \
`list_schemas`, `list_tables`, `describe_table`, `explore_schema`, `get_table_ddl` and \
`explain_query` work against a connection directly. Statements that modify data may need the user \
to confirm in a Blanco dialog before they run; statements against PROD connections and DROP/TRUNCATE \
always do. Prefer writing SQL into a tab and running it with `run_tab` when the user should see it.";

#[derive(Clone)]
pub(crate) struct BlancoMcpServer {
    bridge: McpBridgeClient,
    db_service: Arc<DatabaseService>,
    app_database: AppDatabase,
    allow_writes: Arc<AtomicBool>,
    tool_router: ToolRouter<Self>,
}

/// A connection request resolved against the saved configuration.
struct ResolvedConnection {
    config: ConnectionConfig,
    database: String,
    environment: Option<EnvironmentType>,
}

impl ResolvedConnection {
    fn context(&self) -> ConnectionContext {
        ConnectionContext {
            connection_id: self.config.id,
            connection_name: self.config.name.clone(),
            db_type: self.config.db_type,
            database_name: self.database.clone(),
            schema_name: None,
            environment_type: self.environment,
        }
    }

    fn is_prod(&self) -> bool {
        self.environment == Some(EnvironmentType::Prod)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ConnectionParams {
    /// Connection id from `list_connections`. Omit to use the connection of
    /// `tab` (default: the active tab).
    pub connection_id: Option<i64>,
    /// Database to use; defaults to the tab's database, else the connection's
    /// configured one.
    pub database: Option<String>,
    /// The tab whose connection to use when `connection_id` is omitted.
    pub tab: Option<TabSelector>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SchemaScopedParams {
    /// Omit to use the connection of `tab` (default: the active tab).
    pub connection_id: Option<i64>,
    pub database: Option<String>,
    pub tab: Option<TabSelector>,
    /// Schema to look in; defaults to the dialect's default schema. Ignored by
    /// backends without schemas.
    pub schema: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TableParams {
    /// Omit to use the connection of `tab` (default: the active tab).
    pub connection_id: Option<i64>,
    pub database: Option<String>,
    pub tab: Option<TabSelector>,
    pub schema: Option<String>,
    pub table: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExploreSchemaParams {
    /// Omit to use the connection of `tab` (default: the active tab).
    pub connection_id: Option<i64>,
    pub database: Option<String>,
    pub tab: Option<TabSelector>,
    /// LIKE patterns matched against table names, e.g. `["user%", "order%"]`.
    /// Empty means every table.
    #[serde(default)]
    pub table_names: Vec<String>,
    /// Tables per page, default 50.
    pub limit: Option<usize>,
    /// Tables to skip, default 0.
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RunSqlParams {
    /// Omit to use the connection of `tab` (default: the active tab).
    pub connection_id: Option<i64>,
    pub database: Option<String>,
    pub tab: Option<TabSelector>,
    /// The statement to run. Reads run immediately; writes may require the user
    /// to confirm in Blanco.
    pub sql: String,
    /// Rows to return at most (default 100, max 1000). Reads without a LIMIT get
    /// one appended in the dialect's syntax.
    pub max_rows: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExplainParams {
    /// Omit to use the connection of `tab` (default: the active tab).
    pub connection_id: Option<i64>,
    pub database: Option<String>,
    pub tab: Option<TabSelector>,
    pub sql: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(crate) struct ReadTabParams {
    /// Which tab; omit for the active tab.
    #[serde(default)]
    pub tab: TabSelector,
    /// 1-based first line, default 1.
    pub start_line: Option<usize>,
    /// 1-based last line (inclusive), default the end of the tab or the first
    /// 200 lines when the tab is large.
    pub end_line: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WriteTabParams {
    /// Which tab; omit for the active tab.
    #[serde(default)]
    pub tab: TabSelector,
    /// `replace_all` (default), `insert_before_line` or `replace_lines`.
    #[serde(default)]
    pub operation: WriteOperation,
    pub content: String,
    /// 1-based line, required for `insert_before_line` and `replace_lines`.
    pub start_line: Option<usize>,
    /// 1-based inclusive end line, required for `replace_lines`.
    pub end_line: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateQueryTabParams {
    /// Omit to use the connection of `tab` (default: the active tab).
    pub connection_id: Option<i64>,
    pub database: Option<String>,
    pub tab: Option<TabSelector>,
    /// Initial text of the tab.
    pub content: Option<String>,
    /// Tab title; defaults to the connection name.
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TabParams {
    #[serde(default)]
    pub tab: TabSelector,
}

fn json_result(value: &impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// A failure the agent should read and react to (bad tab, declined write,
/// database error), as opposed to a protocol error.
fn tool_error(message: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        message.into(),
    )]))
}

fn clamp_max_rows(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(DEFAULT_MAX_ROWS)
        .clamp(1, MAX_ROWS_LIMIT)
}

impl BlancoMcpServer {
    pub(crate) fn new(
        bridge: McpBridgeClient,
        db_service: Arc<DatabaseService>,
        app_database: AppDatabase,
        allow_writes: Arc<AtomicBool>,
    ) -> Self {
        Self {
            bridge,
            db_service,
            app_database,
            allow_writes,
            tool_router: Self::tool_router(),
        }
    }

    /// Pick the connection a tool works against: an explicit id, else the
    /// connection of the given (or active) tab, whose database also fills in
    /// when none was passed.
    async fn resolve_connection(
        &self,
        connection_id: Option<i64>,
        mut database: Option<String>,
        tab: Option<TabSelector>,
    ) -> Result<ResolvedConnection, String> {
        let connection_id = match connection_id {
            Some(connection_id) => connection_id,
            None => {
                let tab_connection = self
                    .bridge_request(
                        |reply| McpRequest::TabConnection {
                            tab: tab.unwrap_or_default(),
                            reply,
                        },
                        TAB_REQUEST_TIMEOUT,
                    )
                    .await?;
                if database.is_none() {
                    database = Some(tab_connection.database);
                }
                tab_connection.connection_id
            }
        };
        let config = self
            .db_service
            .get_connection_config(connection_id)
            .await
            .ok_or_else(|| {
                format!("No connection with id {connection_id}. Use list_connections to find one.")
            })?;
        let environment = self
            .app_database
            .load_connections()
            .await
            .map_err(|error| format!("Failed to read connections: {error}"))?
            .into_iter()
            .find(|connection| connection.id == Some(connection_id))
            .map(|connection| connection.environment_type);
        let database = database
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| config.database.clone());
        Ok(ResolvedConnection {
            config,
            database,
            environment,
        })
    }

    async fn bridge_request<T>(
        &self,
        build: impl FnOnce(super::bridge::Reply<T>) -> McpRequest,
        timeout: Duration,
    ) -> Result<T, String> {
        tokio::time::timeout(timeout, self.bridge.request(build))
            .await
            .map_err(|_| "Blanco did not answer in time".to_string())?
    }

    /// Apply the write policy to `sql`, asking the user when needed. `Ok(())`
    /// means the statement may run.
    async fn authorize(&self, connection: &ResolvedConnection, sql: &str) -> Result<(), String> {
        let access = blanco_core::write_guard::classify(connection.config.db_type, sql);
        if access == StatementAccess::Read {
            return Ok(());
        }
        if connection.config.read_only {
            return Err(format!(
                "Connection \"{}\" is marked read-only in Blanco; the statement was not run.",
                connection.config.name
            ));
        }
        let policy = write_policy(
            access,
            contains_destructive_ddl(sql),
            connection.is_prod(),
            self.allow_writes.load(Ordering::Relaxed),
        );
        if policy == WritePolicy::Run {
            return Ok(());
        }
        let confirmation = WriteConfirmation {
            connection_name: connection.config.name.clone(),
            database_name: connection.database.clone(),
            environment: connection.environment,
            language: connection.config.db_type.dialect().editor_language(),
            sql: sql.to_string(),
        };
        let allowed = tokio::time::timeout(
            CONFIRM_TIMEOUT,
            self.bridge.confirm_write(confirmation),
        )
        .await
        .map_err(|_| {
            "The user did not answer the confirmation dialog in time; the statement was not run."
                .to_string()
        })??;
        if allowed {
            Ok(())
        } else {
            Err("The user declined to run this statement in Blanco.".to_string())
        }
    }
}

#[tool_router]
impl BlancoMcpServer {
    #[tool(
        name = "list_connections",
        description = "List the database connections saved in Blanco: id, name, type, database, environment (DEV/TEST/PROD) and whether Blanco is currently connected."
    )]
    async fn list_connections(&self) -> Result<CallToolResult, ErrorData> {
        let connections = match self.app_database.load_connections().await {
            Ok(connections) => connections,
            Err(error) => return tool_error(format!("Failed to read connections: {error}")),
        };
        let statuses = self
            .db_service
            .get_active_connection_statuses()
            .await
            .unwrap_or_default();
        let entries: Vec<serde_json::Value> = connections
            .into_iter()
            .map(|connection| {
                let connected = connection
                    .id
                    .is_some_and(|id| statuses.keys().any(|(config_id, _)| *config_id == id));
                serde_json::json!({
                    "id": connection.id,
                    "name": connection.name,
                    "db_type": connection.db_type.as_str(),
                    "database": connection.database_name,
                    "host": connection.host,
                    "environment": connection.environment_type.display_name(),
                    "read_only": connection.read_only,
                    "connected": connected,
                })
            })
            .collect();
        json_result(&entries)
    }

    #[tool(
        name = "list_tabs",
        description = "List the open editor tabs with their index, stable id, title, kind, connection and which one is active."
    )]
    async fn list_tabs(&self) -> Result<CallToolResult, ErrorData> {
        match self
            .bridge_request(|reply| McpRequest::ListTabs { reply }, TAB_REQUEST_TIMEOUT)
            .await
        {
            Ok(tabs) => json_result(&tabs),
            Err(error) => tool_error(error),
        }
    }

    #[tool(
        name = "read_tab",
        description = "Read the text of a query or script tab as line-numbered content, optionally limited to a line range. Defaults to the active tab."
    )]
    async fn read_tab(
        &self,
        Parameters(params): Parameters<ReadTabParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .bridge_request(
                |reply| McpRequest::ReadTab {
                    tab: params.tab,
                    start_line: params.start_line,
                    end_line: params.end_line,
                    reply,
                },
                TAB_REQUEST_TIMEOUT,
            )
            .await
        {
            Ok(content) => json_result(&content),
            Err(error) => tool_error(error),
        }
    }

    #[tool(
        name = "write_tab",
        description = "Write text into a query or script tab: replace everything, insert before a line, or replace a line range. Defaults to the active tab. Does not run anything."
    )]
    async fn write_tab(
        &self,
        Parameters(params): Parameters<WriteTabParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .bridge_request(
                |reply| McpRequest::WriteTab {
                    tab: params.tab,
                    edit: TabEdit {
                        operation: params.operation,
                        content: params.content,
                        start_line: params.start_line,
                        end_line: params.end_line,
                    },
                    reply,
                },
                TAB_REQUEST_TIMEOUT,
            )
            .await
        {
            Ok(outcome) => json_result(&outcome),
            Err(error) => tool_error(error),
        }
    }

    #[tool(
        name = "create_query_tab",
        description = "Open a new query tab on a connection, optionally with initial text and a title, and make it active."
    )]
    async fn create_query_tab(
        &self,
        Parameters(params): Parameters<CreateQueryTabParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        let context = connection.context();
        match self
            .bridge_request(
                |reply| McpRequest::CreateQueryTab {
                    context,
                    content: params.content,
                    title: params.title,
                    reply,
                },
                TAB_REQUEST_TIMEOUT,
            )
            .await
        {
            Ok(summary) => json_result(&summary),
            Err(error) => tool_error(error),
        }
    }

    #[tool(
        name = "set_active_tab",
        description = "Bring a tab to the front by index or id."
    )]
    async fn set_active_tab(
        &self,
        Parameters(params): Parameters<TabParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .bridge_request(
                |reply| McpRequest::SetActiveTab {
                    tab: params.tab,
                    reply,
                },
                TAB_REQUEST_TIMEOUT,
            )
            .await
        {
            Ok(summary) => json_result(&summary),
            Err(error) => tool_error(error),
        }
    }

    #[tool(
        name = "run_tab",
        description = "Run a query tab exactly like its Run button (statement at the cursor, or the selection), so the results show up in Blanco. Returns the final result set (up to 100 rows). Writes against PROD connections ask the user to confirm first. Defaults to the active tab."
    )]
    async fn run_tab(
        &self,
        Parameters(params): Parameters<TabParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .bridge_request(
                |reply| McpRequest::RunTab {
                    tab: params.tab,
                    reply,
                },
                RUN_TAB_TIMEOUT,
            )
            .await
        {
            Ok(result) => json_result(&result),
            Err(error) => tool_error(error),
        }
    }

    #[tool(
        name = "run_sql",
        description = "Run a SQL statement (or Redis command) and return the result set as JSON. Uses the active tab's connection unless connection_id is given. Reads run at once and get a row limit appended when they have none. Statements that modify data may open a confirmation dialog in Blanco; PROD connections and DROP/TRUNCATE always do."
    )]
    async fn run_sql(
        &self,
        Parameters(params): Parameters<RunSqlParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        if let Err(error) = self.authorize(&connection, &params.sql).await {
            return tool_error(error);
        }
        let max_rows = clamp_max_rows(params.max_rows);
        let is_read = blanco_core::write_guard::classify(connection.config.db_type, &params.sql)
            == StatementAccess::Read;
        let auto_limit = is_read.then_some(max_rows);
        match run_sql_json(
            self.db_service.as_ref(),
            connection.config.id,
            Some(&connection.database),
            connection.config.db_type,
            &params.sql,
            auto_limit,
            max_rows,
        )
        .await
        {
            Ok(result) => json_result(&result),
            Err(error) => tool_error(format!("Query execution failed: {error:#}")),
        }
    }

    #[tool(
        name = "explain_query",
        description = "Run the dialect's EXPLAIN for a statement and return the plan rows."
    )]
    async fn explain_query(
        &self,
        Parameters(params): Parameters<ExplainParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        // Some backends execute the statement to explain it, so the inner SQL
        // is authorized like a run.
        if let Err(error) = self.authorize(&connection, &params.sql).await {
            return tool_error(error);
        }
        let wrapped = sql_parser::explain::wrap_explain(connection.config.db_type, &params.sql);
        match run_sql_json(
            self.db_service.as_ref(),
            connection.config.id,
            Some(&connection.database),
            connection.config.db_type,
            &wrapped,
            None,
            MAX_ROWS_LIMIT,
        )
        .await
        {
            Ok(result) => json_result(&result),
            Err(error) => tool_error(format!("EXPLAIN failed: {error:#}")),
        }
    }

    #[tool(
        name = "list_schemas",
        description = "List the schemas of a database. Empty for backends without schemas (SQLite, Redis)."
    )]
    async fn list_schemas(
        &self,
        Parameters(params): Parameters<ConnectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        let live = match self
            .db_service
            .get_or_create_connection(connection.config.id, Some(&connection.database))
            .await
        {
            Ok(live) => live,
            Err(error) => return tool_error(format!("Failed to connect: {error:#}")),
        };
        if !live.supports_schemas() {
            return json_result(&serde_json::json!({ "schemas": [] }));
        }
        match live.get_schemas().await {
            Ok(schemas) => json_result(&serde_json::json!({ "schemas": schemas })),
            Err(error) => tool_error(format!("Failed to list schemas: {error:#}")),
        }
    }

    #[tool(
        name = "list_tables",
        description = "List tables, views and materialized views in a schema (or the whole database for backends without schemas)."
    )]
    async fn list_tables(
        &self,
        Parameters(params): Parameters<SchemaScopedParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        let live = match self
            .db_service
            .get_or_create_connection(connection.config.id, Some(&connection.database))
            .await
        {
            Ok(live) => live,
            Err(error) => return tool_error(format!("Failed to connect: {error:#}")),
        };
        let schema = params.schema.as_deref();
        let tables = match live.get_tables(schema).await {
            Ok(tables) => tables,
            Err(error) => return tool_error(format!("Failed to list tables: {error:#}")),
        };
        let views = live.get_views(schema).await.unwrap_or_default();
        let materialized_views = live
            .get_materialized_views(schema)
            .await
            .unwrap_or_default();
        json_result(&serde_json::json!({
            "tables": tables,
            "views": views,
            "materialized_views": materialized_views,
        }))
    }

    #[tool(
        name = "describe_table",
        description = "Columns (name, type, nullability, default, primary key) and indexes of a table."
    )]
    async fn describe_table(
        &self,
        Parameters(params): Parameters<TableParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        let live = match self
            .db_service
            .get_or_create_connection(connection.config.id, Some(&connection.database))
            .await
        {
            Ok(live) => live,
            Err(error) => return tool_error(format!("Failed to connect: {error:#}")),
        };
        let schema = params.schema.as_deref();
        let columns = match live.get_columns_for_table(&params.table, schema).await {
            Ok(columns) => columns,
            Err(error) => return tool_error(format!("Failed to read columns: {error:#}")),
        };
        let indexes = live
            .get_indexes_for_table(&params.table, schema)
            .await
            .unwrap_or_default();
        json_result(&serde_json::json!({
            "table": params.table,
            "schema": params.schema,
            "columns": columns,
            "indexes": indexes,
        }))
    }

    #[tool(
        name = "explore_schema",
        description = "Paginated overview of tables with their columns, foreign keys and inbound references, optionally filtered by LIKE patterns on the table name."
    )]
    async fn explore_schema(
        &self,
        Parameters(params): Parameters<ExploreSchemaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        // The service takes the patterns as one comma-separated string.
        let table_names = if params.table_names.is_empty() {
            None
        } else {
            Some(params.table_names.join(","))
        };
        match self
            .db_service
            .get_database_schema_paginated(
                connection.config.id,
                Some(&connection.database),
                table_names.as_deref(),
                Some(params.limit.unwrap_or(50) as i64),
                Some(params.offset.unwrap_or(0) as i64),
            )
            .await
        {
            Ok(schema) => json_result(&schema),
            Err(error) => tool_error(format!("Failed to read schema: {error:#}")),
        }
    }

    #[tool(
        name = "get_table_ddl",
        description = "The CREATE TABLE statement for a table, as the backend reports it."
    )]
    async fn get_table_ddl(
        &self,
        Parameters(params): Parameters<TableParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let connection = match self
            .resolve_connection(params.connection_id, params.database, params.tab)
            .await
        {
            Ok(connection) => connection,
            Err(error) => return tool_error(error),
        };
        let live = match self
            .db_service
            .get_or_create_connection(connection.config.id, Some(&connection.database))
            .await
        {
            Ok(live) => live,
            Err(error) => return tool_error(format!("Failed to connect: {error:#}")),
        };
        match live
            .table_ddl(params.schema.as_deref(), &params.table)
            .await
        {
            Ok(ddl) => json_result(&serde_json::json!({ "ddl": ddl })),
            Err(error) => tool_error(format!("Failed to read DDL: {error:#}")),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BlancoMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("blanco", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(INSTRUCTIONS.to_string());
        info
    }
}

/// Reserve the loopback port synchronously so an occupied port is reported to
/// the caller right away instead of from inside the serve task.
pub(crate) fn bind(port: u16) -> anyhow::Result<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("binding 127.0.0.1:{port}"))?;
    listener
        .set_nonblocking(true)
        .context("configuring the MCP listener")?;
    Ok(listener)
}

/// Serve `server` on `listener` until `cancellation` fires. Every request must
/// carry `Authorization: Bearer <token>`.
pub(crate) async fn serve(
    listener: std::net::TcpListener,
    token: String,
    server: BlancoMcpServer,
    cancellation: CancellationToken,
) -> anyhow::Result<()> {
    let listener =
        tokio::net::TcpListener::from_std(listener).context("registering the MCP listener")?;
    let router = router(token, server, cancellation.clone());
    axum::serve(listener, router)
        .with_graceful_shutdown(cancellation.cancelled_owned())
        .await
        .context("serving MCP")?;
    Ok(())
}

pub(crate) fn router(
    token: String,
    server: BlancoMcpServer,
    cancellation: CancellationToken,
) -> axum::Router {
    let config = StreamableHttpServerConfig::default().with_cancellation_token(cancellation);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let expected: Arc<str> = Arc::from(token);
    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(
            expected,
            require_bearer,
        ))
}

async fn require_bearer(
    State(expected): State<Arc<str>>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if presented.is_some_and(|header| bearer_matches(header, &expected)) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "unauthorized",
        )
            .into_response()
    }
}

/// Compare without short-circuiting on the first differing byte, so response
/// timing does not leak how much of the token was right.
fn bearer_matches(header: &str, expected: &str) -> bool {
    let Some(presented) = header.strip_prefix("Bearer ") else {
        return false;
    };
    let presented = presented.trim().as_bytes();
    let expected = expected.as_bytes();
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_comparison() {
        assert!(bearer_matches("Bearer abc123", "abc123"));
        assert!(bearer_matches("Bearer abc123 ", "abc123"));
        assert!(!bearer_matches("Bearer abc124", "abc123"));
        assert!(!bearer_matches("Bearer abc12", "abc123"));
        assert!(!bearer_matches("Basic abc123", "abc123"));
        assert!(!bearer_matches("abc123", "abc123"));
    }

    #[test]
    fn max_rows_is_clamped() {
        assert_eq!(clamp_max_rows(None), DEFAULT_MAX_ROWS);
        assert_eq!(clamp_max_rows(Some(0)), 1);
        assert_eq!(clamp_max_rows(Some(5)), 5);
        assert_eq!(clamp_max_rows(Some(50_000)), MAX_ROWS_LIMIT);
    }
}
