//! The server's sessions and running queries, with ways to cancel a query
//! or end a session.

use std::sync::Arc;
use std::time::Duration;

use blanco_core::{ConnectionContext, ServerSession};
use database::DedicatedConnection;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Disableable as _, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
    notification::NotificationType,
    select::{Select, SelectEvent, SelectState},
    table::{Column, DataTable, TableDelegate, TableSelection, TableState},
    v_flex,
};

use crate::app::CopySelection;
use crate::result_ext::ResultExt as _;
use blanco_ui::IconName;

/// The auto refresh choices, `None` for off.
const REFRESH_INTERVALS: [(&str, Option<u64>); 5] = [
    ("Off", None),
    ("Every 2 s", Some(2)),
    ("Every 5 s", Some(5)),
    ("Every 10 s", Some(10)),
    ("Every 30 s", Some(30)),
];

/// Query text shown in a cell is cut here; copying gives all of it.
const SHOWN_QUERY_CHARACTERS: usize = 300;

pub struct ProcessListTab {
    pub title: String,
    pub context: ConnectionContext,
    /// A connection of the tab's own, so listing and killing never wait
    /// behind the query being killed. Opened on the first refresh and again
    /// after a refresh fails.
    connection: Option<Arc<DedicatedConnection>>,
    sessions: Vec<ServerSession>,
    table_state: Entity<TableState<SessionsTableDelegate>>,
    interval_select: Entity<SelectState<Vec<String>>>,
    loading: bool,
    error: Option<String>,
    read_only: bool,
    refreshed_at: Option<chrono::DateTime<chrono::Local>>,
    focus_handle: FocusHandle,
    auto_refresh: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ProcessListTab {
    pub fn new(context: ConnectionContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = format!("Processes: {}", context.connection_name);
        let tab = cx.entity().downgrade();
        let table_state = cx.new(|cx| {
            TableState::new(SessionsTableDelegate::new(tab), window, cx)
                .row_selectable(true)
                .cell_selectable(true)
                .col_selectable(false)
        });
        let labels: Vec<String> = REFRESH_INTERVALS
            .iter()
            .map(|(label, _)| label.to_string())
            .collect();
        let interval_select =
            cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(0)), window, cx));
        let interval_subscription = cx.subscribe_in(
            &interval_select,
            window,
            |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                let SelectEvent::Confirm(Some(label)) = event else {
                    return;
                };
                let seconds = REFRESH_INTERVALS
                    .iter()
                    .find(|(choice, _)| choice == label)
                    .and_then(|(_, seconds)| *seconds);
                this.set_refresh_interval(seconds.map(Duration::from_secs), window, cx);
            },
        );
        Self {
            title,
            context,
            connection: None,
            sessions: Vec::new(),
            table_state,
            interval_select,
            loading: false,
            error: None,
            read_only: false,
            refreshed_at: None,
            focus_handle: cx.focus_handle(),
            auto_refresh: None,
            _subscriptions: vec![interval_subscription],
        }
    }

    fn set_refresh_interval(
        &mut self,
        interval: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Replacing the task drops the previous loop.
        self.auto_refresh = interval.map(|interval| {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(interval).await;
                    let refreshed = this.update_in(cx, |this, window, cx| {
                        if !this.loading {
                            this.refresh(window, cx);
                        }
                    });
                    if refreshed.is_err() {
                        break;
                    }
                }
            })
        });
    }

    /// List the sessions again, opening the tab's connection first when it
    /// has none.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.context.dialect().process_list_query() else {
            self.error = Some(format!(
                "{} has no process list in Blanco",
                self.context.db_type
            ));
            cx.notify();
            return;
        };
        self.loading = true;
        cx.notify();

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database =
            (!self.context.database_name.is_empty()).then(|| self.context.database_name.clone());
        let existing = self.connection.clone();

        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let read_only = db_service
                    .get_connection_config(connection_id)
                    .await
                    .is_some_and(|config| config.read_only);
                let connection = match existing {
                    Some(connection) => connection,
                    None => Arc::new(
                        db_service
                            .open_dedicated_connection(connection_id, database.as_deref())
                            .await?,
                    ),
                };
                let result = connection
                    .execute_query(query, database.as_deref(), None)
                    .await?;
                Ok((connection, result, read_only))
            })
            .await;
            this.update_in(cx, |this, _window, cx| {
                this.loading = false;
                match result {
                    Ok((connection, result, read_only)) => {
                        this.connection = Some(connection);
                        this.read_only = read_only;
                        this.error = None;
                        this.refreshed_at = Some(chrono::Local::now());
                        let sessions = result
                            .rows
                            .iter()
                            .filter_map(|row| ServerSession::from_row(row))
                            .collect();
                        this.set_sessions(sessions, cx);
                    }
                    Err(error) => {
                        // A connection that failed may be gone; the next
                        // refresh opens a new one.
                        this.connection = None;
                        this.error = Some(format!("{error:#}"));
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    /// Show `sessions`, keeping the selection on the same session when it
    /// is still there.
    fn set_sessions(&mut self, sessions: Vec<ServerSession>, cx: &mut Context<Self>) {
        let (selected_row, selected_col) = match self.table_state.read(cx).selection() {
            TableSelection::Row(row) => (Some(row), None),
            TableSelection::Cell(row, col) => (Some(row), Some(col)),
            _ => (None, None),
        };
        let selected_id = selected_row
            .and_then(|row| self.sessions.get(row))
            .map(|session| session.id.clone());
        self.sessions = sessions.clone();
        self.table_state.update(cx, |table, cx| {
            let row =
                selected_id.and_then(|id| sessions.iter().position(|session| session.id == id));
            table.delegate_mut().sessions = sessions;
            match (row, selected_col) {
                (Some(row), Some(col)) => table.set_selection(TableSelection::Cell(row, col), cx),
                (Some(row), None) => table.set_selection(TableSelection::Row(row), cx),
                (None, _) => table.clear_selection(cx),
            }
            table.refresh(cx);
        });
    }

    /// Ask before cancelling session `row`'s query or ending the session,
    /// then run the backend's statement on the tab's connection.
    pub fn confirm_action(
        &mut self,
        row: usize,
        action: SessionAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let Some(session) = self.sessions.get(row).cloned() else {
            return;
        };
        let dialect = self.context.dialect();
        let statement = match action {
            SessionAction::CancelQuery => dialect.cancel_query_statement(&session.id),
            SessionAction::KillSession => dialect.kill_session_statement(&session.id),
        };
        let Some(statement) = statement else {
            return;
        };
        let (title, description, label) = match action {
            SessionAction::CancelQuery => (
                format!("Cancel the query of session {}?", session.id),
                "The query stops with an error and the session stays open.",
                "Cancel Query",
            ),
            SessionAction::KillSession => (
                format!("Kill session {}?", session.id),
                "The session is disconnected and its open transaction is rolled back.",
                "Kill Session",
            ),
        };
        let environment_note = match self.context.environment_type {
            Some(environment) if environment.confirms_writes() => {
                format!(" This is a {environment} connection.")
            }
            _ => String::new(),
        };
        let details: Vec<String> = [
            session.user.as_ref().map(|user| format!("User: {user}")),
            session
                .database
                .as_ref()
                .map(|database| format!("Database: {database}")),
            session
                .query
                .as_ref()
                .map(|query| format!("Query: {}", one_line(query, 200))),
        ]
        .into_iter()
        .flatten()
        .collect();
        let description = format!("{description}{environment_note}");
        let this = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let this = this.clone();
            let statement = statement.clone();
            let session_id = session.id.clone();
            dialog
                .title(title.clone())
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child(description.clone()))
                        .children(
                            details
                                .iter()
                                .map(|detail| div().text_xs().child(detail.clone())),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("session-action-back").label("Back").outline()),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("session-action-confirm")
                                    .danger()
                                    .label(label)
                                    .on_click(move |_, window, cx| {
                                        this.update(cx, |this, cx| {
                                            this.run_action(
                                                action,
                                                session_id.clone(),
                                                statement.clone(),
                                                window,
                                                cx,
                                            );
                                        })
                                        .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    fn run_action(
        &mut self,
        action: SessionAction,
        session_id: String,
        statement: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let database =
            (!self.context.database_name.is_empty()).then(|| self.context.database_name.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                connection
                    .execute_query(&statement, database.as_deref(), None)
                    .await
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                let verb = match action {
                    SessionAction::CancelQuery => "Cancelled the query of session",
                    SessionAction::KillSession => "Killed session",
                };
                let notification = match result {
                    // PostgreSQL's functions answer false, not an error, for
                    // a session that is gone or not the user's to signal.
                    Ok(result) if returned_false(&result) => (
                        NotificationType::Warning,
                        format!(
                            "Session {session_id} was not signalled: it has ended, or the role may not signal it"
                        ),
                    ),
                    Ok(_) => (NotificationType::Success, format!("{verb} {session_id}")),
                    Err(error) => (NotificationType::Error, format!("{error:#}")),
                };
                window.push_notification(notification, cx);
                this.refresh(window, cx);
            })
            .log_err();
        })
        .detach();
    }

    /// The copy shortcut: a cell's full value, or selected rows as TSV with
    /// a header row.
    fn copy_selection(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        let table = self.table_state.read(cx);
        let text = match table.selection() {
            TableSelection::Cell(row, col) => self
                .sessions
                .get(row)
                .map(|session| cell_value(session, col)),
            TableSelection::Row(_) | TableSelection::None => {
                let mut rows: Vec<usize> = table.selected_rows().iter().copied().collect();
                rows.sort_unstable();
                (!rows.is_empty()).then(|| rows_as_tsv(&self.sessions, &rows))
            }
            TableSelection::Column(_) => None,
        };
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}

/// What a session's row menu or the toolbar asks of the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    CancelQuery,
    KillSession,
}

/// Whether a statement's single result was `false`, as PostgreSQL's
/// `pg_cancel_backend` and `pg_terminate_backend` report a session they
/// could not signal.
fn returned_false(result: &blanco_core::QueryResult) -> bool {
    matches!(
        result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|cell| cell.as_deref()),
        Some("f" | "false")
    )
}

const COLUMN_NAMES: [&str; 6] = ["Session", "User", "Database", "State", "Duration", "Query"];

/// A cell's full text, as it is copied.
fn cell_value(session: &ServerSession, col: usize) -> String {
    match col {
        0 => session.id.clone(),
        1 => session.user.clone().unwrap_or_default(),
        2 => session.database.clone().unwrap_or_default(),
        3 => session.state.clone().unwrap_or_default(),
        4 => session.duration.map(format_duration).unwrap_or_default(),
        5 => session.query.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

fn rows_as_tsv(sessions: &[ServerSession], rows: &[usize]) -> String {
    let field = |value: String| value.replace(['\t', '\n', '\r'], " ");
    let mut text = COLUMN_NAMES.join("\t");
    for session in rows.iter().filter_map(|&row| sessions.get(row)) {
        text.push('\n');
        let cells: Vec<String> = (0..COLUMN_NAMES.len())
            .map(|col| field(cell_value(session, col)))
            .collect();
        text.push_str(&cells.join("\t"));
    }
    text
}

/// `12.3 s`, `4m 05s`, `2h 03m`, `3d 04h`.
fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{:.1} s", duration.as_secs_f64()),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        3600..86400 => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d {:02}h", seconds / 86400, seconds % 86400 / 3600),
    }
}

/// `text` on one line, cut after `limit` characters.
fn one_line(text: &str, limit: usize) -> String {
    let mut line: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit + 1)
        .collect();
    if line.chars().count() > limit {
        line = line.chars().take(limit).collect();
        line.push('…');
    }
    line
}

impl Focusable for ProcessListTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProcessListTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let dialect = self.context.dialect();
        let selected = self.table_state.read(cx).selection();
        let selected_row = match selected {
            TableSelection::Row(row) | TableSelection::Cell(row, _) => Some(row),
            _ => None,
        };
        let can_act = !self.read_only && selected_row.is_some();
        let status: SharedString = if let Some(error) = &self.error {
            format!("Error: {error}").into()
        } else if self.loading && self.refreshed_at.is_none() {
            "Loading…".into()
        } else {
            let count = match self.sessions.len() {
                1 => "1 session".to_string(),
                count => format!("{count} sessions"),
            };
            match &self.refreshed_at {
                Some(at) => format!("{count}, refreshed {}", at.format("%H:%M:%S")).into(),
                None => count.into(),
            }
        };
        let action_button = |id: &'static str, label: &'static str, action: SessionAction| {
            Button::new(id)
                .outline()
                .small()
                .label(label)
                .disabled(!can_act)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(row) = selected_row {
                        this.confirm_action(row, action, window, cx);
                    }
                }))
        };

        v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .on_action(cx.listener(Self::copy_selection))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("process-list-refresh")
                            .outline()
                            .small()
                            .icon(IconName::RotateCcw)
                            .label("Refresh")
                            .loading(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .w(px(140.))
                            .child(Select::new(&self.interval_select).small()),
                    )
                    .when(dialect.can_cancel_queries(), |this| {
                        this.child(action_button(
                            "process-list-cancel",
                            "Cancel Query",
                            SessionAction::CancelQuery,
                        ))
                    })
                    .when(dialect.can_kill_sessions(), |this| {
                        this.child(action_button(
                            "process-list-kill",
                            "Kill Session",
                            SessionAction::KillSession,
                        ))
                    })
                    .child(div().flex_1())
                    .when(self.read_only, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Read-only connection"),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(if self.error.is_some() {
                                theme.danger_foreground
                            } else {
                                theme.muted_foreground
                            })
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .child(DataTable::new(&self.table_state).bordered(false)),
            )
    }
}

pub struct SessionsTableDelegate {
    sessions: Vec<ServerSession>,
    columns: Vec<Column>,
    tab: WeakEntity<ProcessListTab>,
}

impl SessionsTableDelegate {
    fn new(tab: WeakEntity<ProcessListTab>) -> Self {
        let widths = [90., 120., 120., 140., 90., 520.];
        let columns = COLUMN_NAMES
            .iter()
            .zip(widths)
            .map(|(name, width)| Column {
                name: (*name).into(),
                width: px(width),
                ..Default::default()
            })
            .collect();
        Self {
            sessions: Vec::new(),
            columns,
            tab,
        }
    }
}

impl TableDelegate for SessionsTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.sessions.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let value = self
            .sessions
            .get(row_ix)
            .map(|session| match col_ix {
                5 => session
                    .query
                    .as_deref()
                    .map(|query| one_line(query, SHOWN_QUERY_CHARACTERS))
                    .unwrap_or_default(),
                col => cell_value(session, col),
            })
            .unwrap_or_default();
        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .overflow_hidden()
            .child(value)
    }

    fn context_menu(
        &mut self,
        (row_ix, col_ix): (usize, usize),
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(session) = self.sessions.get(row_ix).cloned() else {
            return menu;
        };
        let tab = self.tab.clone();
        let (dialect, read_only) = tab
            .upgrade()
            .map(|tab| {
                let tab = tab.read(cx);
                (tab.context.dialect(), tab.read_only)
            })
            .unzip();
        let read_only = read_only.unwrap_or(true);
        let action_item = |label: &'static str, action: SessionAction| {
            let tab = tab.clone();
            PopupMenuItem::new(label)
                .disabled(read_only)
                .on_click(move |_, window, cx| {
                    tab.update(cx, |tab, cx| tab.confirm_action(row_ix, action, window, cx))
                        .log_err();
                })
        };
        let value = cell_value(&session, col_ix);
        let row = rows_as_tsv(std::slice::from_ref(&session), &[0]);
        menu.when(dialect.is_some_and(|d| d.can_cancel_queries()), |menu| {
            menu.item(action_item("Cancel Query", SessionAction::CancelQuery))
        })
        .when(dialect.is_some_and(|d| d.can_kill_sessions()), |menu| {
            menu.item(action_item("Kill Session", SessionAction::KillSession))
        })
        .separator()
        .item(PopupMenuItem::new("Copy Value").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
        }))
        .item(PopupMenuItem::new("Copy Row").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(row.clone()));
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{format_duration, one_line, rows_as_tsv};
    use blanco_core::ServerSession;
    use std::time::Duration;

    #[test]
    fn durations_read_at_a_glance() {
        assert_eq!(format_duration(Duration::from_millis(3250)), "3.2 s");
        assert_eq!(format_duration(Duration::from_secs(245)), "4m 05s");
        assert_eq!(format_duration(Duration::from_secs(7380)), "2h 03m");
        assert_eq!(format_duration(Duration::from_secs(273_600)), "3d 04h");
    }

    #[test]
    fn rows_copy_as_tsv_on_one_line_each() {
        let session = ServerSession {
            id: "7".into(),
            user: Some("app".into()),
            database: None,
            state: Some("active".into()),
            duration: Some(Duration::from_secs(2)),
            query: Some("SELECT 1\n\tFROM t".into()),
        };
        assert_eq!(
            rows_as_tsv(&[session], &[0]),
            "Session\tUser\tDatabase\tState\tDuration\tQuery\n7\tapp\t\tactive\t2.0 s\tSELECT 1  FROM t"
        );
        assert_eq!(one_line("a\n  b", 10), "a b");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }
}

#[cfg(test)]
#[path = "process_list_test.rs"]
mod visual_tests;
