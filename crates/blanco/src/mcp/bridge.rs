//! The channel between the MCP server (tokio threads) and the GPUI foreground.
//!
//! Tab operations need the window and the `EditorPanel`, which only exist on
//! the foreground thread, so a tool builds an [`McpRequest`] carrying a
//! one-shot reply channel, sends it through [`McpBridgeClient`], and awaits the
//! answer. `BlancoApp` drains the other end in a foreground task and answers
//! each request in place. `smol::channel` is runtime-agnostic, so both sides
//! can await it.

use std::cell::RefCell;
use std::rc::Rc;

use blanco_core::{ConnectionContext, EnvironmentType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::tool_handlers::write_tab::WriteOperation;

/// One-shot answer channel for a request. Bounded to one so a reply can never
/// block the foreground, and `Result<_, String>` so a domain error ("no such
/// tab") reaches the agent as a tool error rather than a transport failure.
pub(crate) type Reply<T> = smol::channel::Sender<Result<T, String>>;

/// Which tab a tool means. Both fields empty means the active tab. `id` is the
/// stable identifier from `list_tabs`, preferred over `index`, which shifts
/// when tabs are closed or reordered.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
pub(crate) struct TabSelector {
    /// Zero-based position in the tab strip.
    pub index: Option<usize>,
    /// Stable tab id as reported by `list_tabs`.
    pub id: Option<u64>,
}

/// The connection a tab works against, flattened to plain strings for the wire.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TabConnection {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: String,
    pub database: String,
    pub schema: Option<String>,
    /// `DEV`, `TEST` or `PROD` when the connection is tagged.
    pub environment: Option<String>,
}

impl From<&ConnectionContext> for TabConnection {
    fn from(context: &ConnectionContext) -> Self {
        Self {
            connection_id: context.connection_id,
            connection_name: context.connection_name.clone(),
            db_type: context.db_type.as_str().to_string(),
            database: context.database_name.clone(),
            schema: context.schema_name.clone(),
            environment: context
                .environment_type
                .map(|environment| environment.display_name().to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TabSummary {
    pub index: usize,
    /// Stable id for query and script tabs (the ones whose text can be read
    /// and written). Other tab kinds have none.
    pub id: Option<u64>,
    pub title: String,
    /// `query`, `script`, `settings`, `snippet`, `table_structure`,
    /// `object_ddl` or `schema_graph`.
    pub kind: &'static str,
    pub active: bool,
    pub connection: Option<TabConnection>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TabContent {
    /// Line-numbered text, `"<line>: <text>"` per line.
    pub content: String,
    pub total_lines: usize,
    pub start_line: usize,
    pub end_line: usize,
    /// True when lines after `end_line` were left out.
    pub truncated: bool,
}

/// One `write_tab` edit: how `content` lands in the tab and where.
#[derive(Debug, Clone)]
pub(crate) struct TabEdit {
    pub operation: WriteOperation,
    pub content: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct WriteOutcome {
    pub message: String,
    pub total_lines: usize,
}

/// A write the MCP client wants to run, shown to the user for approval.
#[derive(Debug, Clone)]
pub(crate) struct WriteConfirmation {
    pub connection_name: String,
    pub database_name: String,
    pub environment: Option<EnvironmentType>,
    pub sql: String,
}

pub(crate) enum McpRequest {
    ListTabs {
        reply: Reply<Vec<TabSummary>>,
    },
    ReadTab {
        tab: TabSelector,
        start_line: Option<usize>,
        end_line: Option<usize>,
        reply: Reply<TabContent>,
    },
    WriteTab {
        tab: TabSelector,
        edit: TabEdit,
        reply: Reply<WriteOutcome>,
    },
    CreateQueryTab {
        context: ConnectionContext,
        content: Option<String>,
        title: Option<String>,
        reply: Reply<TabSummary>,
    },
    SetActiveTab {
        tab: TabSelector,
        reply: Reply<TabSummary>,
    },
    /// The connection a tab works against, so connection tools can default to
    /// "whatever the user is looking at".
    TabConnection {
        tab: TabSelector,
        reply: Reply<TabConnection>,
    },
    /// Run a query tab exactly as the Run button would, results landing in its
    /// results panel. The reply carries the final result set as JSON once the
    /// run ends, however it ends.
    RunTab {
        tab: TabSelector,
        reply: Reply<serde_json::Value>,
    },
    /// Ask the user whether a write may run. `true` when they click Run.
    ConfirmWrite {
        confirmation: WriteConfirmation,
        reply: smol::channel::Sender<bool>,
    },
}

/// The tokio-side handle. Cheap to clone; every MCP session shares one.
#[derive(Clone)]
pub(crate) struct McpBridgeClient {
    sender: smol::channel::Sender<McpRequest>,
}

impl McpBridgeClient {
    /// A client and the receiver `BlancoApp` drains.
    pub(crate) fn channel() -> (Self, smol::channel::Receiver<McpRequest>) {
        let (sender, receiver) = smol::channel::unbounded();
        (Self { sender }, receiver)
    }

    /// Send the request `build` produces around a fresh reply channel and wait
    /// for the foreground to answer it.
    pub(crate) async fn request<T>(
        &self,
        build: impl FnOnce(Reply<T>) -> McpRequest,
    ) -> Result<T, String> {
        let (reply_sender, reply_receiver) = smol::channel::bounded(1);
        self.sender
            .send(build(reply_sender))
            .await
            .map_err(|_| "Blanco's window is not running".to_string())?;
        reply_receiver
            .recv()
            .await
            .map_err(|_| "Blanco's window closed before answering".to_string())?
    }

    /// Show the write-confirmation dialog and wait for the user's choice.
    pub(crate) async fn confirm_write(
        &self,
        confirmation: WriteConfirmation,
    ) -> Result<bool, String> {
        let (reply, receiver) = smol::channel::bounded(1);
        self.sender
            .send(McpRequest::ConfirmWrite {
                confirmation,
                reply,
            })
            .await
            .map_err(|_| "Blanco's window is not running".to_string())?;
        receiver
            .recv()
            .await
            .map_err(|_| "Blanco's window closed before answering".to_string())
    }
}

/// Deliver a reply, noting when nobody is listening any more (the tool call
/// timed out or the MCP session went away). Never an error for the caller: the
/// foreground has nothing to do about it.
pub(crate) fn send_reply<T>(reply: &Reply<T>, result: Result<T, String>) {
    if reply.try_send(result).is_err() {
        tracing::warn!("MCP request finished after its caller stopped waiting");
    }
}

/// The confirm dialog's answer, shared by its Run button and its close path.
/// Whichever fires first wins, and dropping the last handle without an answer
/// (the dialog was dismissed some other way) counts as a refusal, so the
/// waiting tool call always gets an answer.
#[derive(Clone)]
pub(crate) struct ConfirmReply(Rc<RefCell<Option<smol::channel::Sender<bool>>>>);

impl ConfirmReply {
    pub(crate) fn new(sender: smol::channel::Sender<bool>) -> Self {
        Self(Rc::new(RefCell::new(Some(sender))))
    }

    pub(crate) fn answer(&self, allowed: bool) {
        if let Some(sender) = self.0.borrow_mut().take()
            && sender.try_send(allowed).is_err()
        {
            tracing::warn!("MCP write confirmation arrived after the tool call gave up");
        }
    }
}

impl Drop for ConfirmReply {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) == 1 {
            self.answer(false);
        }
    }
}
