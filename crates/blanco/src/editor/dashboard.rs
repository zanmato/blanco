//! Dashboards: panels of query results from any connections, laid out on a
//! grid and refreshed together. A refresh opens one read only connection
//! per server and database the panels use, runs that server's panels on it
//! one after another, and closes it again, so a dashboard holds no
//! connection between refreshes.

mod connection_picker;
mod layout;
mod panel;
mod parameters;
mod thresholds;
mod variables;

use std::collections::HashMap;
use std::time::Duration;

use app_database::{AppDatabase, DASHBOARD_COLUMNS, DashboardData, DashboardPanelData};
use blanco_core::{ConnectionContext, QueryResult};
use database::DatabaseService;
use futures::StreamExt as _;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global, InteractiveElement as _,
    IntoElement, ParentElement, Render, ScrollHandle, StatefulInteractiveElement as _, Styled,
    Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder as _, px, relative,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::NotificationType,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};

use panel::{DashboardPanel, PanelQuery};
pub(crate) use panel::{PanelKind, PanelView, Thresholds};
pub(crate) use parameters::{DashboardParameters, TimeRange};
use parameters::{DashboardVariable, variables_from_json};

use super::rename_form::RenameTabForm;
use super::{EditorPanel, TabCreationParams, TabType};
use crate::app::{
    CopyAsCSV, CopyAsHTML, CopyAsJSON, CopyAsMarkdown, CopyAsSQL, CopyAsTSV, CopyAsVALUES,
    CopyColumnValues, CopySelection,
};
use crate::result_ext::ResultExt as _;
use crate::status_bar::{ActivityReporter, ActivityResult};
use blanco_ui::IconName;

/// The height of one grid row, which a panel's height counts in.
const ROW_HEIGHT: f32 = 72.;

/// The automatic refresh choices, `None` for only on request. Every refresh
/// connects again, so none is shorter than half a minute.
const REFRESH_INTERVALS: [(&str, Option<u32>); 5] = [
    ("Manual refresh", None),
    ("Every 30 s", Some(30)),
    ("Every minute", Some(60)),
    ("Every 5 min", Some(300)),
    ("Every 15 min", Some(900)),
];

const PANEL_WIDTHS: [(&str, u8); 5] = [
    ("Quarter", 3),
    ("Third", 4),
    ("Half", 6),
    ("Two Thirds", 8),
    ("Full Width", DASHBOARD_COLUMNS),
];

const PANEL_HEIGHTS: [(&str, u8); 4] = [
    ("Small", 2),
    ("Medium", 4),
    ("Large", 6),
    ("Extra Large", 8),
];

/// A query tab that came from a dashboard, or whose result went to one:
/// its results offer to save the result back into that panel, and the view
/// the panel had is applied to the tab's next result.
#[derive(Clone, Debug, PartialEq)]
pub struct DashboardLink {
    pub dashboard_id: i64,
    pub dashboard_name: String,
    /// The panel the tab edits, `None` for a tab opened to add one.
    pub panel: Option<LinkedPanel>,
    /// The dashboard's time range and variable values when the tab was
    /// opened from it, which runs in the tab write into the query.
    pub time_range: TimeRange,
    pub variable_values: Vec<(String, String)>,
}

impl DashboardLink {
    pub fn parameters(&self) -> DashboardParameters {
        DashboardParameters::new(self.time_range, self.variable_values.clone())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkedPanel {
    pub id: i64,
    pub title: String,
    pub width: u8,
    pub height: u8,
    /// The panel's view, which a stat keeps when it is saved back from the
    /// table a query tab shows it as.
    pub view: PanelView,
}

/// A save to a dashboard waiting for the tab's query to run first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PendingPanelSave {
    /// Into the panel the tab is linked to.
    Update,
    /// As a new panel of this dashboard, or of a new one.
    Add(Option<i64>),
}

/// The dashboards there are, for the menus that list them. Read again by
/// [`DashboardDirectory::reload`] whenever one is made, renamed or deleted.
#[derive(Default)]
pub struct DashboardDirectory {
    pub dashboards: Vec<DashboardData>,
}

impl Global for DashboardDirectory {}

impl DashboardDirectory {
    pub fn dashboards(cx: &App) -> &[DashboardData] {
        cx.try_global::<Self>()
            .map(|directory| directory.dashboards.as_slice())
            .unwrap_or_default()
    }

    pub fn reload(cx: &mut App) {
        let Some(database) = cx.try_global::<AppDatabase>().cloned() else {
            return;
        };
        cx.spawn(async move |cx| {
            let dashboards = database.load_dashboards().await;
            cx.update(|cx| match dashboards {
                Ok(dashboards) => {
                    cx.set_global(DashboardDirectory { dashboards });
                    cx.refresh_windows();
                }
                Err(error) => tracing::error!("Failed to list the dashboards: {error}"),
            });
        })
        .detach();
    }
}

/// One panel's query, run, and what came of it.
struct PanelOutcome {
    panel_id: i64,
    result: Result<QueryResult, String>,
}

/// The panels that run on one server and database.
/// Resolves once the dashboard tab is gone. A query with no timeout set
/// stops then, so it does not hold its connection after the tab.
type TabClosed = futures::future::Shared<futures::channel::oneshot::Receiver<()>>;

/// `run` ended by the query timeout, or by the tab closing when no timeout
/// is set.
async fn bounded_run<T>(
    run: impl std::future::Future<Output = anyhow::Result<T>>,
    timeout: Option<Duration>,
    closed: TabClosed,
) -> anyhow::Result<T> {
    let timed = async move {
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, run)
                .await
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "The query exceeded the configured {}s timeout",
                        timeout.as_secs()
                    ))
                }),
            None => run.await,
        }
    };
    futures::pin_mut!(timed);
    match futures::future::select(timed, closed).await {
        futures::future::Either::Left((result, _)) => result,
        futures::future::Either::Right(_) => Err(anyhow::anyhow!("The dashboard was closed")),
    }
}

struct ServerGroup {
    connection_id: i64,
    connection_name: String,
    database_name: String,
    queries: Vec<PanelQuery>,
}

pub struct DashboardTab {
    pub dashboard_id: i64,
    /// The editor showing the tab, which opens the query tabs of its panels.
    editor: WeakEntity<EditorPanel>,
    pub title: String,
    dashboard: Option<DashboardData>,
    panels: Vec<Entity<DashboardPanel>>,
    interval_select: Entity<SelectState<Vec<String>>>,
    time_range: TimeRange,
    time_select: Entity<SelectState<Vec<String>>>,
    variables: Vec<DashboardVariable>,
    /// Each variable's choices, by name: its values, or its query's rows.
    variable_options: HashMap<String, Result<Vec<String>, String>>,
    variable_selects: Vec<(String, Entity<SelectState<Vec<String>>>)>,
    variable_subscriptions: Vec<Subscription>,
    loading: bool,
    load_error: Option<String>,
    refreshing: bool,
    /// The automatic refresh came due while the tab was not shown. The next
    /// time it is drawn it refreshes, so a hidden dashboard queries nothing.
    refresh_due: bool,
    refreshed_at: Option<chrono::DateTime<chrono::Local>>,
    auto_refresh: Option<Task<()>>,
    refresh_task: Option<Task<()>>,
    /// The panel being sized by its corner, and its size when that began.
    resize_start: Option<layout::ResizeStart>,
    scroll_handle: ScrollHandle,
    focus_handle: FocusHandle,
    /// Dropped with the tab, which resolves `closed`.
    _open: futures::channel::oneshot::Sender<()>,
    closed: TabClosed,
    _subscriptions: Vec<Subscription>,
}

impl DashboardTab {
    pub fn new(
        dashboard_id: i64,
        editor: WeakEntity<EditorPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
                this.set_refresh_interval(seconds, true, window, cx);
            },
        );
        let time_labels: Vec<String> = TimeRange::ALL
            .iter()
            .map(|range| range.label().to_string())
            .collect();
        let default_range = TimeRange::ALL
            .iter()
            .position(|range| *range == TimeRange::default());
        let time_select = cx
            .new(|cx| SelectState::new(time_labels, default_range.map(IndexPath::new), window, cx));
        let time_subscription = cx.subscribe_in(
            &time_select,
            window,
            |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                let SelectEvent::Confirm(Some(label)) = event else {
                    return;
                };
                let Some(range) = TimeRange::ALL
                    .into_iter()
                    .find(|range| range.label() == label)
                else {
                    return;
                };
                this.time_range = range;
                if let Some(dashboard) = &mut this.dashboard {
                    dashboard.time_range = range.key();
                }
                this.save_dashboard(cx);
                this.refresh(window, cx);
            },
        );
        let (open, closed) = futures::channel::oneshot::channel();
        let mut this = Self {
            dashboard_id,
            editor,
            title: "Dashboard".to_string(),
            dashboard: None,
            panels: Vec::new(),
            interval_select,
            time_range: TimeRange::default(),
            time_select,
            variables: Vec::new(),
            variable_options: HashMap::new(),
            variable_selects: Vec::new(),
            variable_subscriptions: Vec::new(),
            loading: true,
            load_error: None,
            refreshing: false,
            refresh_due: false,
            refreshed_at: None,
            auto_refresh: None,
            refresh_task: None,
            resize_start: None,
            scroll_handle: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _open: open,
            closed: futures::FutureExt::shared(closed),
            _subscriptions: vec![
                interval_subscription,
                time_subscription,
                // A rename made elsewhere, such as in the sidebar's list, or
                // the dashboard deleted.
                cx.observe_global_in::<DashboardDirectory>(window, |this, window, cx| {
                    let listed = DashboardDirectory::dashboards(cx)
                        .iter()
                        .find(|dashboard| dashboard.id == this.dashboard_id)
                        .map(|dashboard| dashboard.name.clone());
                    match listed {
                        Some(name) => {
                            if name != this.title {
                                if let Some(dashboard) = &mut this.dashboard {
                                    dashboard.name = name.clone();
                                }
                                this.title = name;
                                cx.notify();
                            }
                        }
                        // Missing from a list read before it was made, or
                        // deleted: the database tells which.
                        None => {
                            let dashboard_id = this.dashboard_id;
                            let editor = this.editor.clone();
                            let database = AppDatabase::global(cx).clone();
                            cx.spawn_in(window, async move |_, cx| {
                                if !matches!(database.load_dashboard(dashboard_id).await, Ok(None))
                                {
                                    return;
                                }
                                editor
                                    .update_in(cx, |editor, window, cx| {
                                        editor.close_dashboard_tabs(dashboard_id, window, cx)
                                    })
                                    .log_err();
                            })
                            .detach();
                        }
                    }
                }),
            ],
        };
        this.load(window, cx);
        this
    }

    /// Read the dashboard and its panels again, then refresh. Panels already
    /// shown keep their result until the refresh replaces it.
    pub fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let database = AppDatabase::global(cx).clone();
        let dashboard_id = self.dashboard_id;
        cx.spawn_in(window, async move |this, cx| {
            let loaded = async {
                let dashboard = database.load_dashboard(dashboard_id).await?;
                let panels = database.load_dashboard_panels(dashboard_id).await?;
                Ok::<_, sqlx::Error>((dashboard, panels))
            }
            .await;
            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match loaded {
                    Ok((Some(dashboard), panels)) => {
                        this.load_error = None;
                        this.show_dashboard(dashboard, panels, window, cx);
                        // Refreshed once shown and its variables have their
                        // choices, so a dashboard restored behind another
                        // tab runs nothing until it is opened.
                        this.resolve_variables(window, cx);
                    }
                    Ok((None, _)) => {
                        this.load_error = Some("This dashboard was deleted.".to_string());
                    }
                    Err(error) => {
                        this.load_error = Some(format!("The dashboard did not load: {error}"));
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn show_dashboard(
        &mut self,
        dashboard: DashboardData,
        panels: Vec<DashboardPanelData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = dashboard.name.clone();
        let interval_ix = REFRESH_INTERVALS
            .iter()
            .position(|(_, seconds)| *seconds == dashboard.refresh_seconds)
            .unwrap_or(0);
        self.interval_select.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::new(interval_ix)), window, cx);
        });
        let refresh_seconds = dashboard.refresh_seconds;
        self.time_range = TimeRange::from_key(&dashboard.time_range);
        let range_ix = TimeRange::ALL
            .iter()
            .position(|range| *range == self.time_range);
        self.time_select.update(cx, |select, cx| {
            select.set_selected_index(range_ix.map(IndexPath::new), window, cx);
        });
        self.variables = variables_from_json(&dashboard.variables);
        self.dashboard = Some(dashboard);
        self.set_refresh_interval(refresh_seconds, false, window, cx);

        let mut existing: HashMap<i64, Entity<DashboardPanel>> = self
            .panels
            .drain(..)
            .map(|panel| (panel.read(cx).data.id, panel))
            .collect();
        self.panels = panels
            .into_iter()
            .map(|data| match existing.remove(&data.id) {
                Some(panel) => {
                    panel.update(cx, |panel, cx| {
                        let view = PanelView::from_json(&data.view);
                        panel.data = data;
                        panel.set_view(view, window, cx);
                    });
                    panel
                }
                None => cx.new(|cx| DashboardPanel::new(data, window, cx)),
            })
            .collect();
        cx.notify();
    }

    /// Refresh every `seconds`, or only on request for `None`. `save` keeps
    /// the choice with the dashboard.
    fn set_refresh_interval(
        &mut self,
        seconds: Option<u32>,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Replacing the task drops the previous loop.
        self.auto_refresh = seconds.map(|seconds| {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(u64::from(seconds)))
                        .await;
                    let marked = this.update(cx, |this, cx| {
                        this.refresh_due = true;
                        cx.notify();
                    });
                    if marked.is_err() {
                        break;
                    }
                }
            })
        });
        if save && let Some(dashboard) = &mut self.dashboard {
            dashboard.refresh_seconds = seconds;
            self.save_dashboard(cx);
        }
    }

    fn save_dashboard(&self, cx: &mut Context<Self>) {
        let Some(dashboard) = self.dashboard.clone() else {
            return;
        };
        let database = AppDatabase::global(cx).clone();
        cx.spawn(async move |_, cx| {
            let saved = database.update_dashboard(&dashboard).await;
            cx.update(|cx| match saved {
                Ok(()) => DashboardDirectory::reload(cx),
                Err(error) => tracing::error!("Failed to save the dashboard: {error}"),
            });
        })
        .detach();
    }

    /// Run every panel's query. Does nothing while a refresh is running.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A time range or variable picked meanwhile must not be lost: the
        // running refresh still has the old values, so another follows it.
        if self.refreshing {
            self.refresh_due = true;
            return;
        }
        self.refresh_due = false;
        let parameters = self.parameters();
        let queries: Vec<PanelQuery> = self
            .panels
            .iter()
            .filter_map(|panel| panel.read(cx).query())
            .map(|mut query| {
                query.query = parameters.apply(&query.query, query.db_type.dialect());
                query
            })
            .collect();
        if queries.is_empty() {
            return;
        }
        for panel in &self.panels {
            if panel.read(cx).query().is_some() {
                panel.update(cx, |panel, cx| panel.set_loading(cx));
            }
        }
        self.refreshing = true;
        cx.notify();

        let service = DatabaseService::global(cx).clone();
        let timeout = variables::query_timeout(cx);
        let activity = ActivityReporter::global(cx);
        let dashboard_name = self.title.clone();
        let total = queries.len();
        let refreshing = activity.begin(format!(
            "{dashboard_name}: refreshing {}",
            panels_text(total)
        ));
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let closed = self.closed.clone();
        // Detached: a refresh always runs to its end, bounded by the query
        // timeout or the tab closing, so its connections are closed even
        // when the tab is.
        gpui_tokio::Tokio::handle(cx).spawn({
            let dashboard_name = dashboard_name.clone();
            async move {
                let groups = server_groups(queries);
                futures::future::join_all(groups.into_iter().map(|group| {
                    run_server_group(
                        service.clone(),
                        group,
                        timeout,
                        closed.clone(),
                        sender.clone(),
                        activity.clone(),
                        dashboard_name.clone(),
                    )
                }))
                .await;
            }
        });
        self.refresh_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut failed = 0;
            while let Some(outcome) = receiver.next().await {
                if outcome.result.is_err() {
                    failed += 1;
                }
                let shown = this.update_in(cx, |this, window, cx| {
                    this.show_outcome(outcome, window, cx);
                });
                if shown.is_err() {
                    return;
                }
            }
            refreshing.finish(if failed == 0 {
                ActivityResult::Ok(
                    format!("{dashboard_name}: refreshed {}", panels_text(total)).into(),
                )
            } else {
                ActivityResult::Err(
                    format!(
                        "{dashboard_name}: {failed} of {} failed",
                        panels_text(total)
                    )
                    .into(),
                )
            });
            this.update(cx, |this, cx| {
                this.refreshing = false;
                this.refreshed_at = Some(chrono::Local::now());
                cx.notify();
            })
            .log_err();
        }));
    }

    fn show_outcome(&mut self, outcome: PanelOutcome, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self
            .panels
            .iter()
            .find(|panel| panel.read(cx).data.id == outcome.panel_id)
            .cloned()
        else {
            return;
        };
        panel.update(cx, |panel, cx| match outcome.result {
            Ok(result) => panel.set_result(result, window, cx),
            Err(error) => panel.set_error(error, cx),
        });
    }

    /// Loaded, refreshed and every panel shown.
    #[cfg(feature = "screenshots")]
    pub(crate) fn is_settled(&self, cx: &App) -> bool {
        !self.loading
            && !self.refreshing
            && self.refreshed_at.is_some()
            && self.panels.iter().all(|panel| !panel.read(cx).is_loading())
    }

    fn panel_index(&self, panel_id: i64, cx: &App) -> Option<usize> {
        self.panels
            .iter()
            .position(|panel| panel.read(cx).data.id == panel_id)
    }

    fn save_panel(&self, panel_id: i64, cx: &mut Context<Self>) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        let data = self.panels[ix].read(cx).data.clone();
        let database = AppDatabase::global(cx).clone();
        cx.background_spawn(async move {
            if let Err(error) = database.update_dashboard_panel(&data).await {
                tracing::error!("Failed to save the dashboard panel: {error}");
            }
        })
        .detach();
    }

    fn set_panel_kind(
        &mut self,
        panel_id: i64,
        kind: PanelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        self.panels[ix].update(cx, |panel, cx| {
            let view = panel.view.with_kind(kind);
            panel.set_view(view, window, cx);
        });
        self.save_panel(panel_id, cx);
        cx.notify();
    }

    fn set_panel_size(
        &mut self,
        panel_id: i64,
        width: Option<u8>,
        height: Option<u8>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        self.panels[ix].update(cx, |panel, cx| {
            if let Some(width) = width {
                panel.data.width = width;
            }
            if let Some(height) = height {
                panel.data.height = height;
            }
            cx.notify();
        });
        self.save_panel(panel_id, cx);
        cx.notify();
    }

    /// Move a panel `step` places along the layout order.
    /// Move a panel `step` places along the layout order.
    fn move_panel(&mut self, panel_id: i64, step: isize, cx: &mut Context<Self>) {
        let Some(target) = self
            .panel_index(panel_id, cx)
            .and_then(|ix| ix.checked_add_signed(step))
        else {
            return;
        };
        self.move_panel_to(panel_id, target, cx);
    }

    fn remove_panel(&mut self, panel_id: i64, cx: &mut Context<Self>) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        self.panels.remove(ix);
        let database = AppDatabase::global(cx).clone();
        cx.background_spawn(async move {
            if let Err(error) = database.delete_dashboard_panel(panel_id).await {
                tracing::error!("Failed to remove the dashboard panel: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    fn rename_panel(&mut self, panel_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        let current = self.panels[ix].read(cx).data.title.clone();
        let tab = cx.entity().downgrade();
        open_name_dialog(
            "Rename Panel",
            "Rename",
            current,
            window,
            cx,
            move |name, _, cx| {
                tab.update(cx, |tab, cx| {
                    let Some(ix) = tab.panel_index(panel_id, cx) else {
                        return;
                    };
                    tab.panels[ix].update(cx, |panel, cx| {
                        panel.data.title = name;
                        cx.notify();
                    });
                    tab.save_panel(panel_id, cx);
                    cx.notify();
                })
                .log_err();
            },
        );
    }

    /// The time range and the value each variable stands for now: the one
    /// picked, else its first choice.
    fn parameters(&self) -> DashboardParameters {
        let values = self
            .variables
            .iter()
            .filter_map(|variable| {
                let value = self.variable_value(variable)?;
                Some((variable.name.clone(), value))
            })
            .collect();
        DashboardParameters::new(self.time_range, values)
    }

    fn variable_value(&self, variable: &DashboardVariable) -> Option<String> {
        let options = self.variable_options.get(&variable.name)?.as_ref().ok()?;
        variable
            .current
            .clone()
            .filter(|current| options.contains(current))
            .or_else(|| options.first().cloned())
    }

    fn link(&self, panel: Option<&DashboardPanel>) -> DashboardLink {
        let parameters = self.parameters();
        DashboardLink {
            time_range: parameters.time_range,
            variable_values: parameters.values,
            dashboard_id: self.dashboard_id,
            dashboard_name: self.title.clone(),
            panel: panel.map(|panel| LinkedPanel {
                id: panel.data.id,
                title: panel.data.title.clone(),
                width: panel.data.width,
                height: panel.data.height,
                view: panel.view.clone(),
            }),
        }
    }

    fn edit_panel(&self, panel_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        let panel = self.panels[ix].read(cx);
        let Some(context) = panel.context() else {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "The panel's connection was deleted, so its query has nowhere to run.",
                ),
                cx,
            );
            return;
        };
        let link = self.link(Some(panel));
        let sql = panel.data.query.clone();
        let editor = self.editor.clone();
        window.defer(cx, move |window, cx| {
            editor
                .update(cx, |editor, cx| {
                    editor.edit_dashboard_panel(link, context, sql, window, cx)
                })
                .log_err();
        });
    }

    /// Ask which connection the new panel runs on, then open a query tab on
    /// it whose result can become the panel.
    fn add_panel(&self, window: &mut Window, cx: &mut Context<Self>) {
        let database = AppDatabase::global(cx).clone();
        cx.spawn_in(window, async move |this, cx| {
            let connections = database.load_connections().await;
            this.update_in(cx, |this, window, cx| match connections {
                Ok(connections) => this.open_add_panel_dialog(connections, window, cx),
                Err(error) => window.push_notification(
                    (
                        NotificationType::Error,
                        format!("The connections did not load: {error}"),
                    ),
                    cx,
                ),
            })
            .log_err();
        })
        .detach();
    }

    fn open_add_panel_dialog(
        &mut self,
        connections: Vec<app_database::ConnectionData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The dashboard's default connection comes first, else the one most
        // of its panels use, opened on the database a panel there runs on.
        let contexts: Vec<ConnectionContext> = self
            .panels
            .iter()
            .filter_map(|panel| panel.read(cx).context())
            .collect();
        let most_used = contexts
            .iter()
            .max_by_key(|context| {
                contexts
                    .iter()
                    .filter(|other| other.connection_id == context.connection_id)
                    .count()
            })
            .map(|context| context.connection_id);
        let preferred_id = self
            .dashboard
            .as_ref()
            .and_then(|dashboard| dashboard.default_connection_id)
            .or(most_used);
        let preferred = preferred_id.and_then(|id| {
            contexts
                .iter()
                .find(|context| context.connection_id == id)
                .cloned()
                .or_else(|| {
                    connections
                        .iter()
                        .find(|connection| connection.id == Some(id))
                        .and_then(connection_context)
                })
        });
        let picker = cx
            .new(|cx| connection_picker::ConnectionPicker::new(connections, preferred, window, cx));
        if picker.read(cx).is_empty() {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "Add a connection first: a panel runs its query on one.",
                ),
                cx,
            );
            return;
        }
        let tab = cx.entity().downgrade();
        let take_pick = {
            let picker = picker.downgrade();
            move |window: &mut Window, cx: &mut App| -> bool {
                let Some(context) = picker
                    .upgrade()
                    .and_then(|picker| picker.read(cx).selected())
                else {
                    window.push_notification(
                        (NotificationType::Warning, "Pick the connection to run on."),
                        cx,
                    );
                    return false;
                };
                tab.update(cx, |tab, cx| tab.add_panel_on(context, window, cx))
                    .log_err();
                true
            }
        };
        let take_pick = std::rc::Rc::new(take_pick);
        picker.update(cx, |picker, _| {
            let take_pick = take_pick.clone();
            picker.on_confirm(move |window, cx| {
                if take_pick(window, cx) {
                    window.close_dialog(cx);
                }
            });
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let take_pick = take_pick.clone();
            dialog
                .title("Add Panel")
                .w(px(420.))
                .child(picker.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("ok").primary().label("Write Query")),
                        ),
                )
                .on_ok(move |_, window, cx| take_pick(window, cx))
        });
    }

    /// Open a query tab on `context` whose result can become a panel, and
    /// make its connection the dashboard's default.
    fn add_panel_on(
        &mut self,
        context: ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dashboard) = &mut self.dashboard
            && dashboard.default_connection_id != Some(context.connection_id)
        {
            dashboard.default_connection_id = Some(context.connection_id);
            self.save_dashboard(cx);
        }
        let link = self.link(None);
        let editor = self.editor.clone();
        window.defer(cx, move |window, cx| {
            editor
                .update(cx, |editor, cx| {
                    editor.open_tab_for_new_panel(link, context, window, cx)
                })
                .log_err();
        });
    }

    fn copy_from_panel(
        &self,
        panel: &Entity<DashboardPanel>,
        format: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        panel.update(cx, |panel, cx| panel.copy(format, window, cx));
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let status = if self.refreshing {
            "Refreshing…".to_string()
        } else if let Some(at) = self.refreshed_at {
            format!("Refreshed {}", at.format("%H:%M:%S"))
        } else {
            String::new()
        };
        h_flex()
            .w_full()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .truncate()
                    .child(self.title.clone()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .whitespace_nowrap()
                    .child(status),
            )
            .child(
                div()
                    .id("dashboard-time-range")
                    .w(px(140.))
                    .child(Select::new(&self.time_select).small())
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new(
                            "Panels filter on it with $__timeFilter(column), in UTC",
                        )
                        .build(window, cx)
                    }),
            )
            .child(
                div()
                    .w(px(150.))
                    .child(Select::new(&self.interval_select).small()),
            )
            .child(
                Button::new("dashboard-refresh")
                    .small()
                    .ghost()
                    .icon(IconName::RotateCcw)
                    .tooltip("Refresh")
                    .disabled(self.refreshing)
                    .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
            )
            .child(
                Button::new("dashboard-add-panel")
                    .small()
                    .primary()
                    .icon(IconName::Plus)
                    .label("Add Panel")
                    .on_click(cx.listener(|this, _, window, cx| this.add_panel(window, cx))),
            )
            .child({
                let tab = cx.entity().downgrade();
                Button::new("dashboard-menu")
                    .small()
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .dropdown_menu(move |menu, _, _| {
                        let delete = tab.clone();
                        let add_variable = tab.clone();
                        menu.item(PopupMenuItem::new("Add Variable…").on_click(
                            move |_, window, cx| {
                                add_variable
                                    .update(cx, |tab, cx| {
                                        tab.open_variable_dialog(None, window, cx)
                                    })
                                    .log_err();
                            },
                        ))
                        .separator()
                        .item(
                            PopupMenuItem::new("Delete Dashboard…").on_click(
                                move |_, window, cx| {
                                    if let Some(tab) = delete.upgrade() {
                                        let (id, name) = {
                                            let tab = tab.read(cx);
                                            (tab.dashboard_id, tab.title.clone())
                                        };
                                        confirm_delete_dashboard(id, name, window, cx);
                                    }
                                },
                            ),
                        )
                    })
            })
    }

    fn render_panel(
        &self,
        ix: usize,
        panel: &Entity<DashboardPanel>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let data = &panel.read(cx).data;
        let panel_id = data.id;
        let width = data.width;
        let height = data.height;
        let title = data.title.clone();
        let read = panel.read(cx);
        let kind = read.view.kind();
        let failed = read.error().is_some();
        let status = if read.is_loading() {
            Some("Running…".to_string())
        } else if kind == PanelKind::Stat {
            None
        } else {
            read.row_count(cx).map(|rows| match rows {
                1 => "1 row".to_string(),
                rows => format!("{rows} rows"),
            })
        };
        let connection_name = read
            .data
            .connection
            .as_ref()
            .map(|connection| connection.name.clone());
        let last = self.panels.len().saturating_sub(1);
        let tab = cx.entity().downgrade();
        let menu_panel = panel.clone();

        let dragged = layout::DraggedPanel {
            panel_id,
            title: data.title.clone().into(),
        };
        let header = h_flex()
            .id(("dashboard-panel-header", panel_id as usize))
            .w_full()
            .flex_shrink_0()
            .cursor_grab()
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            // A double click opens the query, as Edit Query does.
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if event.click_count() == 2 {
                        this.edit_panel(panel_id, window, cx);
                    }
                }),
            )
            .pl_3()
            .pr_1()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .truncate()
                    .child(title),
            )
            .when(failed, |this| {
                this.child(
                    gpui_component::Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(theme.danger),
                )
            })
            .when_some(status, |this, status| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .whitespace_nowrap()
                        .child(status),
                )
            })
            .child(
                Button::new(("dashboard-panel-menu", ix))
                    .xsmall()
                    .ghost()
                    .icon(IconName::EllipsisVertical)
                    .when_some(connection_name, |button, name| {
                        button.tooltip(format!("Runs on {name}"))
                    })
                    .dropdown_menu(move |menu, window, cx| {
                        panel_menu(
                            menu,
                            tab.clone(),
                            &menu_panel,
                            panel_id,
                            kind,
                            width,
                            height,
                            ix,
                            last,
                            window,
                            cx,
                        )
                    }),
            );

        let column_panel = panel.clone();
        // The grid's copy commands reach the app only when a query tab is
        // active, so each panel answers them for its own grid.
        macro_rules! copy_as {
            ($action:ty, $format:expr) => {{
                let panel = panel.clone();
                cx.listener(move |this: &mut Self, _: &$action, window, cx| {
                    this.copy_from_panel(&panel, $format, window, cx)
                })
            }};
        }

        div()
            .id(("dashboard-panel", panel_id as usize))
            .w(relative(f32::from(width) / f32::from(DASHBOARD_COLUMNS)))
            .h(px(f32::from(height) * ROW_HEIGHT))
            .p_1()
            .drag_over::<layout::DraggedPanel>(|style, _, _, cx| {
                style.bg(cx.theme().drag_border.opacity(0.25))
            })
            .on_drop(
                cx.listener(move |this, dragged: &layout::DraggedPanel, _, cx| {
                    this.move_panel_to(dragged.panel_id, ix, cx);
                }),
            )
            .child(
                v_flex()
                    .relative()
                    .size_full()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
                    .bg(theme.background)
                    .overflow_hidden()
                    .child(header)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .on_action(copy_as!(CopySelection, None))
                            .on_action(copy_as!(CopyAsCSV, Some("csv")))
                            .on_action(copy_as!(CopyAsTSV, Some("tsv")))
                            .on_action(copy_as!(CopyAsJSON, Some("json")))
                            .on_action(copy_as!(CopyAsSQL, Some("sql")))
                            .on_action(copy_as!(CopyAsVALUES, Some("values")))
                            .on_action(copy_as!(CopyAsMarkdown, Some("markdown")))
                            .on_action(copy_as!(CopyAsHTML, Some("html")))
                            .on_action(cx.listener(move |_, action: &CopyColumnValues, _, cx| {
                                let text = column_panel.read(cx).column_text(
                                    action.column,
                                    action.distinct,
                                    action.as_in_list,
                                    cx,
                                );
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                            }))
                            .child(panel.clone()),
                    )
                    .child(
                        div()
                            .id(("dashboard-panel-resize", panel_id as usize))
                            .debug_selector(|| "dashboard-panel-resize".to_string())
                            .absolute()
                            .right_0()
                            .bottom_0()
                            .size(px(14.))
                            .cursor(gpui::CursorStyle::ResizeUpLeftDownRight)
                            .text_color(theme.muted_foreground)
                            .child(gpui_component::Icon::new(IconName::ResizeCorner).size(px(12.)))
                            // The resize starts with the drag, not the mouse
                            // down: a redraw between the two would end it.
                            .on_drag(layout::ResizingPanel { panel_id }, {
                                let tab = cx.entity().downgrade();
                                move |dragged, _, window, cx| {
                                    let position = window.mouse_position();
                                    tab.update(cx, |tab, cx| {
                                        tab.start_resize(dragged.panel_id, position, cx)
                                    })
                                    .log_err();
                                    cx.new(|_| dragged.clone())
                                }
                            }),
                    ),
            )
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .text_color(theme.muted_foreground)
            .child(div().text_base().child("This dashboard has no panels yet"))
            .child(div().text_sm().text_center().max_w(px(460.)).child(
                "Run a query and pick this dashboard under Add to Dashboard in its \
                         results, or add a panel here.",
            ))
            .child(
                Button::new("dashboard-empty-add")
                    .small()
                    .icon(IconName::Plus)
                    .label("Add Panel")
                    .on_click(cx.listener(|this, _, window, cx| this.add_panel(window, cx))),
            )
    }
}

/// The menu of one panel.
#[allow(clippy::too_many_arguments)]
fn panel_menu(
    menu: PopupMenu,
    tab: gpui::WeakEntity<DashboardTab>,
    panel: &Entity<DashboardPanel>,
    panel_id: i64,
    kind: PanelKind,
    width: u8,
    height: u8,
    ix: usize,
    last: usize,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let on = |tab: &gpui::WeakEntity<DashboardTab>,
              f: fn(&mut DashboardTab, i64, &mut Window, &mut Context<DashboardTab>)| {
        let tab = tab.clone();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
            tab.update(cx, |tab, cx| f(tab, panel_id, window, cx))
                .log_err();
        }
    };
    let show_as = PopupMenu::build(window, cx, {
        let tab = tab.clone();
        move |menu, _, _| {
            PanelKind::all().into_iter().fold(menu, |menu, choice| {
                let tab = tab.clone();
                menu.item(
                    PopupMenuItem::new(choice.label())
                        .checked(choice == kind)
                        .on_click(move |_, window, cx| {
                            tab.update(cx, |tab, cx| {
                                tab.set_panel_kind(panel_id, choice, window, cx)
                            })
                            .log_err();
                        }),
                )
            })
        }
    });
    let widths = PopupMenu::build(window, cx, {
        let tab = tab.clone();
        move |menu, _, _| {
            PANEL_WIDTHS
                .into_iter()
                .fold(menu, |menu, (label, choice)| {
                    let tab = tab.clone();
                    menu.item(PopupMenuItem::new(label).checked(choice == width).on_click(
                        move |_, _, cx| {
                            tab.update(cx, |tab, cx| {
                                tab.set_panel_size(panel_id, Some(choice), None, cx)
                            })
                            .log_err();
                        },
                    ))
                })
        }
    });
    let heights = PopupMenu::build(window, cx, {
        let tab = tab.clone();
        move |menu, _, _| {
            PANEL_HEIGHTS
                .into_iter()
                .fold(menu, |menu, (label, choice)| {
                    let tab = tab.clone();
                    menu.item(
                        PopupMenuItem::new(label)
                            .checked(choice == height)
                            .on_click(move |_, _, cx| {
                                tab.update(cx, |tab, cx| {
                                    tab.set_panel_size(panel_id, None, Some(choice), cx)
                                })
                                .log_err();
                            }),
                    )
                })
        }
    });
    let has_connection = panel.read(cx).data.connection.is_some();
    menu.item(
        PopupMenuItem::new("Edit Query…")
            .disabled(!has_connection)
            .on_click(on(&tab, |tab, panel_id, window, cx| {
                tab.edit_panel(panel_id, window, cx)
            })),
    )
    .item(PopupMenuItem::new("Rename…").on_click(on(&tab, DashboardTab::rename_panel)))
    .separator()
    .item(PopupMenuItem::submenu("Show as", show_as))
    .item(
        PopupMenuItem::new("Thresholds…")
            .disabled(matches!(kind, PanelKind::Chart(_)))
            .on_click(on(&tab, DashboardTab::open_threshold_dialog)),
    )
    .item(PopupMenuItem::submenu("Width", widths))
    .item(PopupMenuItem::submenu("Height", heights))
    .item(
        PopupMenuItem::new("Move Earlier")
            .disabled(ix == 0)
            .on_click(on(&tab, |tab, panel_id, _, cx| {
                tab.move_panel(panel_id, -1, cx)
            })),
    )
    .item(
        PopupMenuItem::new("Move Later")
            .disabled(ix >= last)
            .on_click(on(&tab, |tab, panel_id, _, cx| {
                tab.move_panel(panel_id, 1, cx)
            })),
    )
    .separator()
    .item(
        PopupMenuItem::new("Remove Panel").on_click(on(&tab, |tab, panel_id, _, cx| {
            tab.remove_panel(panel_id, cx)
        })),
    )
}

/// Group the queries by the server and database they run on, keeping the
/// panels' order within each.
fn server_groups(queries: Vec<PanelQuery>) -> Vec<ServerGroup> {
    let mut groups: Vec<ServerGroup> = Vec::new();
    for query in queries {
        match groups.iter_mut().find(|group| {
            group.connection_id == query.connection_id && group.database_name == query.database_name
        }) {
            Some(group) => group.queries.push(query),
            None => groups.push(ServerGroup {
                connection_id: query.connection_id,
                connection_name: query.connection_name.clone(),
                database_name: query.database_name.clone(),
                queries: vec![query],
            }),
        }
    }
    groups
}

/// Run one server's panels on a connection opened for them, and close it.
async fn run_server_group(
    service: DatabaseService,
    group: ServerGroup,
    timeout: Option<Duration>,
    closed: TabClosed,
    sender: futures::channel::mpsc::UnboundedSender<PanelOutcome>,
    activity: ActivityReporter,
    dashboard_name: String,
) {
    let database = (!group.database_name.is_empty()).then_some(group.database_name.as_str());
    let connecting = activity.begin(format!(
        "{dashboard_name}: connecting to {}",
        group.connection_name
    ));
    let send = |panel_id: i64, result: Result<QueryResult, String>| {
        // The tab was closed; the refresh still closes its connection.
        if sender
            .unbounded_send(PanelOutcome { panel_id, result })
            .is_err()
        {
            tracing::debug!("A dashboard closed before its panel {panel_id} was shown");
        }
    };
    let connection = match service
        .open_short_lived_read_connection(group.connection_id, database)
        .await
    {
        Ok(connection) => connection,
        Err(error) => {
            let message = format!("Could not connect: {error:#}");
            for query in &group.queries {
                send(query.panel_id, Err(message.clone()));
            }
            return;
        }
    };
    drop(connecting);
    let _running = activity.begin(format!(
        "{dashboard_name}: running {} on {}",
        panels_text(group.queries.len()),
        group.connection_name
    ));
    for query in &group.queries {
        let run = connection.connection.execute_script(&query.query, database);
        let results = bounded_run(run, timeout, closed.clone()).await;
        send(
            query.panel_id,
            results
                .map_err(|error| format!("{error:#}"))
                .and_then(shown_result),
        );
    }
    service.close_short_lived_connection(connection).await;
}

fn panels_text(count: usize) -> String {
    match count {
        1 => "1 panel".to_string(),
        count => format!("{count} panels"),
    }
}

/// The result a panel shows of its statements' results: the last one with
/// columns, as a script's final query is what it is for.
fn shown_result(results: Vec<QueryResult>) -> Result<QueryResult, String> {
    let mut results = results;
    let ix = results
        .iter()
        .rposition(|result| !result.columns.is_empty())
        .ok_or_else(|| "The query returned no result to show.".to_string())?;
    Ok(results.swap_remove(ix))
}

/// Ask for a name, create an empty dashboard and show it in `editor`.
pub(crate) fn new_dashboard(editor: WeakEntity<EditorPanel>, window: &mut Window, cx: &mut App) {
    open_name_dialog(
        "New Dashboard",
        "Create",
        String::new(),
        window,
        cx,
        move |name, window, cx| {
            let database = AppDatabase::global(cx).clone();
            let editor = editor.clone();
            window
                .spawn(cx, async move |cx| {
                    let created = database.create_dashboard(&name, None).await;
                    cx.update(|window, cx| match created {
                        Ok(dashboard_id) => {
                            DashboardDirectory::reload(cx);
                            editor
                                .update(cx, |editor, cx| {
                                    editor.open_dashboard_tab(dashboard_id, window, cx);
                                })
                                .log_err();
                        }
                        Err(error) => window.push_notification(
                            (
                                NotificationType::Error,
                                format!("The dashboard was not created: {error}"),
                            ),
                            cx,
                        ),
                    })
                    .log_err();
                })
                .detach();
        },
    );
}

/// A context on a saved connection's own database, as opening a query tab on
/// the connection gives.
pub(crate) fn connection_context(
    connection: &app_database::ConnectionData,
) -> Option<ConnectionContext> {
    let dialect = connection.db_type.dialect();
    Some(ConnectionContext {
        connection_id: connection.id?,
        connection_name: connection.name.clone(),
        db_type: connection.db_type,
        database_name: match dialect.file_database_name() {
            "" => connection.database_name.clone().unwrap_or_default(),
            file => file.to_string(),
        },
        schema_name: None,
        environment_type: Some(connection.environment_type),
    })
}

/// Ask before deleting a dashboard with its panels, and close its tabs.
pub(crate) fn confirm_delete_dashboard(
    dashboard_id: i64,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Delete Dashboard")
            .w(px(380.))
            .child(format!(
                "Delete \"{name}\" and its panels? The queries they ran are not kept anywhere else."
            ))
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new().child(Button::new("cancel").label("Cancel").outline()),
                    )
                    .child(
                        DialogAction::new().child(Button::new("delete").danger().label("Delete")),
                    ),
            )
            .on_ok(move |_, _, cx| {
                let database = AppDatabase::global(cx).clone();
                cx.spawn(async move |cx| {
                    let deleted = database.delete_dashboard(dashboard_id).await;
                    cx.update(|cx| match deleted {
                        Ok(()) => DashboardDirectory::reload(cx),
                        Err(error) => tracing::error!("Failed to delete the dashboard: {error}"),
                    });
                })
                .detach();
                // Its tabs close when the dashboard leaves the list.
                true
            })
    });
}

/// Ask for a name in a dialog and hand it to `on_name` unless it is blank.
pub(crate) fn open_name_dialog(
    title: &'static str,
    ok_label: &'static str,
    current: String,
    window: &mut Window,
    cx: &mut App,
    on_name: impl Fn(String, &mut Window, &mut App) + 'static,
) {
    let form = RenameTabForm::new(current, window, cx);
    let on_name = std::rc::Rc::new(on_name);
    window.open_dialog(cx, move |dialog, _, _| {
        let form = form.clone();
        let on_name = on_name.clone();
        dialog
            .title(title)
            .w(px(320.))
            .child(form.clone())
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new().child(Button::new("cancel").label("Cancel").outline()),
                    )
                    .child(DialogAction::new().child(Button::new("ok").primary().label(ok_label))),
            )
            .on_ok(move |_, window, cx| {
                let name = form.read(cx).get_value(cx).trim().to_string();
                if name.is_empty() {
                    return false;
                }
                on_name(name, window, cx);
                true
            })
    });
}

impl Focusable for DashboardTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DashboardTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Only a shown tab is drawn, so a refresh that came due while it was
        // hidden runs now.
        // A resize ends when its drag does, wherever the pointer let go.
        if self.resize_start.is_some() && !cx.has_active_drag() {
            cx.defer_in(window, |this, _, cx| this.finish_resize(cx));
        }
        if self.refresh_due && !self.refreshing {
            self.refresh_due = false;
            cx.defer_in(window, |this, window, cx| this.refresh(window, cx));
        }
        let danger = cx.theme().danger;
        let background = cx.theme().muted.opacity(0.4);
        let body = if let Some(error) = &self.load_error {
            div()
                .p_4()
                .text_sm()
                .text_color(danger)
                .child(error.clone())
                .into_any_element()
        } else if self.loading {
            div().into_any_element()
        } else if self.panels.is_empty() {
            self.render_empty(cx).into_any_element()
        } else {
            let panels: Vec<_> = self
                .panels
                .clone()
                .iter()
                .enumerate()
                .map(|(ix, panel)| self.render_panel(ix, panel, cx).into_any_element())
                .collect();
            div()
                .id("dashboard-grid")
                .size_full()
                .on_drag_move(cx.listener(Self::on_resize_move))
                .overflow_y_scroll()
                .track_scroll(&self.scroll_handle)
                .p_2()
                .child(h_flex().w_full().flex_wrap().items_start().children(panels))
                .into_any_element()
        };
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(background)
            .child(self.render_header(cx))
            .children(self.render_variables(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl EditorPanel {
    /// Show the dashboard, in the tab already open for it when there is one.
    pub fn open_dashboard_tab(
        &mut self,
        dashboard_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<DashboardTab> {
        if let Some((ix, tab)) = self.dashboard_tab(dashboard_id, cx) {
            self.activate_tab(ix, window, cx);
            return tab;
        }
        let editor = cx.entity().downgrade();
        let tab = cx.new(|cx| DashboardTab::new(dashboard_id, editor, window, cx));
        self.tabs.push(TabType::Dashboard(tab.clone()));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
        tab
    }

    /// Reopen the dashboards the window had open when the app last ran,
    /// showing the one that was active.
    pub fn restore_dashboard_tabs(
        &mut self,
        dashboards: Vec<(i64, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previously_active = self.active_tab_ix;
        let mut active = None;
        for (dashboard_id, was_active) in &dashboards {
            let editor = cx.entity().downgrade();
            let tab = cx.new(|cx| DashboardTab::new(*dashboard_id, editor, window, cx));
            self.tabs.push(TabType::Dashboard(tab));
            if *was_active {
                active = Some(self.tabs.len() - 1);
            }
        }
        self.active_tab_ix =
            active.unwrap_or(previously_active.min(self.tabs.len().saturating_sub(1)));
        self.persisted_dashboards = dashboards;
        cx.notify();
    }

    /// Write which dashboards the window has open when that changed.
    pub(super) fn persist_open_dashboards(&mut self, cx: &mut Context<Self>) {
        let open: Vec<(i64, bool)> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| match tab {
                TabType::Dashboard(tab) => {
                    Some((tab.read(cx).dashboard_id, ix == self.active_tab_ix))
                }
                _ => None,
            })
            .collect();
        if open == self.persisted_dashboards {
            return;
        }
        self.persisted_dashboards = open.clone();
        let database = AppDatabase::global(cx).clone();
        let window_id = self.window_id;
        cx.background_spawn(async move {
            if let Err(error) = database.save_window_dashboards(window_id, &open).await {
                tracing::error!("Failed to remember the open dashboards: {error}");
            }
        })
        .detach();
    }

    /// The bar over a query tab that edits a dashboard panel, or was opened
    /// to add one: what it belongs to, and saving the result there.
    pub(super) fn render_dashboard_link_bar(
        &self,
        tab: &super::QueryTab,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let results = tab.results_panel.read(cx);
        let link = results.dashboard_link()?.clone();
        let has_result = results.dashboard_panel_source(cx).is_some();
        let theme = cx.theme();
        let dashboard_id = link.dashboard_id;
        let (message, button) = match &link.panel {
            Some(panel) => (
                format!("Editing \"{}\" on {}", panel.title, link.dashboard_name),
                Button::new("dashboard-link-save")
                    .small()
                    .primary()
                    .label("Save to Panel")
                    .tooltip("Save this query and how its result is shown into the panel")
                    .on_click(cx.listener(|editor, _, window, cx| {
                        editor.update_linked_dashboard_panel(window, cx)
                    })),
            ),
            None => (
                format!("New panel for {}", link.dashboard_name),
                Button::new("dashboard-link-add")
                    .small()
                    .primary()
                    .label("Add to Dashboard")
                    .tooltip("Add this query and how its result is shown as a panel")
                    .on_click(cx.listener(move |editor, _, window, cx| {
                        editor.add_result_to_dashboard(Some(dashboard_id), window, cx)
                    })),
            ),
        };
        Some(
            h_flex()
                .w_full()
                .flex_shrink_0()
                .px_3()
                .py_1p5()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.accent.opacity(0.4))
                .child(
                    gpui_component::Icon::new(IconName::LayoutDashboard)
                        .small()
                        .text_color(theme.green),
                )
                .child(div().text_sm().truncate().child(message))
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .when(!has_result, |this| {
                            this.child("Saving runs the query first. Show its result as a table or a chart to save it so.")
                        }),
                )
                .child(
                    Button::new("dashboard-link-back")
                        .small()
                        .ghost()
                        .label("Back to Dashboard")
                        .on_click(cx.listener(move |editor, _, window, cx| {
                            editor.open_dashboard_tab(dashboard_id, window, cx);
                        })),
                )
                .child(button)
                .into_any_element(),
        )
    }

    /// The dashboard the active tab shows.
    pub fn active_dashboard_id(&self, cx: &App) -> Option<i64> {
        match self.tabs.get(self.active_tab_ix)? {
            TabType::Dashboard(tab) => Some(tab.read(cx).dashboard_id),
            _ => None,
        }
    }

    fn dashboard_tab(&self, dashboard_id: i64, cx: &App) -> Option<(usize, Entity<DashboardTab>)> {
        self.tabs
            .iter()
            .enumerate()
            .find_map(|(ix, tab)| match tab {
                TabType::Dashboard(tab) if tab.read(cx).dashboard_id == dashboard_id => {
                    Some((ix, tab.clone()))
                }
                _ => None,
            })
    }

    /// Read an open dashboard again, after a panel was added or saved.
    pub fn reload_dashboard_tab(
        &mut self,
        dashboard_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, tab)) = self.dashboard_tab(dashboard_id, cx) {
            tab.update(cx, |tab, cx| tab.load(window, cx));
        }
    }

    /// Close the tabs of a deleted dashboard.
    pub fn close_dashboard_tabs(
        &mut self,
        dashboard_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        while let Some((ix, _)) = self.dashboard_tab(dashboard_id, cx) {
            self.remove_tab(ix, cx);
        }
        self.focus_active_tab(window, cx);
        cx.notify();
    }

    /// Open a panel's query in a query tab tied to the panel and run it, so
    /// its result can be changed and saved back.
    pub fn edit_dashboard_panel(
        &mut self,
        link: DashboardLink,
        context: ConnectionContext,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = link
            .panel
            .as_ref()
            .map(|panel| panel.title.clone())
            .unwrap_or_else(|| "Panel".to_string());
        self.create_and_add_tab_with_connection(
            window,
            TabCreationParams {
                title,
                content: Some(sql),
                db_id: None,
                last_run_at: None,
                context,
            },
            cx,
        );
        if let Some(TabType::Query(tab)) = self.tabs.get(self.active_tab_ix) {
            tab.results_panel.update(cx, |results, cx| {
                results.set_dashboard_link(Some(link), true, cx)
            });
        }
        self.on_run_query(window, cx);
    }

    /// Open an empty query tab on `context` whose result offers to become a
    /// panel of the linked dashboard.
    pub fn open_tab_for_new_panel(
        &mut self,
        link: DashboardLink,
        context: ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_and_add_tab_with_connection(
            window,
            TabCreationParams {
                title: format!("New panel: {}", link.dashboard_name),
                content: Some(String::new()),
                db_id: None,
                last_run_at: None,
                context,
            },
            cx,
        );
        if let Some(TabType::Query(tab)) = self.tabs.get(self.active_tab_ix) {
            tab.results_panel.update(cx, |results, cx| {
                results.set_dashboard_link(Some(link), false, cx)
            });
        }
    }

    /// Add the active query tab's current result to a dashboard as a new
    /// panel, `None` for a new dashboard named first.
    pub fn add_result_to_dashboard(
        &mut self,
        dashboard_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.result_is_current(cx) {
            self.run_then_save(PendingPanelSave::Add(dashboard_id), window, cx);
            return;
        }
        self.add_current_result_to_dashboard(dashboard_id, window, cx);
    }

    fn add_current_result_to_dashboard(
        &mut self,
        dashboard_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(TabType::Query(tab)) = self.tabs.get(self.active_tab_ix) else {
            return;
        };
        let context = tab.context.clone();
        let results = tab.results_panel.clone();
        let Some((query, view)) = results.read(cx).dashboard_panel_source(cx) else {
            window.push_notification(
                (
                    NotificationType::Warning,
                    "Run a query first: a panel shows the result of the query it ran.",
                ),
                cx,
            );
            return;
        };
        let editor_text = tab.editor.read(cx).text().to_string();
        let title = panel_title(&editor_text, &query, &tab.title);
        let (width, height) = match view.kind() {
            PanelKind::Stat => (3, 2),
            PanelKind::Table | PanelKind::Chart(_) => (6, 4),
        };
        let panel = DashboardPanelData {
            id: 0,
            dashboard_id: dashboard_id.unwrap_or_default(),
            title,
            connection: Some(app_database::PanelConnection {
                id: context.connection_id,
                name: context.connection_name.clone(),
                db_type: context.db_type.as_str().to_string(),
                environment_type: context.environment_type.unwrap_or_default(),
            }),
            database_name: context.database_name.clone(),
            schema_name: context.schema_name.clone(),
            query,
            view: view.to_json(),
            position: 0,
            width,
            height,
        };
        let editor = cx.entity().downgrade();
        let save = move |dashboard_id: i64,
                         name: Option<String>,
                         open: bool,
                         window: &mut Window,
                         cx: &mut App| {
            let database = AppDatabase::global(cx).clone();
            let mut panel = panel.clone();
            panel.dashboard_id = dashboard_id;
            let editor = editor.clone();
            let results = results.clone();
            window
                .spawn(cx, async move |cx| {
                    let inserted = database.insert_dashboard_panel(&panel).await;
                    cx.update(|window, cx| match inserted {
                        Ok(panel_id) => {
                            let dashboard_name = name.clone().or_else(|| {
                                DashboardDirectory::dashboards(cx)
                                    .iter()
                                    .find(|dashboard| dashboard.id == dashboard_id)
                                    .map(|dashboard| dashboard.name.clone())
                            });
                            let dashboard_name = dashboard_name.unwrap_or_default();
                            // The dashboard's range carries over when the tab
                            // already came from it, else its saved one.
                            let saved_range = DashboardDirectory::dashboards(cx)
                                .iter()
                                .find(|dashboard| dashboard.id == dashboard_id)
                                .map(|dashboard| TimeRange::from_key(&dashboard.time_range))
                                .unwrap_or_default();
                            results.update(cx, |results, cx| {
                                let (time_range, variable_values) = results
                                    .dashboard_link()
                                    .filter(|link| link.dashboard_id == dashboard_id)
                                    .map(|link| (link.time_range, link.variable_values.clone()))
                                    .unwrap_or((saved_range, Vec::new()));
                                results.set_dashboard_link(
                                    Some(DashboardLink {
                                        time_range,
                                        variable_values,
                                        dashboard_id,
                                        dashboard_name: dashboard_name.clone(),
                                        panel: Some(LinkedPanel {
                                            id: panel_id,
                                            title: panel.title.clone(),
                                            width: panel.width,
                                            height: panel.height,
                                            view: PanelView::from_json(&panel.view),
                                        }),
                                    }),
                                    false,
                                    cx,
                                );
                            });
                            editor
                                .update(cx, |editor, cx| {
                                    if open {
                                        editor.open_dashboard_tab(dashboard_id, window, cx);
                                    } else {
                                        editor.reload_dashboard_tab(dashboard_id, window, cx);
                                        window.push_notification(
                                            (
                                                NotificationType::Success,
                                                format!(
                                                    "Added \"{}\" to {dashboard_name}",
                                                    panel.title
                                                ),
                                            ),
                                            cx,
                                        );
                                    }
                                })
                                .log_err();
                            DashboardDirectory::reload(cx);
                        }
                        Err(error) => window.push_notification(
                            (
                                NotificationType::Error,
                                format!("The panel was not added: {error}"),
                            ),
                            cx,
                        ),
                    })
                    .log_err();
                })
                .detach();
        };
        match dashboard_id {
            Some(dashboard_id) => save(dashboard_id, None, false, window, cx),
            None => {
                let connection_id = context.connection_id;
                open_name_dialog(
                    "New Dashboard",
                    "Create",
                    String::new(),
                    window,
                    cx,
                    move |name, window, cx| {
                        let database = AppDatabase::global(cx).clone();
                        let save = save.clone();
                        window
                            .spawn(cx, async move |cx| {
                                let created =
                                    database.create_dashboard(&name, Some(connection_id)).await;
                                cx.update(|window, cx| match created {
                                    Ok(dashboard_id) => {
                                        save(dashboard_id, Some(name), true, window, cx)
                                    }
                                    Err(error) => window.push_notification(
                                        (
                                            NotificationType::Error,
                                            format!("The dashboard was not created: {error}"),
                                        ),
                                        cx,
                                    ),
                                })
                                .log_err();
                            })
                            .detach();
                    },
                );
            }
        }
    }

    /// Save the active query tab's current result into the panel it came
    /// from.
    pub fn update_linked_dashboard_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.result_is_current(cx) {
            self.run_then_save(PendingPanelSave::Update, window, cx);
            return;
        }
        self.save_current_result_to_panel(window, cx);
    }

    /// Whether the active query tab's result came from the query its editor
    /// holds now, so saving it saves what the user sees in the editor.
    fn result_is_current(&self, cx: &App) -> bool {
        let Some(TabType::Query(tab)) = self.tabs.get(self.active_tab_ix) else {
            return false;
        };
        let shown = tab
            .results_panel
            .read(cx)
            .dashboard_panel_source(cx)
            .map(|(query, _)| query);
        let written = self
            .statement_to_run(self.active_tab_ix, cx)
            .ok()
            .flatten()
            .map(|statement| statement.query);
        match (shown, written) {
            (Some(shown), Some(written)) => shown.trim() == written.trim(),
            _ => false,
        }
    }

    /// Run the active tab's query, and save its result once the run ends
    /// well.
    fn run_then_save(
        &mut self,
        save: PendingPanelSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(TabType::Query(tab)) = self.tabs.get(self.active_tab_ix) else {
            return;
        };
        self.pending_panel_save = Some((tab.results_panel.clone(), save));
        self.on_run_query(window, cx);
    }

    /// Finish a save waiting on the run of the tab at `tab_index`.
    pub(super) fn finish_pending_panel_save(
        &mut self,
        tab_index: usize,
        outcome: &Result<serde_json::Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((results, save)) = self.pending_panel_save.take() else {
            return;
        };
        let Some(TabType::Query(tab)) = self.tabs.get(tab_index) else {
            self.pending_panel_save = Some((results, save));
            return;
        };
        if tab.results_panel != results {
            self.pending_panel_save = Some((results, save));
            return;
        }
        if let Err(error) = outcome {
            window.push_notification(
                (
                    NotificationType::Warning,
                    format!("Nothing was saved to the dashboard: {error}"),
                ),
                cx,
            );
            return;
        }
        if self.active_tab_ix != tab_index {
            self.activate_tab(tab_index, window, cx);
        }
        match save {
            PendingPanelSave::Update => self.save_current_result_to_panel(window, cx),
            PendingPanelSave::Add(dashboard_id) => {
                self.add_current_result_to_dashboard(dashboard_id, window, cx)
            }
        }
    }

    fn save_current_result_to_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(TabType::Query(tab)) = self.tabs.get(self.active_tab_ix) else {
            return;
        };
        let context = tab.context.clone();
        let results = tab.results_panel.clone();
        let Some(link) = results.read(cx).dashboard_link().cloned() else {
            return;
        };
        let Some(linked) = link.panel.clone() else {
            return;
        };
        let Some((query, view)) = results.read(cx).dashboard_panel_source(cx) else {
            return;
        };
        // A stat shows as a table in a query tab; saving that table keeps
        // the stat.
        let view = view.keeping_settings_of(&linked.view);
        let panel = DashboardPanelData {
            id: linked.id,
            dashboard_id: link.dashboard_id,
            title: linked.title.clone(),
            connection: Some(app_database::PanelConnection {
                id: context.connection_id,
                name: context.connection_name.clone(),
                db_type: context.db_type.as_str().to_string(),
                environment_type: context.environment_type.unwrap_or_default(),
            }),
            database_name: context.database_name.clone(),
            schema_name: context.schema_name.clone(),
            query,
            view: view.to_json(),
            position: 0,
            width: linked.width,
            height: linked.height,
        };
        let database = AppDatabase::global(cx).clone();
        cx.spawn_in(window, async move |editor, cx| {
            let saved = database.update_dashboard_panel_source(&panel).await;
            editor
                .update_in(cx, |editor, window, cx| match saved {
                    Ok(()) => {
                        results.update(cx, |results, _| {
                            if let Some(link) = results.dashboard_link_mut()
                                && let Some(linked) = &mut link.panel
                            {
                                linked.view = view;
                            }
                        });
                        editor.reload_dashboard_tab(link.dashboard_id, window, cx);
                        window.push_notification(
                            (
                                NotificationType::Success,
                                format!("Saved \"{}\" on {}", linked.title, link.dashboard_name),
                            ),
                            cx,
                        );
                    }
                    Err(error) => window.push_notification(
                        (
                            NotificationType::Error,
                            format!("The panel was not saved: {error}"),
                        ),
                        cx,
                    ),
                })
                .log_err();
        })
        .detach();
    }
}

/// A new panel's title: the comment on the line above the statement in the
/// editor, or the one it opens with, else the tab's title.
fn panel_title(editor_text: &str, query: &str, tab_title: &str) -> String {
    let comment = |line: &str| {
        line.trim()
            .strip_prefix("--")
            .map(str::trim)
            .filter(|comment| !comment.is_empty())
            .map(str::to_string)
    };
    let above = editor_text.find(query.trim()).and_then(|start| {
        editor_text[..start]
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(comment)
    });
    above
        .or_else(|| {
            query
                .lines()
                .find(|line| !line.trim().is_empty())
                .and_then(comment)
        })
        .unwrap_or_else(|| tab_title.to_string())
}

#[cfg(test)]
#[path = "dashboard_test.rs"]
mod dashboard_test;

#[cfg(test)]
mod tests {
    use super::*;

    fn query(panel_id: i64, connection_id: i64, database_name: &str) -> PanelQuery {
        PanelQuery {
            panel_id,
            db_type: blanco_core::DatabaseType::SQLite,
            connection_id,
            connection_name: format!("Connection {connection_id}"),
            database_name: database_name.to_string(),
            query: "SELECT 1".to_string(),
        }
    }

    #[test]
    fn a_run_without_a_timeout_ends_when_the_tab_closes() {
        let (open, closed) = futures::channel::oneshot::channel::<()>();
        let closed = futures::FutureExt::shared(closed);
        drop(open);
        let result = futures::executor::block_on(super::bounded_run(
            futures::future::pending::<anyhow::Result<()>>(),
            None,
            closed,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn panels_on_one_server_and_database_share_a_group() {
        let groups = server_groups(vec![
            query(1, 7, "sales"),
            query(2, 8, "events"),
            query(3, 7, "sales"),
            query(4, 7, "archive"),
        ]);
        let shape: Vec<(i64, &str, Vec<i64>)> = groups
            .iter()
            .map(|group| {
                (
                    group.connection_id,
                    group.database_name.as_str(),
                    group.queries.iter().map(|query| query.panel_id).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (7, "sales", vec![1, 3]),
                (8, "events", vec![2]),
                (7, "archive", vec![4]),
            ]
        );
    }

    #[test]
    fn a_panel_is_titled_by_its_opening_comment() {
        let editor = "-- Orders waiting to ship\nSELECT 1;\n\n-- Best sellers\nSELECT 2;";
        assert_eq!(panel_title(editor, "SELECT 2", "Query 1"), "Best sellers");
        assert_eq!(
            panel_title("", "-- Pending\nSELECT 1", "Query 1"),
            "Pending"
        );
        assert_eq!(panel_title("SELECT 1", "SELECT 1", "Query 1"), "Query 1");
    }
}
