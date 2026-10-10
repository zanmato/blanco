use blanco_ui::{IconName as BlancoIcon, SqlView, SqlViewMessage};
use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::mcp::McpService;
use crate::mcp::bridge::{
    ConfirmReply, McpRequest, Reply as McpReply, WriteAnswer, WriteConfirmation, send_reply,
};
use gpui::{
    Action, App, AppContext, BorrowAppContext, Context, Entity, ExternalPaths, FocusHandle,
    Focusable, InteractiveElement, IntoElement, Menu, MenuItem, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, actions,
    div, prelude::FluentBuilder, px, svg,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, IconName, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    global_state::GlobalState,
    h_flex,
    kbd::Kbd,
    menu::AppMenuBar,
    notification::{Notification, NotificationType},
    resizable::{ResizablePanel, ResizableState, h_resizable, resizable_panel},
    spinner::Spinner,
    status_bar::StatusBar,
    v_flex,
};
use serde::Deserialize;
use smol::channel;
use tracing::{debug, error, info};

use crate::{
    app_settings::AppSettings,
    command_palette::CommandPalette,
    connection_credentials,
    connection_modal::NewConnectionModal,
    connections::{ConnectionsPanel, ConnectionsPanelEvent},
    editor::{
        EditorPanel, EditorPanelEvent, ObjectDdlParams, TabCreationParams, TableStructureParams,
    },
    history_panel::{HistoryPanel, HistoryPanelEvent},
    result_ext::ResultExt,
    snippets_panel::{RefreshSnippets, SnippetsPanel, SnippetsPanelEvent},
    sqlite_file,
    status_bar::{ActivityMessage, ActivityReporter, ActivityResult, StatusBarState, StatusKind},
    update_manager::{UpdateManager, UpdateState},
};
use app_database::{AppDatabase, ConnectionData};
use blanco_core::ConnectionContext;

actions!(
    blanco_app,
    [
        Quit,
        OpenConnection,
        OpenSettings,
        OpenTerminal,
        OpenNewConnectionModal,
        ImportConnections,
        ExportConnections,
        OpenSqliteFile,
        NewSnippet,
        CommitChanges,
        RollbackChanges,
        CopyAsCSV,
        CopyAsTSV,
        CopyAsJSON,
        CopyAsSQL,
        CopyAsVALUES,
        CopyAsMarkdown,
        ExportData,
        ExportAsCSV,
        ExportAsTSV,
        ExportAsJSON,
        ExportAsSQL,
        ExportAsMarkdown,
        ToggleFilterBar,
        OpenSqlFile,
        SaveTab,
        SaveTabAs,
        RunSqlFile,
        ClearSelection,
        ToggleRenderWhitespace,
        ToggleWordWrap,
        IncreaseEditorFontSize,
        DecreaseEditorFontSize,
        ResetEditorFontSize,
        RunQuery,
        ExplainQuery,
        ToggleCommandPalette,
        CloseActiveTab,
        NewQueryTab,
        ActivateNextTab,
        ActivatePreviousTab,
        ToggleResults,
        CancelQuery,
        ImportTableData,
        ExportTableData,
        RefreshConnections,
        StartCellEdit,
        EditNextCell,
        EditPrevCell,
        ToggleCellExpression,
        ClearCompareSelection,
    ]
);

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct ConnectToConnection {
    pub connection_id: i64,
}

/// Bring an editor tab to the front by its position in the tab strip.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct ActivateEditorTab {
    pub index: usize,
}

/// Switch to a theme registered in the `ThemeRegistry` by name.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SwitchTheme {
    pub name: String,
}

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = blanco_app, no_json)]
pub struct ToggleSidebar;

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct NewQuery {
    pub connection_id: i64,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub content: Option<String>,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CopyAsFormat {
    pub format: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct AddRow;

/// Copy the values of one result column: one per line, or as an IN list.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CopyColumnValues {
    pub column: usize,
    /// Each value once.
    pub distinct: bool,
    /// `('a', 'b')` instead of one value per line.
    pub as_in_list: bool,
    /// Only the selected rows when several are selected, else every row.
    pub selected_only: bool,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DuplicateRow {
    pub row: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DeleteRow {
    pub row: usize,
}

/// Hide a result column, by its position in the grid.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct HideResultColumn {
    pub col: usize,
}

/// Pin a result column to the left edge, or unpin it.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct PinResultColumn {
    pub col: usize,
    pub pinned: bool,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SetCellNull {
    pub row: usize,
    pub col: usize,
}

/// Filter the result's table on the server by a cell's value.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct FilterByCellValue {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SetCellDefault {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SelectCellForCompare {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CompareCellWithSelected {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SelectRowForCompare {
    pub row: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CompareRowWithSelected {
    pub row: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct EditCellInPopover {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct OpenSnippetEditor {
    pub snippet_id: Option<i64>,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct RenameTab {
    pub tab_index: usize,
    pub new_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct ExecuteSubstitutedQuery {
    pub query: String,
    pub connection_id: i64,
    pub database_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CreateNewQueryTab {
    pub context: ConnectionContext,
    pub table_name: Option<String>,
    /// When set, the new tab immediately inspects `table_name` as a key (used by
    /// the "Inspect Key" action on key/value backends). Plain "New Query" leaves
    /// this false and just opens an editor tab.
    pub inspect_key: bool,
}

/// Open a JavaScript script tab on a connection. Scripts are connection-scoped
/// rather than object-scoped, so there is no table to scaffold from.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CreateNewScriptTab {
    pub context: ConnectionContext,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct OpenObjectDdl {
    pub kind: blanco_core::RoutineKind,
    pub context: ConnectionContext,
    pub object_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct OpenTableStructure {
    pub context: ConnectionContext,
    pub table_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct OpenSchemaGraph {
    pub context: ConnectionContext,
}

/// Back up a database with the native dump tool.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct BackupDatabase {
    pub context: ConnectionContext,
}

/// Restore a dump into a database with the native client tools.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct RestoreDatabase {
    pub context: ConnectionContext,
}

/// Reload one connection's subtree in the sidebar, e.g. after DDL ran from a
/// structure tab.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct RefreshConnectionTree {
    pub connection_id: i64,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct EditConnection {
    pub connection_id: i64,
}

// Action for database connection state
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DatabaseConnected {
    pub connection_id: i64,
    pub database_name: String,
}

// Action for formatting the current SQL query
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct FormatQuery;

impl From<database::DatabaseConnectedMessage> for DatabaseConnected {
    fn from(msg: database::DatabaseConnectedMessage) -> Self {
        Self {
            connection_id: msg.connection_id,
            database_name: msg.database_name,
        }
    }
}

// Action dispatched when the database service detects that a connection has
// dropped (e.g. the server closed an idle connection overnight, or an SSH
// tunnel died). Lets the sidebar reflect the disconnected state.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DatabaseDisconnected {
    pub connection_id: i64,
    pub database_name: String,
    /// Closed after its idle timeout rather than lost.
    pub idle: bool,
}

impl From<database::DatabaseDisconnectedMessage> for DatabaseDisconnected {
    fn from(msg: database::DatabaseDisconnectedMessage) -> Self {
        Self {
            connection_id: msg.connection_id,
            database_name: msg.database_name,
            idle: msg.idle,
        }
    }
}

/// Corner radius of the sidebar and editor cards. Deliberately separate from
/// `theme.radius`, which stays smaller for the controls inside the cards.
pub(crate) const PANEL_RADIUS: gpui::Pixels = px(8.);

/// Gap between the cards and the window edges. Half of it is applied as padding
/// on each side of the split so the two cards end up `PANEL_GAP` apart.
pub(crate) const PANEL_GAP: gpui::Pixels = px(6.);

/// Width of the icon rail on the left edge of the sidebar card. It is the
/// only part of the sidebar that stays visible while collapsed.
pub(crate) const SIDEBAR_RAIL_WIDTH: gpui::Pixels = px(42.);

/// Which view is active in the sidebar, selected from the icon rail.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SidebarTab {
    Connections,
    Snippets,
    History,
}

impl SidebarTab {
    /// Rail order, top to bottom.
    const ALL: [SidebarTab; 3] = [Self::Connections, Self::Snippets, Self::History];

    fn label(self) -> &'static str {
        match self {
            Self::Connections => "Connections",
            Self::Snippets => "Snippets",
            Self::History => "History",
        }
    }

    fn icon(self) -> BlancoIcon {
        match self {
            Self::Connections => BlancoIcon::Database,
            Self::Snippets => BlancoIcon::Braces,
            Self::History => BlancoIcon::History,
        }
    }
}

/// An MCP write waiting for the user, with the channel its tool call awaits.
type QueuedConfirmation = (WriteConfirmation, channel::Sender<WriteAnswer>);

/// One tag for every write confirmation, so a burst of requests keeps a single
/// notification instead of stacking one per statement.
const MCP_CONFIRMATION_NOTIFICATION_TAG: &str = "mcp-write-confirmation";

pub struct BlancoApp {
    focus_handle: FocusHandle,
    sidebar: Entity<ConnectionsPanel>,
    snippets_panel: Entity<SnippetsPanel>,
    history_panel: Entity<HistoryPanel>,
    editor_panel: Entity<EditorPanel>,
    /// The object last selected in the sidebar for the active tab, so the
    /// sidebar is only moved when the active tab shows something else.
    revealed_target: Option<crate::connections::RevealTarget>,
    command_palette: Entity<CommandPalette>,
    sidebar_collapsed: bool,
    sidebar_tab: SidebarTab,
    /// `None` on macOS, where the menus live in the system menu bar.
    app_menu_bar: Option<Entity<AppMenuBar>>,
    main_resize_state: Entity<ResizableState>,
    status_bar: Entity<StatusBarState>,
    _subscriptions: Vec<Subscription>,
    _action_task: Task<()>,
    _activity_task: Task<()>,
    _idle_disconnect_task: Task<()>,
    _open_files_task: Task<()>,
    _save_window_bounds_task: Task<()>,
    _save_editor_font_size_task: Task<()>,
    /// Drains MCP requests onto the foreground; see `spawn_mcp_listener`.
    _mcp_task: Task<()>,
    /// `run_tab` callers waiting for their query tab's run to end, matched to
    /// `EditorPanelEvent::QueryRunEnded` by tab index in arrival order.
    mcp_run_replies: VecDeque<(usize, McpReply<serde_json::Value>)>,
    /// Write confirmations waiting for their turn; see
    /// `spawn_mcp_confirmation_queue`.
    mcp_confirmations: channel::Sender<QueuedConfirmation>,
    _mcp_confirmation_task: Task<()>,
}

impl BlancoApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        init_menus(cx);

        let action_task = Self::spawn_service_listener(cx);
        let idle_disconnect_task = Self::spawn_idle_disconnect(cx);
        let open_files_task = Self::spawn_open_file_listener(window, cx);
        let (status_bar, activity_task) = Self::spawn_activity_status(cx);
        let mcp_task = Self::spawn_mcp_listener(window, cx);
        let (mcp_confirmations, mcp_confirmation_task) =
            Self::spawn_mcp_confirmation_queue(window, cx);

        let sidebar = cx.new(|cx| ConnectionsPanel::new(window, cx));
        let snippets_panel = cx.new(|cx| SnippetsPanel::new(window, cx));
        let history_panel = cx.new(|cx| HistoryPanel::new(window, cx));
        let main_resize_state = cx.new(|_| ResizableState::default());

        let saved_tabs = Self::load_saved_tabs(cx);
        let editor_panel = cx.new(|cx| EditorPanel::new_with_saved_tabs(window, cx, saved_tabs));
        let command_palette = cx.new(|cx| {
            CommandPalette::new(sidebar.downgrade(), editor_panel.downgrade(), window, cx)
        });
        let app_menu_bar = (!cfg!(target_os = "macos")).then(|| AppMenuBar::new(cx));

        let mut subscriptions = Self::wire_subscriptions(
            &sidebar,
            &snippets_panel,
            &history_panel,
            &editor_panel,
            &status_bar,
            window,
            cx,
        );
        subscriptions.push(Self::wire_mcp_confirmation_notification(window, cx));
        if let Some(update_manager) = UpdateManager::try_global(cx) {
            let update_state = update_manager.state.clone();
            let just_updated = update_manager.updated_from.is_some();
            subscriptions.push(cx.observe(&update_state, |_, _, cx| cx.notify()));
            if just_updated {
                Self::notify_updated(window, cx);
            }
        }

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        Self {
            focus_handle,
            sidebar,
            snippets_panel,
            history_panel,
            editor_panel,
            revealed_target: None,
            command_palette,
            sidebar_collapsed: false,
            sidebar_tab: SidebarTab::Connections,
            app_menu_bar,
            main_resize_state,
            status_bar,
            _subscriptions: subscriptions,
            _action_task: action_task,
            _activity_task: activity_task,
            _idle_disconnect_task: idle_disconnect_task,
            _open_files_task: open_files_task,
            _save_window_bounds_task: Task::ready(()),
            _save_editor_font_size_task: Task::ready(()),
            _mcp_task: mcp_task,
            mcp_run_replies: VecDeque::new(),
            mcp_confirmations,
            _mcp_confirmation_task: mcp_confirmation_task,
        }
    }

    /// Tell the user the update they restarted for is the build now running.
    fn notify_updated(window: &mut Window, cx: &mut Context<Self>) {
        let version = env!("CARGO_PKG_VERSION");
        window.defer(cx, move |window, cx| {
            window.push_notification(
                Notification::new()
                    .message(format!("Updated to v{version}, click to view changelog"))
                    .with_type(NotificationType::Success)
                    .on_click(move |_, _, cx| {
                        cx.open_url(&UpdateManager::changelog_url(&format!("v{version}")));
                    }),
                cx,
            );
        });
    }

    /// Clicking the write confirmation's or a finished backup's system
    /// notification brings Blanco's window forward, and the notification goes
    /// away once the window is in front by any route.
    fn wire_mcp_confirmation_notification(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let window_handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            if response.tag != MCP_CONFIRMATION_NOTIFICATION_TAG
                && response.tag != crate::backup::NOTIFICATION_TAG
            {
                return;
            }
            cx.activate(true);
            window_handle
                .update(cx, |_, window, _| window.activate_window())
                .log_err();
        });
        cx.observe_window_activation(window, |_, window, cx| {
            if window.is_window_active() {
                cx.dismiss_system_notification(MCP_CONFIRMATION_NOTIFICATION_TAG);
                cx.dismiss_system_notification(crate::backup::NOTIFICATION_TAG);
            }
        })
    }

    /// Answer MCP requests on the foreground. The receiver is handed over by
    /// `McpService` once; a second `BlancoApp` (tests) gets no listener.
    fn spawn_mcp_listener(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        let Some(receiver) = McpService::take_bridge_receiver(cx) else {
            return Task::ready(());
        };
        cx.spawn_in(window, async move |weak_app, cx| {
            while let Ok(request) = receiver.recv().await {
                if weak_app
                    .update_in(cx, |app, window, cx| {
                        app.handle_mcp_request(request, window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
            info!("MCP bridge listener ended");
        })
    }

    /// Show MCP write confirmations one at a time. Agents running in parallel
    /// can ask at once, and stacking their dialogs would put a newer statement
    /// under the pointer just as the user clicks Run on the one they read.
    fn spawn_mcp_confirmation_queue(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (channel::Sender<QueuedConfirmation>, Task<()>) {
        let (sender, receiver) = channel::unbounded::<QueuedConfirmation>();
        let task = cx.spawn_in(window, async move |weak_app, cx| {
            while let Ok((confirmation, reply)) = receiver.recv().await {
                // The tool call timed out or its MCP session ended while queued.
                if reply.is_closed() {
                    continue;
                }
                let (answer_sender, answer_receiver) = channel::bounded(1);
                let abandoned = Rc::new(Cell::new(false));
                if weak_app
                    .update_in(cx, |app, window, cx| {
                        app.show_mcp_write_confirmation(
                            confirmation,
                            ConfirmReply::new(answer_sender),
                            receiver.clone(),
                            abandoned.clone(),
                            window,
                            cx,
                        );
                    })
                    .is_err()
                {
                    break;
                }
                let answer = smol::future::or(
                    async { Some(answer_receiver.recv().await.unwrap_or(WriteAnswer::Refuse)) },
                    async {
                        reply.closed().await;
                        None
                    },
                )
                .await;
                match answer {
                    Some(answer) => {
                        if reply.try_send(answer).is_err() {
                            tracing::warn!(
                                "MCP write confirmation arrived after the tool call gave up"
                            );
                        }
                    }
                    None => {
                        // Closing it from here could pop a dialog the user
                        // opened on top, so it stays up with Run disabled
                        // until they dismiss it.
                        abandoned.set(true);
                        if weak_app
                            .update_in(cx, |_, window, _| window.refresh())
                            .is_err()
                        {
                            break;
                        }
                        answer_receiver.recv().await.ok();
                    }
                }
            }
        });
        (sender, task)
    }

    fn handle_mcp_request(
        &mut self,
        request: McpRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor_panel = self.editor_panel.clone();
        match request {
            McpRequest::ListTabs { reply } => {
                let tabs = editor_panel.read(cx).tab_summaries(cx);
                send_reply(&reply, Ok(tabs));
            }
            McpRequest::ReadTab {
                tab,
                start_line,
                end_line,
                reply,
            } => {
                let result = editor_panel.read(cx).resolve_tab(tab).and_then(|index| {
                    editor_panel
                        .read(cx)
                        .read_tab_text(index, start_line, end_line, cx)
                });
                send_reply(&reply, result);
            }
            McpRequest::WriteTab { tab, edit, reply } => {
                let result = editor_panel.update(cx, |panel, cx| {
                    let index = panel.resolve_tab(tab)?;
                    panel.write_tab_text(index, &edit, window, cx)
                });
                send_reply(&reply, result);
            }
            McpRequest::CreateQueryTab {
                context,
                content,
                title,
                reply,
            } => {
                let result = editor_panel.update(cx, |panel, cx| {
                    panel.create_query_tab_for_mcp(context, content, title, window, cx)
                });
                send_reply(&reply, result);
                cx.notify();
            }
            McpRequest::SetActiveTab { tab, reply } => {
                let result = editor_panel.update(cx, |panel, cx| {
                    let index = panel.resolve_tab(tab)?;
                    panel.activate_tab_for_mcp(index, window, cx)
                });
                send_reply(&reply, result);
            }
            McpRequest::TabConnection { tab, reply } => {
                let panel = editor_panel.read(cx);
                let result = panel.resolve_tab(tab).and_then(|index| {
                    let summary = panel.tab_summary_at(index, cx)?;
                    summary.connection.ok_or_else(|| {
                        format!(
                            "Tab {index} ({}) is not bound to a connection; pass connection_id or pick a query tab.",
                            summary.kind
                        )
                    })
                });
                send_reply(&reply, result);
            }
            McpRequest::RunTab { tab, reply } => {
                let index = match editor_panel.read(cx).resolve_tab(tab) {
                    Ok(index) => index,
                    Err(error) => {
                        send_reply(&reply, Err(error));
                        return;
                    }
                };
                // Register before running: a run that fails synchronously
                // emits `QueryRunEnded` from inside `run_query_tab_for_mcp`.
                self.mcp_run_replies.push_back((index, reply));
                let outcome = editor_panel.update(cx, |panel, cx| {
                    panel.run_query_tab_for_mcp(index, window, cx)
                });
                if let Err(error) = outcome {
                    self.resolve_mcp_run(index, Err(error));
                }
            }
            McpRequest::ConfirmWrite {
                confirmation,
                reply,
            } => {
                // The dialog is easy to miss while Blanco sits behind the
                // agent's window. The notification only brings Blanco forward:
                // the write is answered in the dialog, where the whole
                // statement is readable.
                if !window.is_window_active() {
                    cx.show_system_notification(gpui::SystemNotification {
                        tag: MCP_CONFIRMATION_NOTIFICATION_TAG.into(),
                        title: confirmation.title().into(),
                        body: confirmation.target().into(),
                        actions: Vec::new(),
                    });
                }
                if self
                    .mcp_confirmations
                    .try_send((confirmation, reply))
                    .is_err()
                {
                    error!("MCP confirmation queue is closed; the write was refused");
                }
                // Refresh the open dialog's count of waiting requests.
                window.refresh();
            }
        }
    }

    /// Hand a finished run to the oldest `run_tab` caller waiting on that tab.
    fn resolve_mcp_run(&mut self, tab_index: usize, outcome: Result<serde_json::Value, String>) {
        let Some(position) = self
            .mcp_run_replies
            .iter()
            .position(|(index, _)| *index == tab_index)
        else {
            return;
        };
        if let Some((_, reply)) = self.mcp_run_replies.remove(position) {
            send_reply(&reply, outcome);
        }
    }

    /// The dialog an MCP write waits on. Run and "Don't ask again" run it;
    /// Cancel, Escape, or the dialog going away for any other reason refuse
    /// through `ConfirmReply`'s drop. Clicking the backdrop does nothing, because the
    /// click that focuses Blanco's window would otherwise refuse the write.
    fn show_mcp_write_confirmation(
        &mut self,
        confirmation: WriteConfirmation,
        reply: ConfirmReply,
        waiting: channel::Receiver<QueuedConfirmation>,
        abandoned: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_prod = confirmation.environment == Some(blanco_core::EnvironmentType::Prod);
        let title = confirmation.title();
        let target = confirmation.target();
        let sql_view = cx.new(|cx| {
            let mut view = SqlView::new(
                usize::MAX,
                cx.theme().highlight_theme.clone(),
                confirmation.language,
                cx,
            )
            .show_copy_button(false);
            view.append_text(&SqlViewMessage::SqlStatement(confirmation.sql), cx);
            view
        });
        let remember_label = (!confirmation.rememberable_kinds.is_empty()).then(|| {
            format!(
                "Don't ask again for {}",
                confirmation.rememberable_kinds.join(", ")
            )
        });
        let remember_tooltip = format!(
            "Run it, and let this agent run these statements on {} without asking until it disconnects",
            confirmation.connection_name
        );
        window.open_dialog(cx, move |dialog, _, cx| {
            let reply_on_run = reply.clone();
            let reply_on_remember = reply.clone();
            let reply_on_close = reply.clone();
            let abandoned = abandoned.get();
            let waiting_count = waiting.len();
            dialog
                .title(title)
                .overlay_closable(false)
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "Connected through the MCP server, against {target}."
                                )),
                        )
                        .child(
                            div()
                                .h(px(320.))
                                .min_h_0()
                                .overflow_hidden()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(sql_view.clone()),
                        )
                        .when(abandoned, |this| {
                            this.child(div().text_sm().text_color(cx.theme().danger).child(
                                "The agent stopped waiting for an answer. The statement will not run.",
                            ))
                        })
                        .when(waiting_count > 0, |this| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if waiting_count == 1 {
                                        "1 more request is waiting.".to_string()
                                    } else {
                                        format!("{waiting_count} more requests are waiting.")
                                    }),
                            )
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("mcp-write-cancel").label("Cancel").outline()),
                        )
                        .when_some(remember_label.clone(), |footer, label| {
                            footer.child(
                                DialogAction::new().child(
                                    Button::new("mcp-write-remember")
                                        .outline()
                                        .label(label)
                                        .tooltip(remember_tooltip.clone())
                                        .disabled(abandoned)
                                        .on_click(move |_, _, _| {
                                            reply_on_remember.answer(WriteAnswer::RunAndRemember)
                                        }),
                                ),
                            )
                        })
                        .child(
                            DialogAction::new().child(
                                Button::new("mcp-write-run")
                                    .danger()
                                    .label(if is_prod { "Run on PROD" } else { "Run" })
                                    .disabled(abandoned)
                                    .on_click(move |_, _, _| reply_on_run.answer(WriteAnswer::Run)),
                            ),
                        ),
                )
                .on_close(move |_, _, _| reply_on_close.answer(WriteAnswer::Refuse))
        });
    }

    /// Spawn the background task that translates `DatabaseServiceMessage`s
    /// (emitted from any context, including tokio tasks without a `cx`) into
    /// dispatched `DatabaseConnected`/`DatabaseDisconnected` actions. Installs
    /// the message sender on the global `DatabaseService`.
    fn spawn_service_listener(cx: &mut Context<Self>) -> Task<()> {
        let (action_sender, action_receiver) =
            channel::unbounded::<database::DatabaseServiceMessage>();

        cx.update_global::<database::DatabaseService, _>(
            |db_service: &mut database::DatabaseService, _cx| {
                db_service.set_action_sender(action_sender);
            },
        );

        cx.spawn(async move |_weak_handle, cx| {
            info!("DatabaseServiceMessage listener task started");
            while let Ok(msg) = action_receiver.recv().await {
                match msg {
                    database::DatabaseServiceMessage::Connected(conn_msg) => {
                        info!(
                            "DatabaseServiceMessage::Connected received for connection_id: {}, database_name: {}",
                            conn_msg.connection_id, conn_msg.database_name
                        );
                        _weak_handle.update(cx, |_, cx| {
                            info!("Dispatching DatabaseConnected action to app");
                            cx.dispatch_action(&DatabaseConnected::from(conn_msg));
                        }).log_err();
                    }
                    database::DatabaseServiceMessage::Disconnected(disconn_msg) => {
                        info!(
                            "DatabaseServiceMessage::Disconnected received for connection_id: {}, database_name: {}",
                            disconn_msg.connection_id, disconn_msg.database_name
                        );
                        _weak_handle.update(cx, |_, cx| {
                            cx.dispatch_action(&DatabaseDisconnected::from(disconn_msg));
                        }).log_err();
                    }
                }
            }
            info!("DatabaseServiceMessage listener task ended");
        })
    }

    /// Close connections that have sat unused past their idle timeout. A
    /// connection with uncommitted grid edits is kept open, and the service
    /// itself skips one with a query running.
    fn spawn_idle_disconnect(cx: &mut Context<Self>) -> Task<()> {
        const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
        cx.spawn(async move |app, cx| {
            loop {
                cx.background_executor().timer(SWEEP_INTERVAL).await;
                let Ok((db_service, default_timeout, keep)) = app.update(cx, |app, cx| {
                    let default_timeout = AppSettings::global(cx)
                        .settings
                        .database
                        .idle_disconnect_timeout();
                    let keep = app.editor_panel.read(cx).connections_with_pending_edits(cx);
                    (
                        database::DatabaseService::global(cx).clone(),
                        default_timeout,
                        keep,
                    )
                }) else {
                    break;
                };
                db_service
                    .disconnect_idle_connections(default_timeout, &keep)
                    .await;
            }
        })
    }

    /// Open the database files passed at launch and those the platform asks
    /// the running app to open.
    fn spawn_open_file_listener(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        let Some(requests) = cx
            .try_global::<sqlite_file::OpenFileRequests>()
            .map(|requests| requests.receiver.clone())
        else {
            return Task::ready(());
        };
        cx.spawn_in(window, async move |app, cx| {
            while let Ok(paths) = requests.recv().await {
                let opened = app.update_in(cx, |app, window, cx| {
                    for path in &paths {
                        app.open_database_file(path, window, cx);
                    }
                });
                if opened.is_err() {
                    break;
                }
            }
        })
    }

    /// Open the SQLite database at `path` in a new query tab: through its
    /// saved connection when there is one, otherwise through a temporary
    /// connection that can be saved later. Files are recognised by their
    /// header, whatever their extension.
    pub(crate) fn open_database_file(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !sqlite_file::is_sqlite_file(path) {
            window.push_notification(
                (
                    NotificationType::Error,
                    SharedString::from(format!("{} is not a SQLite database.", path.display())),
                ),
                cx,
            );
            return;
        }
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if let Some(connection) = self.sidebar.read(cx).sqlite_connection_for(&path) {
            self.open_query_tab_for_file(&connection, window, cx);
            return;
        }

        let connection = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.add_temporary_sqlite_connection(&path, cx)
        });
        let Some(config) = connection.to_connection_config() else {
            return;
        };
        let db_service = database::DatabaseService::global(cx).clone();
        cx.spawn_in(window, async move |app, cx| {
            db_service.add_connection_config(config).await;
            app.update_in(cx, |app, window, cx| {
                if let Some(id) = connection.id {
                    app.sidebar.update(cx, |sidebar, cx| {
                        sidebar.expand_connection(id, window, cx);
                    });
                }
                app.open_query_tab_for_file(&connection, window, cx);
            })
            .log_err();
        })
        .detach();
    }

    fn open_query_tab_for_file(
        &mut self,
        connection: &ConnectionData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection_id) = connection.id else {
            return;
        };
        let action = CreateNewQueryTab {
            table_name: None,
            inspect_key: false,
            context: ConnectionContext {
                connection_id,
                connection_name: connection.name.clone(),
                db_type: connection.db_type,
                // The sidebar addresses a SQLite database by its schema.
                database_name: "main".to_string(),
                schema_name: None,
                environment_type: Some(connection.environment_type),
            },
        };
        self.on_create_new_query_tab(&action, window, cx);
    }

    /// Create the status-bar entity and the background task that folds
    /// app-wide `ActivityMessage`s (reported through the global
    /// `ActivityReporter`) into it.
    fn spawn_activity_status(cx: &mut Context<Self>) -> (Entity<StatusBarState>, Task<()>) {
        let status_bar = cx.new(|_| StatusBarState::default());
        let (activity_sender, activity_receiver) = channel::unbounded::<ActivityMessage>();
        cx.set_global(ActivityReporter::new(activity_sender));
        let activity_task = cx.spawn({
            let status_bar = status_bar.downgrade();
            async move |_weak_handle, cx| {
                while let Ok(message) = activity_receiver.recv().await {
                    status_bar
                        .update(cx, |state, cx| state.apply(message, cx))
                        .log_err();
                }
            }
        });
        (status_bar, activity_task)
    }

    /// Synchronously load the persisted query tabs from the app database. Blocks
    /// on the tokio runtime because the app cannot render until it knows which
    /// tabs to restore. Load failures degrade to an empty tab set.
    fn load_saved_tabs(cx: &mut Context<Self>) -> Vec<app_database::QueryTabData> {
        info!("Loading saved tabs from database");
        let app_database = AppDatabase::global(cx);
        gpui_tokio::Tokio::handle(cx).block_on(async {
            match app_database.load_query_tabs().await {
                Ok(tabs) => {
                    info!("Loaded {} tabs from database", tabs.len());
                    for tab in &tabs {
                        debug!(
                            "Tab '{}' (db_id: {:?}, connection_id: {:?}, content_len: {})",
                            tab.title,
                            tab.id,
                            tab.connection_id,
                            tab.content.len()
                        );
                    }
                    tabs
                }
                Err(e) => {
                    error!("Failed to load tabs: {}", e);
                    Vec::new()
                }
            }
        })
    }

    /// Wire the cross-panel event subscriptions and window observers. Returns
    /// the subscription guards to be stored on the app so they live as long as
    /// it does.
    fn wire_subscriptions(
        sidebar: &Entity<ConnectionsPanel>,
        snippets_panel: &Entity<SnippetsPanel>,
        history_panel: &Entity<HistoryPanel>,
        editor_panel: &Entity<EditorPanel>,
        status_bar: &Entity<StatusBarState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        let mut subscriptions = Vec::new();

        // Sidebar: open the edit-connection modal, and the first-run dialog.
        subscriptions.push(cx.subscribe_in(
            sidebar,
            window,
            move |app, sidebar, event, window, cx| match event {
                ConnectionsPanelEvent::EditConnection {
                    connection_data, ..
                } => {
                    app.edit_connection_with_secrets(*connection_data.clone(), window, cx);
                }
                ConnectionsPanelEvent::DuplicateConnection { connection_data } => {
                    app.open_new_connection_modal(Some(*connection_data.clone()), None, window, cx);
                }
                ConnectionsPanelEvent::SaveTemporaryConnection { connection_data } => {
                    let temporary_id = connection_data.id;
                    let prefill = ConnectionData {
                        id: None,
                        ..*connection_data.clone()
                    };
                    app.open_new_connection_modal(Some(prefill), temporary_id, window, cx);
                }
                // First-run experience: if the initial load found no saved connections,
                // greet the user with the new connection dialog instead of an empty window.
                // Not when Blanco was started to open a database file.
                ConnectionsPanelEvent::FirstLoadCompleted => {
                    let opening_files = cx
                        .try_global::<sqlite_file::OpenFileRequests>()
                        .is_some_and(|requests| requests.at_launch);
                    if sidebar.read(cx).connections.is_empty() && !opening_files {
                        app.on_new_connection_modal(&OpenNewConnectionModal, window, cx);
                    }
                }
            },
        ));

        // Snippets panel: deletion already refreshes the panel itself.
        subscriptions.push(cx.subscribe_in(
            snippets_panel,
            window,
            move |_app, _snippets_panel, event, _window, _cx| match event {
                SnippetsPanelEvent::SnippetDeleted => {}
            },
        ));

        // History panel: drop a saved query into the active editor tab.
        subscriptions.push(cx.subscribe_in(
            history_panel,
            window,
            move |app, _history_panel, event, window, cx| match event {
                HistoryPanelEvent::InsertQuery(query) => {
                    let query = query.clone();
                    app.editor_panel.update(cx, |editor_panel, cx| {
                        editor_panel.insert_into_active_query(&query, window, cx);
                    });
                }
            },
        ));

        // Editor panel: keep the history panel in sync as queries run.
        subscriptions.push(cx.subscribe_in(
            editor_panel,
            window,
            move |app, _editor_panel, event, _window, cx| match event {
                EditorPanelEvent::QueryRecorded => {
                    app.history_panel.update(cx, |panel, cx| {
                        panel.reload(cx);
                    });
                }
                EditorPanelEvent::QueryRunEnded { tab_index, outcome } => {
                    app.resolve_mcp_run(*tab_index, outcome.clone());
                    app.follow_active_tab(cx);
                }
            },
        ));

        // Switch theme with the system appearance when Follow System is on.
        subscriptions.push(cx.observe_window_appearance(window, |_, window, cx| {
            crate::settings::follow_system_appearance(window.appearance(), cx);
            cx.notify();
        }));

        // Select the active tab's table in the sidebar as tabs change.
        subscriptions.push(cx.observe(editor_panel, |app, _, cx| app.follow_active_tab(cx)));

        // Re-render the status bar whenever the activity state changes.
        subscriptions.push(cx.observe(status_bar, |_, _, cx| cx.notify()));

        // Persist window geometry as it changes (debounced in the handler).
        subscriptions.push(cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_window_bounds(window, cx);
        }));

        subscriptions
    }

    /// Select the active tab's table in the sidebar when it changed since the
    /// last time, unless the setting is off.
    fn follow_active_tab(&mut self, cx: &mut Context<Self>) {
        if !AppSettings::global(cx).settings.general.follow_active_tab {
            return;
        }
        let target = self.editor_panel.read(cx).active_reveal_target(cx);
        if target == self.revealed_target {
            return;
        }
        self.revealed_target = target.clone();
        if let Some(target) = target {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.reveal(target, cx));
        }
    }

    #[cfg(any(test, feature = "screenshots"))]
    pub fn editor_panel(&self) -> &Entity<EditorPanel> {
        &self.editor_panel
    }

    /// Persist the current window geometry to the app settings database so it
    /// can be restored on the next launch. Called on every resize/move, so the
    /// database write is debounced to avoid hammering it during a drag.
    fn persist_window_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (maximized, bounds) = match window.window_bounds() {
            gpui::WindowBounds::Maximized(bounds) => (true, bounds),
            gpui::WindowBounds::Fullscreen(bounds) => (true, bounds),
            gpui::WindowBounds::Windowed(bounds) => (false, bounds),
        };

        let x = f32::from(bounds.origin.x);
        let y = f32::from(bounds.origin.y);
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);

        cx.update_global::<AppSettings, _>(|app_settings, _| {
            let window = &mut app_settings.settings.window;
            window.x = Some(x);
            window.y = Some(y);
            window.width = Some(width);
            window.height = Some(height);
            window.maximized = maximized;
        });

        let db = AppDatabase::global(cx).clone();
        self._save_window_bounds_task = cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let values = [
                ("window.x", x.to_string()),
                ("window.y", y.to_string()),
                ("window.width", width.to_string()),
                ("window.height", height.to_string()),
                ("window.maximized", maximized.to_string()),
            ];
            for (key, value) in values {
                if let Err(e) = db.save_setting(key, &value, false).await {
                    error!("Failed to save {}: {}", key, e);
                }
            }
        });
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> StatusBar {
        let line = self.status_bar.read(cx).display();
        let leading = match line.kind {
            StatusKind::Busy => Spinner::new().xsmall().into_any_element(),
            StatusKind::Ok => Icon::new(IconName::CircleCheck)
                .xsmall()
                .text_color(cx.theme().success)
                .into_any_element(),
            StatusKind::Err => Icon::new(IconName::CircleX)
                .xsmall()
                .text_color(cx.theme().danger)
                .into_any_element(),
            StatusKind::Idle => Icon::new(IconName::Check)
                .xsmall()
                .text_color(cx.theme().muted_foreground)
                .into_any_element(),
        };

        StatusBar::new().left(
            h_flex()
                .items_center()
                .gap_1()
                .child(leading)
                .child(line.text),
        )
    }

    /// The window title bar: app logo and menu bar on the left, the sidebar
    /// toggle on the right. `TitleBar` lays its children out left of the window
    /// controls, so a trailing child lands next to minimize/maximize/close.
    fn render_title_bar(&self, window: &mut Window, cx: &Context<Self>) -> impl IntoElement {
        // On macOS the logo sits beside the 12px traffic lights with no menu
        // bar after it, where the size used elsewhere dwarfs the controls.
        let logo_height = if cfg!(target_os = "macos") {
            px(18.)
        } else {
            px(28.)
        };
        TitleBar::new().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .w_full()
                // macOS has no window controls after the toggle, so without
                // this it touches the window edge instead of lining up with
                // the cards below.
                .when(cfg!(target_os = "macos"), |this| this.pr(PANEL_GAP))
                .child(
                    h_flex()
                        .items_center()
                        .gap_x_3()
                        .child(
                            svg()
                                .h(logo_height)
                                .w(logo_height * LOGO_ASPECT_RATIO)
                                .text_color(window.text_style().color)
                                .path("images/blanco.svg"),
                        )
                        .children(self.app_menu_bar.clone()),
                )
                .child(self.render_palette_trigger(window, cx))
                .child(
                    h_flex()
                        .items_center()
                        .gap_x_2()
                        .children(self.render_update_button(cx))
                        .child(self.render_sidebar_toggle(cx)),
                ),
        )
    }

    /// Looks like a search field but only opens the command palette, so the
    /// palette can be found without knowing its shortcut.
    fn render_palette_trigger(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let shortcut = Kbd::binding_for_action(&ToggleCommandPalette, None, window);
        h_flex()
            .id("palette-trigger")
            .items_center()
            .justify_between()
            .gap_8()
            .h(px(26.))
            .w(px(420.))
            .px_3()
            .rounded_md()
            .bg(cx.theme().input_background())
            .border_1()
            .border_color(cx.theme().border)
            .text_color(cx.theme().muted_foreground)
            .text_sm()
            .cursor_pointer()
            .hover(|this| this.border_color(cx.theme().accent_foreground))
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(ToggleCommandPalette), cx);
            })
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::Search).size(px(14.)))
                    .child("Search commands..."),
            )
            .when_some(shortcut, |this, kbd| this.child(kbd))
    }

    /// Offers a newer release: "Update" downloads it, then "Restart" swaps the
    /// executable and relaunches.
    fn render_update_button(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let state = UpdateManager::try_global(cx)?.state.read(cx).clone();
        let button = Button::new("update").outline().small();
        match state {
            UpdateState::UpToDate => None,
            UpdateState::Available(version) => Some(
                button
                    .label("Update")
                    .tooltip(format!("Download Blanco {version}"))
                    .on_click(|_, window, cx| {
                        let download = UpdateManager::download_update(cx);
                        window
                            .spawn(cx, async move |cx| {
                                let Err(error) = download.await else {
                                    return;
                                };
                                tracing::error!("Failed to download update: {error:#}");
                                cx.update(|window, cx| {
                                    window.push_notification(
                                        (
                                            NotificationType::Error,
                                            SharedString::from(format!(
                                                "Failed to download update: {error:#}"
                                            )),
                                        ),
                                        cx,
                                    );
                                })
                                .log_err();
                            })
                            .detach();
                    }),
            ),
            UpdateState::Manual(version) => Some(
                button
                    .label("Update")
                    .tooltip(format!("Open the Blanco {version} release page"))
                    .on_click(move |_, _, cx| {
                        cx.open_url(&UpdateManager::changelog_url(&format!("v{version}")));
                    }),
            ),
            UpdateState::Downloading(_) => Some(button.label("Downloading").loading(true)),
            UpdateState::Ready(version) => Some(
                button
                    .label("Restart")
                    .tooltip(format!("Restart to update to Blanco {version}"))
                    .on_click(|_, window, cx| {
                        if let Err(error) = UpdateManager::apply_pending_update(cx) {
                            tracing::error!("Failed to apply update: {error:#}");
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    SharedString::from(format!(
                                        "Failed to apply update: {error:#}"
                                    )),
                                ),
                                cx,
                            );
                        }
                    }),
            ),
        }
    }

    fn render_sidebar_toggle(&self, _cx: &Context<Self>) -> impl IntoElement {
        Button::new("toggle-sidebar")
            .ghost()
            .small()
            .icon(if self.sidebar_collapsed {
                Icon::new(IconName::PanelLeftOpen).size_4()
            } else {
                Icon::new(IconName::PanelLeftClose).size_4()
            })
            .tooltip("Toggle Sidebar")
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(ToggleSidebar), cx);
            })
    }

    /// The icon rail on the left edge of the sidebar: one button per
    /// [`SidebarTab`] at the top and a settings shortcut pinned to the bottom.
    /// It sits outside the resizable group so that collapsing the sidebar
    /// leaves the rail in place without disturbing the panel's stored width.
    fn render_sidebar_rail(&self, cx: &Context<Self>) -> impl IntoElement {
        let collapsed = self.sidebar_collapsed;
        // The rail is the activity bar, so its resting icons take the inactive
        // tab colour (overlay0 in Catppuccin, what VS Code uses for
        // `activityBar.inactiveForeground`) rather than `muted_foreground`,
        // which is tuned for secondary text and reads far too bright on icons.
        let inactive = cx.theme().tab_foreground;
        let active = cx.theme().foreground;
        v_flex()
            .flex_none()
            .w(SIDEBAR_RAIL_WIDTH)
            .h_full()
            .items_center()
            .py(PANEL_GAP)
            .gap_1()
            // The rail supplies the whole gap to whichever card follows it,
            // mirroring the shell's inset on its left, so the icons sit
            // centred between the window edge and the card border.
            .mr(PANEL_GAP)
            .children(SidebarTab::ALL.into_iter().enumerate().map(|(ix, tab)| {
                let is_active = !collapsed && tab == self.sidebar_tab;
                Button::new(("sidebar-rail", ix))
                    .ghost()
                    // `Button::icon` scales the icon from the button size, and
                    // a child-bearing button only derives padding from
                    // `Sizable::size`, so the box is sized explicitly to get a
                    // 24px glyph in a 36px hit target.
                    .w(px(36.))
                    .h(px(36.))
                    .px_0()
                    .justify_center()
                    .child(Icon::from(tab.icon()).size_6().text_color(if is_active {
                        active
                    } else {
                        inactive
                    }))
                    .tooltip(tab.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_sidebar_tab(tab, cx);
                    }))
            }))
            .child(div().flex_1())
            .child(
                Button::new("sidebar-rail-settings")
                    .ghost()
                    .w(px(36.))
                    .h(px(36.))
                    .px_0()
                    .justify_center()
                    .child(
                        Icon::from(BlancoIcon::Settings)
                            .size_6()
                            .text_color(inactive),
                    )
                    .tooltip("Settings")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(OpenSettings), cx);
                    }),
            )
    }

    /// The left sidebar card: the panel selected in the rail.
    fn render_sidebar_panel(&self, cx: &Context<Self>) -> ResizablePanel {
        resizable_panel()
            // Kept in the tree while collapsed rather than omitted, so the
            // group's state retains the width across a collapse/re-open.
            .visible(!self.sidebar_collapsed)
            .size(px(288.))
            .size_range(px(288.)..px(500.))
            .flex_none()
            .child(
                div().size_full().pr(PANEL_GAP / 2.).child(
                    div()
                        .w_full()
                        .h_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .bg(cx.theme().sidebar)
                        .rounded(PANEL_RADIUS)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            // Same inset as the editor card's tab bar so the
                            // filter and search inputs line up with it.
                            div()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .pt(PANEL_GAP)
                                .map(|this| match self.sidebar_tab {
                                    SidebarTab::Connections => this.child(self.sidebar.clone()),
                                    SidebarTab::Snippets => this.child(self.snippets_panel.clone()),
                                    SidebarTab::History => this.child(self.history_panel.clone()),
                                }),
                        ),
                ),
            )
    }

    /// Rail click: switch to `tab` and make sure the sidebar is open, or
    /// collapse it when the already visible section is clicked again.
    pub(crate) fn select_sidebar_tab(&mut self, tab: SidebarTab, cx: &mut Context<Self>) {
        if tab == self.sidebar_tab && !self.sidebar_collapsed {
            self.sidebar_collapsed = true;
        } else {
            self.sidebar_tab = tab;
            self.sidebar_collapsed = false;
            // Pick up queries run since this panel was last shown.
            if tab == SidebarTab::History {
                self.history_panel.update(cx, |panel, cx| {
                    panel.reload(cx);
                });
            }
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn command_palette(&self) -> &Entity<CommandPalette> {
        &self.command_palette
    }

    #[cfg(test)]
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    #[cfg(test)]
    pub fn sidebar_tab(&self) -> SidebarTab {
        self.sidebar_tab
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        McpService::stop(cx);
        cx.quit();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        cx.notify();
    }

    fn on_open_terminal(&mut self, _: &OpenTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.toggle_terminal_for_active_tab(window, cx);
        });
    }

    fn on_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_settings_tab(window, cx);
        });
    }

    fn on_activate_editor_tab(
        &mut self,
        action: &ActivateEditorTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = action.index;
        self.editor_panel.update(cx, |panel, cx| {
            panel.activate_tab(index, window, cx);
        });
    }

    fn on_open_sql_file(&mut self, _: &OpenSqlFile, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel
            .update(cx, |panel, cx| panel.prompt_open_sql_files(window, cx));
    }

    fn on_save_tab(&mut self, _: &SaveTab, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel
            .update(cx, |panel, cx| panel.save_active_tab(window, cx));
    }

    fn on_save_tab_as(&mut self, _: &SaveTabAs, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel
            .update(cx, |panel, cx| panel.save_active_tab_as(window, cx));
    }

    fn on_run_sql_file(&mut self, _: &RunSqlFile, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel
            .update(cx, |panel, cx| panel.prompt_run_sql_file(window, cx));
    }

    fn on_close_active_tab(
        &mut self,
        _: &CloseActiveTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.close_active_tab(window, cx);
        });
    }

    fn on_switch_theme(
        &mut self,
        action: &SwitchTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::settings::apply_theme_by_name(&action.name, cx);
    }

    fn on_new_snippet(&mut self, _: &NewSnippet, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.create_snippet_tab(window, cx);
        });
    }

    fn on_open_snippet_editor(
        &mut self,
        action: &OpenSnippetEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = action.snippet_id {
            self.editor_panel.update(cx, |panel, cx| {
                panel.open_snippet_tab(id, window, cx);
            });
        } else {
            self.editor_panel.update(cx, |panel, cx| {
                panel.create_snippet_tab(window, cx);
            });
        }
    }

    fn on_refresh_snippets(
        &mut self,
        _: &RefreshSnippets,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snippets_panel.update(cx, |panel, cx| {
            panel.refresh_snippets(cx);
        });
    }

    /// A new query tab on the active tab's connection and database.
    fn on_new_query_tab(&mut self, _: &NewQueryTab, window: &mut Window, cx: &mut Context<Self>) {
        let context = self.editor_panel.read(cx).active_context(cx).or_else(|| {
            self.sidebar
                .read(cx)
                .selected_table_context(cx)
                .map(|(context, _)| context)
        });
        if let Some(context) = context {
            let action = CreateNewQueryTab {
                context,
                table_name: None,
                inspect_key: false,
            };
            self.on_create_new_query_tab(&action, window, cx);
        }
    }

    fn on_activate_next_tab(
        &mut self,
        _: &ActivateNextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wrap = AppSettings::global(cx).settings.editor.tab_navigation_wraps;
        self.editor_panel.update(cx, |panel, cx| {
            panel.activate_adjacent_tab(1, wrap, window, cx)
        });
    }

    fn on_activate_previous_tab(
        &mut self,
        _: &ActivatePreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wrap = AppSettings::global(cx).settings.editor.tab_navigation_wraps;
        self.editor_panel.update(cx, |panel, cx| {
            panel.activate_adjacent_tab(-1, wrap, window, cx)
        });
    }

    fn on_toggle_results(&mut self, _: &ToggleResults, _: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel
            .update(cx, |panel, cx| panel.toggle_results(cx));
    }

    fn on_cancel_query(&mut self, _: &CancelQuery, _: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel
            .update(cx, |panel, cx| panel.abort_running_query(cx));
    }

    /// Import into the table selected in the sidebar.
    fn on_import_table_data(
        &mut self,
        _: &ImportTableData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar
            .update(cx, |sidebar, cx| match sidebar.selected_table_context(cx) {
                Some((context, table)) => {
                    sidebar.import_table_data(context, Some(table), window, cx)
                }
                None => window.push_notification(
                    (
                        NotificationType::Info,
                        "Select a table in the sidebar to import into.",
                    ),
                    cx,
                ),
            });
    }

    /// Export the table selected in the sidebar.
    fn on_export_table_data(
        &mut self,
        _: &ExportTableData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar
            .update(cx, |sidebar, cx| match sidebar.selected_table_context(cx) {
                Some((context, table)) => {
                    sidebar.export_table_data(context, Some(table), window, cx)
                }
                None => window.push_notification(
                    (
                        NotificationType::Info,
                        "Select a table in the sidebar to export.",
                    ),
                    cx,
                ),
            });
    }

    /// Refresh the connection selected in the sidebar, else the active tab's.
    fn on_refresh_connections(
        &mut self,
        _: &RefreshConnections,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connection_id = self
            .sidebar
            .read(cx)
            .selected_connection_id(cx)
            .or_else(|| {
                self.editor_panel
                    .read(cx)
                    .active_context(cx)
                    .map(|context| context.connection_id)
            });
        if let Some(connection_id) = connection_id {
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.refresh_connection(connection_id, cx)
            });
        }
    }

    fn on_import_connections(
        &mut self,
        _: &ImportConnections,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.import_connections(window, cx));
    }

    fn on_open_sqlite_file(
        &mut self,
        _: &OpenSqliteFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open SQLite database".into()),
        });
        cx.spawn_in(window, async move |app, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            app.update_in(cx, |app, window, cx| {
                for path in &paths {
                    app.open_database_file(path, window, cx);
                }
            })
            .log_err();
        })
        .detach();
    }

    fn on_export_connections(
        &mut self,
        _: &ExportConnections,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.export_connections(None, window, cx)
        });
    }

    fn on_database_connected(
        &mut self,
        action: &DatabaseConnected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.update(cx, |sidebar, cx| {
            // Mark the connection as connected and refresh the sidebar view
            sidebar.validate_connection_as_connected(action.connection_id, cx);
        });
        self.status_bar.update(cx, |state, cx| {
            state.flash(
                ActivityResult::Ok(format!("connected to {}", action.database_name).into()),
                cx,
            );
        });
    }

    fn on_database_disconnected(
        &mut self,
        action: &DatabaseDisconnected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.mark_connection_disconnected(action.connection_id, cx);
        });
        self.status_bar.update(cx, |state, cx| {
            let result = if action.idle {
                ActivityResult::Ok(
                    format!("closed idle connection {}", action.database_name).into(),
                )
            } else {
                ActivityResult::Err(format!("disconnected from {}", action.database_name).into())
            };
            state.flash(result, cx);
        });
    }

    fn on_create_new_query_tab(
        &mut self,
        action: &CreateNewQueryTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = &action.context;
        let title = match (&context.schema_name, &action.table_name) {
            (None, None) => context.database_name.clone(),
            (Some(schema), None) => format!("{}.{}", context.database_name, schema),
            (Some(schema), Some(table)) => {
                format!("{}.{}.{}", context.database_name, schema, table)
            }
            (None, Some(table)) => table.to_string(),
        };

        // Non-SQL backends (Redis) don't get a SELECT scaffold; a clicked key
        // opens the key inspector instead (handled below).
        let content = if context.dialect().supports_sql() {
            action.table_name.as_ref().map(|table| {
                context
                    .dialect()
                    .select_scaffold(context.schema_name.as_deref(), table, 100)
            })
        } else {
            None
        };

        self.editor_panel.update(cx, |panel, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title,
                    content,
                    db_id: None,
                    last_run_at: None,
                    context: context.clone(),
                },
                cx,
            );
        });

        // The "Inspect Key" action drills the freshly opened tab straight into
        // the key inspector; plain "New Query" leaves the editor tab empty.
        if action.inspect_key
            && let Some(key) = action.table_name.clone()
        {
            self.open_redis_key(
                context.connection_id,
                context.database_name.clone(),
                key,
                window,
                cx,
            );
        }

        cx.notify();
    }

    fn on_create_new_script_tab(
        &mut self,
        action: &CreateNewScriptTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = &action.context;
        let title = match &context.schema_name {
            Some(schema) => format!("{}.{} script", context.database_name, schema),
            None => format!("{} script", context.database_name),
        };

        self.editor_panel.update(cx, |panel, cx| {
            panel.create_and_add_script_tab(
                window,
                TabCreationParams {
                    title,
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context: context.clone(),
                },
                cx,
            );
        });

        cx.notify();
    }

    /// Inspect a Redis key in a background task and route the decoded value to
    /// the (freshly created, now active) query tab's results panel.
    fn open_redis_key(
        &mut self,
        connection_id: i64,
        database_name: String,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(results_panel) = self.editor_panel.read(cx).active_results_panel() else {
            return;
        };
        let db_service = database::DatabaseService::global(cx).clone();

        cx.spawn_in(window, async move |_, cx| {
            use database::DatabaseServiceTrait;
            let connection = match db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await
            {
                Ok(connection) => connection,
                Err(e) => {
                    tracing::error!("Failed to open Redis connection: {e}");
                    return;
                }
            };
            let result = match connection.inspect_key(Some(&database_name), &key).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Failed to inspect Redis key {key}: {e}");
                    return;
                }
            };
            results_panel
                .update_in(cx, |panel, window, cx| {
                    panel.set_key_value_result(result, window, cx);
                })
                .log_err();
        })
        .detach();
    }

    fn on_open_table_structure(
        &mut self,
        action: &OpenTableStructure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_tab = self.editor_panel.update(cx, |panel, cx| {
            panel.create_table_structure_tab(
                TableStructureParams {
                    context: action.context.clone(),
                    table_name: action.table_name.clone(),
                },
                window,
                cx,
            )
        });
        if let Some(tab) = weak_tab.upgrade() {
            tab.update(cx, |tab, cx| tab.reload(window, cx));
        }
        cx.notify();
    }

    fn on_refresh_connection_tree(
        &mut self,
        action: &RefreshConnectionTree,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connection_id = action.connection_id;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.refresh_connection(connection_id, cx);
        });
    }

    fn on_open_object_ddl(
        &mut self,
        action: &OpenObjectDdl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_tab = self.editor_panel.update(cx, |panel, cx| {
            panel.create_object_ddl_tab(
                ObjectDdlParams {
                    kind: action.kind,
                    context: action.context.clone(),
                    object_name: action.object_name.clone(),
                },
                window,
                cx,
            )
        });

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = action.context.connection_id;
        let database_name = action.context.database_name.clone();
        let schema_name = action.context.schema_name.clone();
        let object_name = action.object_name.clone();
        let kind = action.kind;
        let activity = ActivityReporter::global(cx)
            .begin(format!("{}: loading DDL", action.context.connection_name));

        cx.spawn_in(window, async move |_, window| {
            let _activity = activity;
            use database::DatabaseServiceTrait;
            let result = match db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await
            {
                Ok(connection) => {
                    connection
                        .object_ddl(kind, schema_name.as_deref(), &object_name)
                        .await
                }
                Err(e) => Err(e),
            };

            window
                .update(|window, cx| {
                    if let Some(tab) = weak_tab.upgrade() {
                        tab.update(cx, |tab, cx| match result {
                            Ok(ddl) => tab.set_ddl(ddl, cx),
                            Err(e) => tab.set_error(format!("{e:#}"), cx),
                        });
                    }
                    let _ = window;
                })
                .log_err();
        })
        .detach();

        cx.notify();
    }

    fn on_backup_database(
        &mut self,
        action: &BackupDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::backup::open_backup(action.context.clone(), window, cx);
    }

    fn on_restore_database(
        &mut self,
        action: &RestoreDatabase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::backup::open_restore(action.context.clone(), window, cx);
    }

    fn on_open_schema_graph(
        &mut self,
        action: &OpenSchemaGraph,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::editor::schema_graph::SchemaGraphParams;

        let weak_tab = self.editor_panel.update(cx, |panel, cx| {
            panel.create_schema_graph_tab(
                SchemaGraphParams {
                    context: action.context.clone(),
                },
                window,
                cx,
            )
        });

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = action.context.connection_id;
        let database_name = action.context.database_name.clone();
        let schema_name = action.context.schema_name.clone();

        cx.spawn_in(window, async move |_, window| {
            use database::DatabaseServiceTrait;
            let connection_result = db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await;

            match connection_result {
                Ok(connection) => {
                    let tables_result = connection
                        .get_tables(schema_name.as_deref())
                        .await
                        .unwrap_or_default();

                    let mut all_table_info = Vec::new();
                    for table_name in &tables_result {
                        let columns = connection
                            .get_columns_for_table(table_name, schema_name.as_deref())
                            .await
                            .unwrap_or_default();

                        all_table_info.push(blanco_core::connection_trait::TableSchemaInfo {
                            name: table_name.clone(),
                            schema: schema_name.clone().unwrap_or_default(),
                            object_type: "table".to_string(),
                            columns,
                            column_count: 0,
                            referenced_by: Vec::new(),
                        });
                    }

                    window
                        .update(|window, cx| {
                            if let Some(tab) = weak_tab.upgrade() {
                                tab.update(cx, |tab, cx| {
                                    tab.update_with_schema(all_table_info, window, cx);
                                });
                            }
                        })
                        .log_err();
                }
                Err(e) => {
                    window
                        .update(|window, cx| {
                            if let Some(tab) = weak_tab.upgrade() {
                                tab.update(cx, |tab, cx| {
                                    tab.set_error(format!("Failed to connect: {e}"), window, cx);
                                });
                            }
                        })
                        .log_err();
                }
            }
        })
        .detach();

        cx.notify();
    }

    fn on_new_connection_modal(
        &mut self,
        _: &OpenNewConnectionModal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_new_connection_modal(None, None, window, cx);
    }

    /// Open the new connection dialog, filled in from `prefill` when given.
    /// `prefill` has no id, so saving creates a connection. When it saves a
    /// temporary connection, `replaces_temporary` is that connection's id: the
    /// saved one takes its place in the sidebar and takes over its tabs.
    fn open_new_connection_modal(
        &mut self,
        prefill: Option<ConnectionData>,
        replaces_temporary: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Create the modal content outside the builder so we can access it
        let modal_content =
            cx.new(|cx| NewConnectionModal::with_connection_data(window, cx, prefill));
        let content_for_focus = modal_content.clone();

        let sidebar = self.sidebar.clone();
        let editor_panel = self.editor_panel.clone();
        window.open_dialog(cx, move |modal, _window, _cx| {
            let content_clone = modal_content.clone();

            modal
                .title("New Connection")
                .h(gpui::px(700.))
                .w(gpui::px(650.))
                .child(modal_content.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("test-connection")
                                .label("Test Connection")
                                .on_click({
                                    let content = content_clone.clone();
                                    move |_, window, cx| {
                                        content.update(cx, |modal, cx| {
                                            modal.test_connection(window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("cancel").label("Cancel").outline().on_click(
                                    |_, window, cx| {
                                        window.close_dialog(cx);
                                    },
                                ))
                                .child(
                                    DialogAction::new()
                                        .child(Button::new("ok").primary().label("Save")),
                                ),
                        ),
                )
                .on_ok({
                    let content = content_clone;
                    let sidebar = sidebar.clone();
                    let editor_panel = editor_panel.clone();
                    move |_, window, cx| {
                        if let Some(conn_data) = content.read(cx).get_connection_data(cx) {
                            let app_database = AppDatabase::global(cx).clone();
                            let db_service = database::DatabaseService::global(cx).clone();
                            let sidebar = sidebar.clone();
                            let editor_panel = editor_panel.clone();
                            window
                                .spawn(cx, async move |cx| {
                                    let result: anyhow::Result<()> = async {
                                        let connection_id =
                                            app_database.save_connection(&conn_data).await?;
                                        tracing::info!("Connection saved with ID: {connection_id}");
                                        let mut connection_data = conn_data;
                                        connection_data.id = Some(connection_id);
                                        if let Err(error) =
                                            connection_credentials::write_connection(
                                                connection_id,
                                                &connection_data,
                                                cx,
                                            )
                                            .await
                                        {
                                            if let Err(cleanup_error) = app_database
                                                .delete_connection(connection_id)
                                                .await
                                            {
                                                tracing::error!(
                                                    "Failed to remove connection metadata after credential storage failed: {cleanup_error}"
                                                );
                                            }
                                            if let Err(cleanup_error) =
                                                connection_credentials::delete_connection(
                                                    connection_id,
                                                    cx,
                                                )
                                                .await
                                            {
                                                tracing::error!(
                                                    "Failed to remove partially saved credentials: {cleanup_error}"
                                                );
                                            }
                                            return Err(error);
                                        }
                                        if let Some(config) = connection_data.to_connection_config()
                                        {
                                            db_service
                                                .add_connection_config(config.without_secrets())
                                                .await;
                                        }
                                        sidebar.update_in(cx, |panel, _window, cx| {
                                            if let Some(temporary_id) = replaces_temporary {
                                                panel.close_temporary_connection(temporary_id, cx);
                                            }
                                            panel.reload_connections(cx);
                                        })?;
                                        if let Some(temporary_id) = replaces_temporary {
                                            let context = ConnectionContext {
                                                connection_id,
                                                connection_name: connection_data.name.clone(),
                                                db_type: connection_data.db_type,
                                                database_name: String::new(),
                                                schema_name: None,
                                                environment_type: Some(
                                                    connection_data.environment_type,
                                                ),
                                            };
                                            editor_panel.update_in(cx, |panel, window, cx| {
                                                panel.move_tabs_to_connection(
                                                    temporary_id,
                                                    &context,
                                                    window,
                                                    cx,
                                                );
                                            })?;
                                        }
                                        Ok(())
                                    }
                                    .await;

                                    cx.update(|window, cx| match result {
                                        Ok(()) => window.push_notification(
                                            (
                                                NotificationType::Success,
                                                "Connection saved successfully",
                                            ),
                                            cx,
                                        ),
                                        Err(error) => {
                                            tracing::error!("Failed to save connection: {error:#}");
                                            window.push_notification(
                                                (
                                                    NotificationType::Error,
                                                    SharedString::from(format!(
                                                        "Failed to save connection: {error:#}"
                                                    )),
                                                ),
                                                cx,
                                            );
                                        }
                                    })
                                    .log_err();
                                })
                                .detach();
                            true
                        } else {
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    "Please fill in all required fields",
                                ),
                                cx,
                            );
                            false
                        }
                    }
                })
        });

        // Focus the first input field after the modal opens
        content_for_focus
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    /// The sidebar keeps connections without their secrets, so fetch the
    /// stored ones for this connection first. The dialog is not opened when
    /// that fails: saving it with empty secret fields would delete them.
    fn edit_connection_with_secrets(
        &mut self,
        connection_data: ConnectionData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = database::DatabaseService::global(cx).secret_store();
        cx.spawn_in(window, async move |this, cx| {
            let hydrated =
                connection_credentials::hydrate_connection(store.as_ref(), connection_data).await;
            cx.update(|window, cx| match hydrated {
                Ok(connection_data) => {
                    this.update(cx, |app, cx| {
                        app.open_edit_connection_modal(connection_data, window, cx);
                    })
                    .log_err();
                }
                Err(error) => {
                    tracing::error!("Failed to load connection credentials: {error:#}");
                    window.push_notification(
                        (
                            NotificationType::Error,
                            SharedString::from(format!(
                                "Failed to load connection credentials: {error:#}"
                            )),
                        ),
                        cx,
                    );
                }
            })
            .log_err();
        })
        .detach();
    }

    fn open_edit_connection_modal(
        &mut self,
        connection_data: ConnectionData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Create the modal content with existing connection data
        let modal_content = cx.new(|cx| {
            NewConnectionModal::with_connection_data(window, cx, Some(connection_data.clone()))
        });
        let content_for_focus = modal_content.clone();

        let connection_data_clone = connection_data.clone();
        let sidebar = self.sidebar.clone();

        window.open_dialog(cx, move |modal, _window, _cx| {
            let content_clone = modal_content.clone();

            modal
                .title("Edit Connection")
                .h(gpui::px(700.))
                .w(gpui::px(650.))
                .child(modal_content.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("test-connection")
                                .label("Test Connection")
                                .on_click({
                                    let content = content_clone.clone();
                                    move |_, window, cx| {
                                        content.update(cx, |modal, cx| {
                                            modal.test_connection(window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("cancel").label("Cancel").outline().on_click(
                                    |_, window, cx| {
                                        window.close_dialog(cx);
                                    },
                                ))
                                .child(
                                    DialogAction::new()
                                        .child(Button::new("ok").primary().label("Save")),
                                ),
                        ),
                )
                .on_ok({
                    let content = content_clone;
                    let conn_data_ref = connection_data_clone.clone();
                    let sidebar = sidebar.clone();
                    move |_, window, cx| {
                        if let Some(mut conn_data) = content.read(cx).get_connection_data(cx) {
                            let original_id = conn_data_ref.id;
                            conn_data.id = original_id;

                            let app_database = AppDatabase::global(cx).clone();
                            let db_service = database::DatabaseService::global(cx).clone();
                            let sidebar = sidebar.clone();
                            window
                                .spawn(cx, async move |cx| {
                                    let result: anyhow::Result<()> = async {
                                        let connection_id =
                                            app_database.save_connection(&conn_data).await?;
                                        tracing::info!(
                                            "Connection updated with ID: {connection_id}"
                                        );
                                        let mut connection_data = conn_data;
                                        connection_data.id = Some(connection_id);
                                        connection_credentials::write_connection(
                                            connection_id,
                                            &connection_data,
                                            cx,
                                        )
                                        .await?;
                                        if let Some(config) = connection_data.to_connection_config()
                                        {
                                            db_service
                                                .add_connection_config(config.without_secrets())
                                                .await;
                                        }
                                        sidebar.update_in(cx, |panel, _window, cx| {
                                            panel.reload_connections(cx);
                                        })?;
                                        Ok(())
                                    }
                                    .await;

                                    cx.update(|window, cx| match result {
                                        Ok(()) => window.push_notification(
                                            (
                                                NotificationType::Success,
                                                "Connection updated successfully",
                                            ),
                                            cx,
                                        ),
                                        Err(error) => {
                                            tracing::error!(
                                                "Failed to update connection: {error:#}"
                                            );
                                            window.push_notification(
                                                (
                                                    NotificationType::Error,
                                                    SharedString::from(format!(
                                                        "Failed to update connection: {error:#}"
                                                    )),
                                                ),
                                                cx,
                                            );
                                        }
                                    })
                                    .log_err();
                                })
                                .detach();
                            true
                        } else {
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    "Please fill in all required fields",
                                ),
                                cx,
                            );
                            false
                        }
                    }
                })
        });

        // Focus the first input field after the modal opens
        content_for_focus
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    fn on_commit_changes(
        &mut self,
        _: &CommitChanges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Delegate commit to the editor panel (which will forward to results panel)
        self.editor_panel.update(cx, |panel, cx| {
            panel.commit_current_changes(window, cx);
        });
    }

    fn on_rollback_changes(
        &mut self,
        _: &RollbackChanges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Delegate rollback to the editor panel (which will forward to results panel)
        self.editor_panel.update(cx, |panel, cx| {
            panel.rollback_current_changes(window, cx);
        });
    }

    fn on_rename_tab(&mut self, action: &RenameTab, _window: &mut Window, cx: &mut Context<Self>) {
        // Delegate tab renaming to the editor panel
        self.editor_panel.update(cx, |panel, cx| {
            panel.rename_tab(action.tab_index, &action.new_name, cx);
        });
        cx.notify();
    }

    fn on_toggle_render_whitespace(
        &mut self,
        _: &ToggleRenderWhitespace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_value = !AppSettings::global(cx).settings.editor.show_whitespace;
        AppSettings::global_mut(cx).settings.editor.show_whitespace = new_value;

        // Save to database
        let db = AppDatabase::global(cx).clone();
        let new_value_str = new_value.to_string();
        cx.spawn(async move |_, _| {
            if let Err(e) = db
                .save_setting("editor.show_whitespace", &new_value_str, false)
                .await
            {
                tracing::error!("Failed to save show_whitespace setting: {}", e);
            }
        })
        .detach();

        // Update all query tab editors
        self.editor_panel.update(cx, |panel, cx| {
            panel.set_all_editors_show_whitespace(new_value, window, cx);
        });

        cx.notify();
    }

    fn on_toggle_word_wrap(
        &mut self,
        _: &ToggleWordWrap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_value = !AppSettings::global(cx).settings.editor.word_wrap;
        AppSettings::global_mut(cx).settings.editor.word_wrap = new_value;

        // Save to database
        let db = AppDatabase::global(cx).clone();
        let new_value_str = new_value.to_string();
        cx.spawn(async move |_, _| {
            if let Err(e) = db
                .save_setting("editor.word_wrap", &new_value_str, false)
                .await
            {
                tracing::error!("Failed to save word_wrap setting: {}", e);
            }
        })
        .detach();

        // Update all query tab editors
        self.editor_panel.update(cx, |panel, cx| {
            panel.set_all_editors_soft_wrap(new_value, window, cx);
        });

        cx.notify();
    }

    fn on_increase_editor_font_size(
        &mut self,
        _: &IncreaseEditorFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::settings::adjust_editor_font_size(1.0, cx);
        self.save_editor_font_size(cx);
    }

    fn on_decrease_editor_font_size(
        &mut self,
        _: &DecreaseEditorFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::settings::adjust_editor_font_size(-1.0, cx);
        self.save_editor_font_size(cx);
    }

    fn on_reset_editor_font_size(
        &mut self,
        _: &ResetEditorFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::settings::set_editor_font_size(
            crate::settings::EditorSettings::DEFAULT_FONT_SIZE,
            cx,
        );
        self.save_editor_font_size(cx);
    }

    /// Persist the editor font size once the shortcut stops repeating. Each
    /// call replaces the pending save, so held keys write the final size once
    /// and never an older one after a newer one.
    fn save_editor_font_size(&mut self, cx: &mut Context<Self>) {
        let db = AppDatabase::global(cx).clone();
        let font_size = AppSettings::global(cx)
            .settings
            .editor
            .font_size
            .to_string();
        self._save_editor_font_size_task = cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            if let Err(error) = db.save_setting("editor.font_size", &font_size, false).await {
                tracing::error!("Failed to save editor font size: {error}");
            }
        });
    }

    fn on_run_query(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.on_run_query(window, cx);
        });
    }

    fn on_explain_query(&mut self, _: &ExplainQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.on_explain_query(window, cx);
        });
    }

    fn on_connect_to_connection(
        &mut self,
        action: &ConnectToConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connection_id = action.connection_id;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.expand_connection(connection_id, window, cx);
        });
    }

    fn on_toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_palette.update(cx, |palette, cx| {
            palette.toggle(window, cx);
        });
    }
}

impl Focusable for BlancoApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BlancoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .key_context("BlancoApp")
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .on_action(cx.listener(Self::on_open_terminal))
            .on_action(cx.listener(Self::on_create_new_query_tab))
            .on_action(cx.listener(Self::on_refresh_connection_tree))
            .on_action(cx.listener(Self::on_create_new_script_tab))
            .on_action(cx.listener(Self::on_open_table_structure))
            .on_action(cx.listener(Self::on_open_object_ddl))
            .on_action(cx.listener(Self::on_open_schema_graph))
            .on_action(cx.listener(Self::on_backup_database))
            .on_action(cx.listener(Self::on_restore_database))
            .on_action(cx.listener(Self::on_new_connection_modal))
            .on_action(cx.listener(Self::on_new_snippet))
            .on_action(cx.listener(Self::on_commit_changes))
            .on_action(cx.listener(Self::on_rollback_changes))
            .on_action(cx.listener(Self::on_rename_tab))
            .on_action(cx.listener(Self::on_database_connected))
            .on_action(cx.listener(Self::on_database_disconnected))
            .on_action(cx.listener(Self::on_open_snippet_editor))
            .on_action(cx.listener(Self::on_refresh_snippets))
            .on_action(cx.listener(Self::on_activate_editor_tab))
            .on_action(cx.listener(Self::on_close_active_tab))
            .on_action(cx.listener(Self::on_open_sql_file))
            .on_action(cx.listener(Self::on_save_tab))
            .on_action(cx.listener(Self::on_save_tab_as))
            .on_action(cx.listener(Self::on_run_sql_file))
            .on_action(cx.listener(Self::on_switch_theme))
            .on_action(cx.listener(Self::on_toggle_render_whitespace))
            .on_action(cx.listener(Self::on_toggle_word_wrap))
            .on_action(cx.listener(Self::on_increase_editor_font_size))
            .on_action(cx.listener(Self::on_decrease_editor_font_size))
            .on_action(cx.listener(Self::on_reset_editor_font_size))
            .on_action(cx.listener(Self::on_run_query))
            .on_action(cx.listener(Self::on_explain_query))
            .on_action(cx.listener(Self::on_connect_to_connection))
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .on_action(cx.listener(Self::on_import_connections))
            .on_action(cx.listener(Self::on_export_connections))
            .on_action(cx.listener(Self::on_open_sqlite_file))
            .on_action(cx.listener(Self::on_new_query_tab))
            .on_action(cx.listener(Self::on_activate_next_tab))
            .on_action(cx.listener(Self::on_activate_previous_tab))
            .on_action(cx.listener(Self::on_toggle_results))
            .on_action(cx.listener(Self::on_cancel_query))
            .on_action(cx.listener(Self::on_import_table_data))
            .on_action(cx.listener(Self::on_export_table_data))
            .on_action(cx.listener(Self::on_refresh_connections))
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .on_drop(cx.listener(|app, paths: &ExternalPaths, window, cx| {
                for path in paths.paths() {
                    app.open_database_file(path, window, cx);
                }
            }))
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Title bar
            .child(self.render_title_bar(window, cx))
            // Main content area: the cards, inset from the window edges. The
            // vertical insets are kept minimal so the cards sit close under the
            // title bar and just above the status bar; only the sides and the
            // split between the cards get the full gap.
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .overflow_hidden()
                    .px(PANEL_GAP)
                    .pb(px(2.))
                    .pt(px(1.))
                    .child(self.render_sidebar_rail(cx))
                    .child(
                        div().flex_1().min_w_0().h_full().child(
                            h_resizable("main-layout")
                                .with_state(&self.main_resize_state)
                                // The cards draw their own borders, so the handle
                                // only needs to stay draggable; its line would
                                // otherwise float in the gutter and overshoot the
                                // rounded corners.
                                .invisible_handles()
                                // Left side: the sidebar body next to the rail
                                .child(self.render_sidebar_panel(cx))
                                // Main panel
                                .child(
                                    resizable_panel().child(
                                        div()
                                            .size_full()
                                            // The other half of the gap comes from
                                            // the sidebar body. While collapsed
                                            // the rail's margin is the whole gap.
                                            .when(!self.sidebar_collapsed, |this| {
                                                this.pl(PANEL_GAP / 2.)
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_1()
                                                    // Let this shrink below its
                                                    // content width, or the panel
                                                    // grows to fit the widest tab
                                                    // and the tab bar never scrolls.
                                                    .min_w_0()
                                                    .h_full()
                                                    .overflow_hidden()
                                                    // The shell is the darkest
                                                    // surface; this card holds the
                                                    // editor, so it takes the
                                                    // content surface the active
                                                    // tab is painted with.
                                                    .bg(cx.theme().tab_active)
                                                    .rounded(PANEL_RADIUS)
                                                    .border_1()
                                                    .border_color(cx.theme().border)
                                                    .child(self.editor_panel.clone()),
                                            ),
                                    ),
                                ),
                        ),
                    ),
            )
            // Status bar pinned to the bottom, spanning the full window width.
            .child(self.render_status_bar(cx))
            .child(self.command_palette.clone())
    }
}

/// Width over height of `images/blanco.svg` (2304x512).
const LOGO_ASPECT_RATIO: f32 = 4.5;

fn init_menus(cx: &mut App) {
    // User-customizable shortcuts, resolved from settings (defaults overlaid
    // with any user overrides).
    let settings = AppSettings::global(cx).settings.clone();
    cx.bind_keys(crate::keybindings::customizable_key_bindings(&settings));

    // Quit is platform-fixed and not user-configurable.
    cx.bind_keys([
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-q", Quit, None),
        #[cfg(not(target_os = "macos"))]
        gpui::KeyBinding::new("alt-f4", Quit, None),
    ]);

    // Results-table cell editing. "DataTable" is the table's own key context;
    // "CellEditor" wraps the inline cell input, so its bindings win over the
    // table's tab -> SelectNextColumn while an edit is in flight (the input's
    // own tab -> IndentInline binding matches first but is unhandled for
    // single-line inputs, so the keystroke falls through to these).
    cx.bind_keys([
        gpui::KeyBinding::new("enter", StartCellEdit, Some("DataTable")),
        gpui::KeyBinding::new("f2", StartCellEdit, Some("DataTable")),
        gpui::KeyBinding::new("tab", EditNextCell, Some("CellEditor")),
        gpui::KeyBinding::new("shift-tab", EditPrevCell, Some("CellEditor")),
        gpui::KeyBinding::new("secondary-r", ToggleCellExpression, Some("CellEditor")),
        gpui::KeyBinding::new(
            "secondary-r",
            ToggleCellExpression,
            Some("ExpandedCellEditor"),
        ),
    ]);

    // Adding and removing conditions in the result filter bar.
    cx.bind_keys(crate::results_panel::filter_bar::key_bindings());

    // Clipboard shortcuts inside a terminal tab. Plain ctrl-c/ctrl-v must reach
    // the shell (interrupt, literal), so the shifted chords copy and paste.
    // tab/shift-tab are bound to focus navigation on the Root context, which
    // would win over the terminal's raw key handler; re-bind them here so they
    // reach the program (Claude Code cycles modes with shift-tab, TUIs use tab).
    cx.bind_keys([
        gpui::KeyBinding::new(
            "tab",
            blanco_terminal::view::SendKeystroke("tab".into()),
            Some("Terminal"),
        ),
        gpui::KeyBinding::new(
            "shift-tab",
            blanco_terminal::view::SendKeystroke("shift-tab".into()),
            Some("Terminal"),
        ),
        gpui::KeyBinding::new(
            "ctrl-shift-c",
            blanco_terminal::view::Copy,
            Some("Terminal"),
        ),
        gpui::KeyBinding::new(
            "ctrl-shift-v",
            blanco_terminal::view::Paste,
            Some("Terminal"),
        ),
        gpui::KeyBinding::new(
            "shift-insert",
            blanco_terminal::view::Paste,
            Some("Terminal"),
        ),
        gpui::KeyBinding::new(
            "ctrl-shift-a",
            blanco_terminal::view::SelectAll,
            Some("Terminal"),
        ),
        gpui::KeyBinding::new(
            "ctrl-shift-k",
            blanco_terminal::view::ClearScrollback,
            Some("Terminal"),
        ),
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-c", blanco_terminal::view::Copy, Some("Terminal")),
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-v", blanco_terminal::view::Paste, Some("Terminal")),
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new(
            "cmd-k",
            blanco_terminal::view::ClearScrollback,
            Some("Terminal"),
        ),
    ]);

    cx.set_menus(build_menu());

    if !cfg!(target_os = "macos") {
        let menu = build_menu().into_iter().map(|menu| menu.owned()).collect();
        GlobalState::global_mut(cx).set_app_menus(menu);
    }
}

fn build_menu() -> Vec<Menu> {
    // macOS always shows the first menu as the application menu, titled with
    // the app's name, and that is where Settings and Quit belong there.
    let mut file_items = vec![
        MenuItem::action("Open SQL File…", OpenSqlFile),
        MenuItem::action("Save", SaveTab),
        MenuItem::action("Save As…", SaveTabAs),
        MenuItem::action("Run SQL File…", RunSqlFile),
        MenuItem::separator(),
        MenuItem::action("New Connection", OpenNewConnectionModal),
        MenuItem::submenu(Menu {
            name: "Open".into(),
            items: vec![MenuItem::action("SQLite Database…", OpenSqliteFile)],
            disabled: false,
        }),
        MenuItem::action("Import Connections…", ImportConnections),
        MenuItem::action("Export Connections…", ExportConnections),
        MenuItem::action("Toggle Terminal", OpenTerminal),
        MenuItem::action("New Snippet", NewSnippet),
    ];
    let mut menus = Vec::new();
    if cfg!(target_os = "macos") {
        menus.push(Menu {
            name: "Blanco".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit Blanco", Quit),
            ],
            disabled: false,
        });
    } else {
        file_items.extend([
            MenuItem::action("Settings", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]);
    }
    menus.extend([
        Menu {
            name: "File".into(),
            items: file_items,
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", gpui_component::input::Undo),
                MenuItem::action("Redo", gpui_component::input::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", gpui_component::input::Cut),
                MenuItem::action("Copy", gpui_component::input::Copy),
                MenuItem::action("Paste", gpui_component::input::Paste),
                MenuItem::separator(),
                MenuItem::action("Select All", gpui_component::input::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Render Whitespace", ToggleRenderWhitespace),
                MenuItem::action("Word Wrap", ToggleWordWrap),
                MenuItem::separator(),
                MenuItem::action("Increase Editor Font Size", IncreaseEditorFontSize),
                MenuItem::action("Decrease Editor Font Size", DecreaseEditorFontSize),
                MenuItem::action("Reset Editor Font Size", ResetEditorFontSize),
            ],
            disabled: false,
        },
    ]);
    menus
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_harness::FullAppHarness;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    async fn test_sidebar_rail_reopens_and_collapses(cx: &mut TestAppContext) {
        let harness = FullAppHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let app = harness.app;

        cx.update(|window, cx| {
            let handle = app.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            window.dispatch_action(Box::new(ToggleSidebar), cx);
        });
        cx.run_until_parked();
        assert!(app.read_with(&cx, |app, _| app.sidebar_collapsed()));

        app.update(&mut cx, |app, cx| {
            app.select_sidebar_tab(SidebarTab::History, cx)
        });
        cx.run_until_parked();
        app.read_with(&cx, |app, _| {
            assert!(
                !app.sidebar_collapsed(),
                "clicking a rail icon should re-open"
            );
            assert_eq!(app.sidebar_tab(), SidebarTab::History);
        });

        app.update(&mut cx, |app, cx| {
            app.select_sidebar_tab(SidebarTab::History, cx)
        });
        cx.run_until_parked();
        app.read_with(&cx, |app, _| {
            assert!(
                app.sidebar_collapsed(),
                "clicking the active icon should collapse"
            );
            assert_eq!(app.sidebar_tab(), SidebarTab::History);
        });

        app.update(&mut cx, |app, cx| {
            app.select_sidebar_tab(SidebarTab::Snippets, cx)
        });
        cx.run_until_parked();
        app.read_with(&cx, |app, _| {
            assert!(!app.sidebar_collapsed());
            assert_eq!(app.sidebar_tab(), SidebarTab::Snippets);
        });
    }

    #[gpui::test]
    async fn test_editor_font_size_actions_step_clamp_and_reset(cx: &mut TestAppContext) {
        use crate::settings::EditorSettings;

        let harness = FullAppHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let app = harness.app;
        let font_size = |cx: &VisualTestContext| {
            cx.read(|cx| AppSettings::global(cx).settings.editor.font_size)
        };
        let dispatch = |action: Box<dyn gpui::Action>, cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let handle = app.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
                window.dispatch_action(action, cx);
            });
            cx.run_until_parked();
        };

        assert_eq!(font_size(&cx), EditorSettings::DEFAULT_FONT_SIZE);
        dispatch(Box::new(IncreaseEditorFontSize), &mut cx);
        assert_eq!(font_size(&cx), EditorSettings::DEFAULT_FONT_SIZE + 1.0);

        for _ in 0..40 {
            dispatch(Box::new(DecreaseEditorFontSize), &mut cx);
        }
        assert_eq!(font_size(&cx), EditorSettings::MIN_FONT_SIZE);

        dispatch(Box::new(ResetEditorFontSize), &mut cx);
        assert_eq!(font_size(&cx), EditorSettings::DEFAULT_FONT_SIZE);
    }

    fn request_write_confirmation(
        app: &Entity<BlancoApp>,
        sql: &str,
        cx: &mut VisualTestContext,
    ) -> channel::Receiver<WriteAnswer> {
        let (reply, receiver) = channel::bounded(1);
        let confirmation = WriteConfirmation {
            connection_name: "test".into(),
            database_name: "main".into(),
            environment: None,
            language: "sql",
            sql: sql.to_string(),
            rememberable_kinds: vec!["DELETE".to_string()],
        };
        app.update_in(cx, |app, window, cx| {
            app.handle_mcp_request(
                McpRequest::ConfirmWrite {
                    confirmation,
                    reply,
                },
                window,
                cx,
            );
        });
        cx.run_until_parked();
        receiver
    }

    fn dismiss_dialog(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.close_dialog(cx));
        cx.run_until_parked();
    }

    fn has_dialog(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| window.has_active_dialog(cx))
    }

    /// A write confirmation that arrives while Blanco is in the background
    /// posts a system notification, and clicking it brings the window forward.
    #[gpui::test]
    async fn test_mcp_write_confirmation_notifies_when_in_background(cx: &mut TestAppContext) {
        let harness = FullAppHarness::new(cx);
        cx.update(|cx| cx.set_app_identity(crate::APP_ID, "Blanco"));
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let app = harness.app;
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.close_all_dialogs(cx);
            window.activate_window();
        });
        cx.run_until_parked();

        let focused = request_write_confirmation(&app, "DELETE FROM a", &mut cx);
        assert!(
            cx.shown_system_notifications().is_empty(),
            "the dialog is already in front of the user"
        );
        dismiss_dialog(&mut cx);
        assert_eq!(focused.try_recv(), Ok(WriteAnswer::Refuse));

        cx.deactivate_window();
        let _background = request_write_confirmation(&app, "DELETE FROM b", &mut cx);
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].tag, MCP_CONFIRMATION_NOTIFICATION_TAG);
        assert_eq!(shown[0].body, "test / main");
        assert!(shown[0].actions.is_empty());
        assert!(!cx.update(|window, _| window.is_window_active()));

        cx.simulate_system_notification_response(gpui::SystemNotificationResponse {
            tag: MCP_CONFIRMATION_NOTIFICATION_TAG.into(),
            action_id: None,
        });
        cx.run_until_parked();
        assert!(cx.update(|window, _| window.is_window_active()));
        assert!(
            has_dialog(&mut cx),
            "the write is still answered in the dialog"
        );
    }

    #[gpui::test]
    async fn test_mcp_write_confirmations_are_shown_one_at_a_time(cx: &mut TestAppContext) {
        let harness = FullAppHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let app = harness.app;
        // Startup can open a dialog of its own; start from an empty stack.
        cx.run_until_parked();
        cx.update(|window, cx| window.close_all_dialogs(cx));

        let first = request_write_confirmation(&app, "DELETE FROM a", &mut cx);
        let second = request_write_confirmation(&app, "DELETE FROM b", &mut cx);
        let abandoned = request_write_confirmation(&app, "DELETE FROM c", &mut cx);
        drop(abandoned);
        let fourth = request_write_confirmation(&app, "DELETE FROM d", &mut cx);
        assert!(has_dialog(&mut cx));

        // Stacked dialogs would close the newest first.
        dismiss_dialog(&mut cx);
        assert_eq!(first.try_recv(), Ok(WriteAnswer::Refuse));
        assert!(second.try_recv().is_err(), "second is still waiting");
        assert!(has_dialog(&mut cx), "the second request is shown next");

        dismiss_dialog(&mut cx);
        assert_eq!(second.try_recv(), Ok(WriteAnswer::Refuse));

        // The request whose caller gave up while queued is skipped.
        assert!(has_dialog(&mut cx));
        dismiss_dialog(&mut cx);
        assert_eq!(fourth.try_recv(), Ok(WriteAnswer::Refuse));
        assert!(!has_dialog(&mut cx));
    }
}
