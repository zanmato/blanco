//! The built-in MCP server. External agents (Claude Code, OpenCode, ...) call
//! into the running Blanco to read and write tabs, run SQL and browse schema,
//! instead of Blanco calling an LLM itself. See `mcp/server.rs` for the tools,
//! `mcp/bridge.rs` for the hop onto the GPUI foreground and `mcp/discovery.rs`
//! for the agent workspace folder that carries the connection details.

pub(crate) mod bridge;
pub(crate) mod discovery;
pub(crate) mod server;
pub(crate) mod tools;

#[cfg(test)]
mod bridge_test;
#[cfg(test)]
mod server_test;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use app_database::AppDatabase;
use database::DatabaseService;
use gpui::{App, BorrowAppContext as _, Global, Task};
use tokio_util::sync::CancellationToken;

use crate::app_settings::AppSettings;
use crate::status_bar::{ActivityReporter, ActivityResult};
use bridge::{McpBridgeClient, McpRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpStatus {
    Stopped,
    Running { port: u16, token: String },
    Error(String),
}

struct RunningServer {
    cancellation: CancellationToken,
    /// Awaits the serve future and records its failure; dropping it aborts the
    /// tokio task, so `stop` only has to cancel and drop.
    _task: Task<()>,
}

/// App-wide handle to the MCP server: its bridge channel, the write-policy
/// switch and the running listener, if any.
pub(crate) struct McpService {
    client: McpBridgeClient,
    /// Handed to `BlancoApp` when the window opens. Requests sent before that
    /// queue in the channel.
    receiver: Option<smol::channel::Receiver<McpRequest>>,
    allow_writes: Arc<AtomicBool>,
    running: Option<RunningServer>,
    status: McpStatus,
}

impl Global for McpService {}

impl McpService {
    /// Create the global. Called once at startup before the window opens so
    /// `BlancoApp` can take the bridge receiver.
    pub(crate) fn init(cx: &mut App) {
        let (client, receiver) = McpBridgeClient::channel();
        cx.set_global(Self {
            client,
            receiver: Some(receiver),
            allow_writes: Arc::new(AtomicBool::new(false)),
            running: None,
            status: McpStatus::Stopped,
        });
    }

    pub(crate) fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// The bridge receiver, once. `None` after the first call and when the
    /// service was never initialised (visual tests that build `BlancoApp`
    /// directly).
    pub(crate) fn take_bridge_receiver(
        cx: &mut App,
    ) -> Option<smol::channel::Receiver<McpRequest>> {
        cx.try_global::<Self>()?;
        cx.update_global::<Self, _>(|service, _| service.receiver.take())
    }

    pub(crate) fn client(&self) -> McpBridgeClient {
        self.client.clone()
    }

    pub(crate) fn status(&self) -> &McpStatus {
        &self.status
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self.status, McpStatus::Running { .. })
    }

    /// Flip the write policy without restarting; read by every `run_sql`.
    pub(crate) fn set_allow_writes(cx: &mut App, allow_writes: bool) {
        cx.update_global::<Self, _>(|service, _| {
            service.allow_writes.store(allow_writes, Ordering::Relaxed);
        });
    }

    /// Bind the configured port and start serving. Failures (port in use,
    /// unwritable workspace) land in `status()` and the status bar; the app
    /// keeps running without the server.
    pub(crate) fn start(cx: &mut App) {
        if Self::global(cx).is_running() {
            return;
        }
        let settings = &AppSettings::global(cx).settings.mcp;
        let port = settings.port;
        let allow_writes = settings.allow_writes;
        Self::set_allow_writes(cx, allow_writes);

        let token = generate_token();
        let listener = match server::bind(port) {
            Ok(listener) => listener,
            Err(error) => {
                Self::fail(cx, format!("{error:#}"));
                return;
            }
        };
        let bound_port = listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or(port);

        let workspace = discovery::workspace_dir();
        if let Err(error) = discovery::write_workspace_config(&workspace, bound_port, &token) {
            tracing::error!("Failed to write the agent workspace config: {error:#}");
            ActivityReporter::global(cx)
                .begin("MCP server")
                .finish(ActivityResult::Err(
                    format!("could not write {}", workspace.display()).into(),
                ));
        }

        let handler = server::BlancoMcpServer::new(
            Self::global(cx).client(),
            Arc::new(DatabaseService::global(cx).clone()),
            AppDatabase::global(cx).clone(),
            Self::global(cx).allow_writes.clone(),
        );
        let cancellation = CancellationToken::new();
        let serve_task = gpui_tokio::Tokio::spawn(
            cx,
            server::serve(listener, token.clone(), handler, cancellation.clone()),
        );
        let task = cx.spawn(async move |cx| {
            let failure = match serve_task.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("{error:#}")),
                Err(join_error) => Some(format!("MCP server task failed: {join_error}")),
            };
            if let Some(message) = failure {
                tracing::error!("MCP server stopped: {message}");
                cx.update(|cx| Self::fail(cx, message));
            }
        });

        cx.update_global::<Self, _>(|service, _| {
            service.running = Some(RunningServer {
                cancellation,
                _task: task,
            });
            service.status = McpStatus::Running {
                port: bound_port,
                token,
            };
        });
        tracing::info!(
            "MCP server listening on {}",
            discovery::server_url(bound_port)
        );
    }

    /// Stop serving, drop the listener and remove the generated workspace
    /// files. A no-op when nothing is running.
    pub(crate) fn stop(cx: &mut App) {
        let running = cx.update_global::<Self, _>(|service, _| {
            service.status = McpStatus::Stopped;
            service.running.take()
        });
        let Some(running) = running else {
            return;
        };
        running.cancellation.cancel();
        drop(running);
        if let Err(error) = discovery::remove_workspace_config(&discovery::workspace_dir()) {
            tracing::warn!("Failed to remove the agent workspace config: {error:#}");
        }
    }

    pub(crate) fn restart(cx: &mut App) {
        Self::stop(cx);
        Self::start(cx);
    }

    fn fail(cx: &mut App, message: String) {
        tracing::error!("MCP server: {message}");
        ActivityReporter::global(cx)
            .begin("MCP server")
            .finish(ActivityResult::Err(message.clone().into()));
        cx.update_global::<Self, _>(|service, _| {
            service.running = None;
            service.status = McpStatus::Error(message);
        });
    }
}

/// 256 bits of randomness as 64 hex characters. Regenerated on every start so
/// a leaked token stops working at the next restart.
fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
