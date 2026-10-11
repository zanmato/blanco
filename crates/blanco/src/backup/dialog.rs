use anyhow::{Result, anyhow};
use blanco_core::{ClientTools, ConnectionContext};
use chrono::Local;
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Disableable as _, IndexPath, Sizable as _, WindowExt,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::{DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputEvent, InputState},
    notification::NotificationType,
    progress::Progress,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::command::{
    BackupFormat, BackupRequest, Compression, DumpFile, DumpFormat, ProgressSource, TableChoice,
    ToolPlan, backup_plan, count_archive_entries, detect_dump, expand_file_name, restore_plan,
    split_arguments, unique_output_path,
};
use super::runner::{RunEvent, RunOutcome, capture_output, run_plan};
use super::tools::{Compatibility, LocatedTool, ToolProgram, Version, compatibility, locate};
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::settings::ClientToolSettings;

/// Lines of tool output kept for the live view.
const OUTPUT_LINES: usize = 500;

/// A located tool checked against the server it is about to work on.
#[derive(Clone, Debug)]
struct ToolCheck {
    tool: LocatedTool,
    compatibility: Compatibility,
}

#[derive(Clone, Debug)]
enum Lookup<T> {
    Pending,
    Ready(T),
    Failed(String),
}

impl<T> Lookup<T> {
    fn from_result(result: Result<T>) -> Self {
        match result {
            Ok(value) => Lookup::Ready(value),
            Err(error) => Lookup::Failed(format!("{error:#}")),
        }
    }

    fn ready(&self) -> Option<&T> {
        match self {
            Lookup::Ready(value) => Some(value),
            Lookup::Pending | Lookup::Failed(_) => None,
        }
    }
}

async fn server_version(
    db_service: &DatabaseService,
    context: &ConnectionContext,
    tools: ClientTools,
) -> Result<Version> {
    let connection = db_service
        .get_or_create_connection(context.connection_id, Some(&context.database_name))
        .await?;
    let result = connection
        .execute_query(
            tools.server_version_query(),
            Some(&context.database_name),
            None,
        )
        .await?;
    let text = result
        .rows
        .first()
        .and_then(|row| row.first())
        .and_then(|value| value.clone())
        .ok_or_else(|| anyhow!("The server did not report its version"))?;
    Version::parse_server(tools, &text)
        .ok_or_else(|| anyhow!("The server reported an unexpected version: {text}"))
}

async fn check_tool(
    program: ToolProgram,
    server: &Version,
    settings: &ClientToolSettings,
) -> Result<ToolCheck> {
    let tool = locate(program, settings, Some(server)).await?;
    let compatibility = compatibility(program, &tool.version, server);
    Ok(ToolCheck {
        tool,
        compatibility,
    })
}

async fn list_tables(
    db_service: &DatabaseService,
    context: &ConnectionContext,
) -> Result<Vec<TableChoice>> {
    let connection = db_service
        .get_or_create_connection(context.connection_id, Some(&context.database_name))
        .await?;
    let dialect = context.db_type.dialect();
    let mut tables = Vec::new();
    if connection.supports_schemas() {
        for schema in connection.get_schemas().await? {
            if dialect.is_system_schema(&schema) {
                continue;
            }
            for table in connection.get_tables(Some(&schema)).await? {
                tables.push(TableChoice::new(Some(schema.clone()), table));
            }
        }
    } else {
        for table in connection.get_tables(None).await? {
            tables.push(TableChoice::new(None, table));
        }
    }
    Ok(tables)
}

/// Persist a backup setting the dialog changed, such as the last folder.
fn remember_setting(key: &'static str, value: String, cx: &mut App) {
    let database = app_database::AppDatabase::global(cx).clone();
    let save = gpui_tokio::Tokio::spawn_result(cx, async move {
        database
            .save_setting(key, &value, false)
            .await
            .map_err(anyhow::Error::from)
    });
    cx.spawn(async move |_| {
        if let Err(error) = save.await {
            tracing::error!("Failed to save {key}: {error:#}");
        }
    })
    .detach();
}

/// Text shown when the run ends, in the dialog and as a notification.
struct RunMessages {
    finished: String,
    stopped: String,
    failed: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RunState {
    Running,
    Stopping,
    Finished,
    Stopped,
    Failed(String),
}

/// A tool run shown in a dialog: progress, live output and the result.
struct ToolRun {
    started: Instant,
    elapsed: Option<Duration>,
    output: VecDeque<SharedString>,
    output_scroll: ScrollHandle,
    progress: ProgressSource,
    counted: usize,
    input_read: Arc<AtomicU64>,
    stop: smol::channel::Sender<()>,
    state: RunState,
    finished_message: String,
}

impl ToolRun {
    fn record(&mut self, events: Vec<RunEvent>) {
        if events.is_empty() {
            return;
        }
        for event in events {
            let line = match event {
                RunEvent::Command(command) => format!("$ {command}"),
                RunEvent::Output(line) => {
                    if let ProgressSource::Lines { counted, .. } = &self.progress
                        && counted.matches(&line)
                    {
                        self.counted += 1;
                    }
                    line
                }
            };
            if self.output.len() == OUTPUT_LINES {
                self.output.pop_front();
            }
            self.output.push_back(line.into());
        }
        self.output_scroll.scroll_to_bottom();
    }

    /// Percent done, `None` while it cannot be told.
    fn percent(&self) -> Option<f32> {
        match &self.progress {
            ProgressSource::InputBytes { total } if *total > 0 => {
                Some(self.input_read.load(Ordering::Relaxed) as f32 / *total as f32 * 100.0)
            }
            ProgressSource::Lines { total, .. } if *total > 0 => {
                Some(self.counted as f32 / *total as f32 * 100.0)
            }
            ProgressSource::InputBytes { .. }
            | ProgressSource::Lines { .. }
            | ProgressSource::Unknown => None,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self.state, RunState::Running | RunState::Stopping)
    }

    fn request_stop(&mut self) {
        if self.state == RunState::Running {
            self.state = RunState::Stopping;
            if self.stop.try_send(()).is_err() {
                tracing::debug!("The tool already finished");
            }
        }
    }

    fn status_line(&self) -> String {
        let elapsed = self.elapsed.unwrap_or_else(|| self.started.elapsed());
        let seconds = elapsed.as_secs();
        let clock = format!("{}:{:02}", seconds / 60, seconds % 60);
        let detail = match &self.progress {
            ProgressSource::Lines { total, .. } => {
                format!("{} of {total} done", self.counted.min(*total))
            }
            ProgressSource::InputBytes { total } => format!(
                "{} of {} read",
                format_bytes(self.input_read.load(Ordering::Relaxed)),
                format_bytes(*total)
            ),
            ProgressSource::Unknown => String::new(),
        };
        match &self.state {
            RunState::Running if detail.is_empty() => format!("Running for {clock}"),
            RunState::Running => format!("{detail} · {clock}"),
            RunState::Stopping => "Stopping…".to_string(),
            RunState::Finished => format!("{} in {clock}", self.finished_message),
            RunState::Stopped => "Stopped".to_string(),
            RunState::Failed(_) => format!("Failed after {clock}"),
        }
    }

    fn render(&self, cx: &App) -> impl IntoElement + use<> {
        let percent = self.percent();
        let finished = !self.is_running();
        let bar_color = match &self.state {
            RunState::Failed(_) => Some(cx.theme().danger),
            RunState::Finished => Some(cx.theme().success),
            _ => None,
        };
        v_flex()
            .gap_2()
            .child(
                Progress::new("tool-run-progress")
                    .loading(!finished && percent.is_none())
                    .value(match (&self.state, percent) {
                        (RunState::Finished, _) => 100.0,
                        (_, Some(percent)) => percent,
                        (_, None) => 0.0,
                    })
                    .when_some(bar_color, |progress, color| progress.color(color)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.status_line()),
            )
            .when_some(
                match &self.state {
                    RunState::Failed(message) => Some(message.clone()),
                    _ => None,
                },
                |this, message| {
                    this.child(div().text_sm().text_color(cx.theme().danger).child(message))
                },
            )
            .child(
                div()
                    .id("tool-run-output")
                    .track_scroll(&self.output_scroll)
                    .overflow_y_scroll()
                    .h(px(220.))
                    .p_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .children(
                        self.output
                            .iter()
                            .map(|line| div().whitespace_nowrap().child(line.clone())),
                    ),
            )
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    match UNITS.get(unit) {
        Some(name) if unit == 0 => format!("{bytes} {name}"),
        Some(name) => format!("{value:.1} {name}"),
        None => format!("{bytes} B"),
    }
}

/// A dialog that runs a tool plan.
trait HostsRun: Sized + 'static {
    fn tool_run(&mut self) -> Option<&mut ToolRun>;
}

/// Start `plan` in the background. It keeps running when the dialog closes;
/// the dialog follows it while open and a notification reports the result.
fn start_run<Host: HostsRun>(
    plan: ToolPlan,
    messages: RunMessages,
    window: &mut Window,
    cx: &mut Context<Host>,
) -> ToolRun {
    let (event_sender, events) = smol::channel::unbounded();
    let (stop_sender, stop) = smol::channel::bounded(1);
    let input_read = Arc::new(AtomicU64::new(0));
    let progress = plan.progress.clone();
    let finished_message = messages.finished.clone();
    let run = cx.background_spawn({
        let input_read = Arc::clone(&input_read);
        async move { run_plan(&plan, &event_sender, &stop, &input_read).await }
    });

    cx.spawn_in(window, async move |host, cx| {
        loop {
            let next = futures::future::select(
                Box::pin(events.recv()),
                Box::pin(smol::Timer::after(Duration::from_millis(250))),
            )
            .await;
            let mut received = Vec::new();
            let closed = match next {
                futures::future::Either::Left((Ok(event), _)) => {
                    received.push(event);
                    false
                }
                futures::future::Either::Left((Err(_), _)) => true,
                futures::future::Either::Right(_) => false,
            };
            received.extend(std::iter::from_fn(|| events.try_recv().ok()));
            // The dialog may be gone while the run goes on.
            if host
                .update(cx, |host, cx| {
                    if let Some(run) = host.tool_run() {
                        run.record(received);
                    }
                    cx.notify();
                })
                .is_err()
            {
                tracing::debug!("The tool's dialog is closed");
            }
            if closed {
                break;
            }
        }

        let result = run.await;
        let (state, notification) = match &result {
            Ok(RunOutcome::Finished) => (
                RunState::Finished,
                (NotificationType::Success, messages.finished.clone()),
            ),
            Ok(RunOutcome::Stopped) => (
                RunState::Stopped,
                (NotificationType::Warning, messages.stopped.clone()),
            ),
            Err(error) => {
                tracing::error!("{}: {error:#}", messages.failed);
                (
                    RunState::Failed(format!("{error:#}")),
                    (
                        NotificationType::Error,
                        format!("{}: {error:#}", messages.failed),
                    ),
                )
            }
        };
        if host
            .update(cx, |host, cx| {
                if let Some(run) = host.tool_run() {
                    run.elapsed = Some(run.started.elapsed());
                    run.state = state;
                }
                cx.notify();
            })
            .is_err()
        {
            tracing::debug!("The tool's dialog is closed");
        }
        cx.update(|window, cx| {
            if !window.is_window_active() {
                cx.show_system_notification(gpui::SystemNotification {
                    tag: super::NOTIFICATION_TAG.into(),
                    title: "Blanco".into(),
                    body: notification.1.clone().into(),
                    actions: Vec::new(),
                });
            }
            window.push_notification(notification, cx);
        })
        .log_err();
    })
    .detach();

    ToolRun {
        started: Instant::now(),
        elapsed: None,
        output: VecDeque::new(),
        output_scroll: ScrollHandle::new(),
        progress,
        counted: 0,
        input_read,
        stop: stop_sender,
        state: RunState::Running,
        finished_message,
    }
}

fn select_row(select: &Entity<SelectState<Vec<String>>>, cx: &App) -> usize {
    select
        .read(cx)
        .selected_index(cx)
        .map(|index| index.row)
        .unwrap_or(0)
}

fn labelled(label: &'static str, field: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().child(label))
        .child(field)
}

fn note(text: impl Into<SharedString>, color: gpui::Hsla) -> impl IntoElement {
    div().text_sm().text_color(color).child(text.into())
}

fn render_tool_check(
    program: ToolProgram,
    check: &Lookup<ToolCheck>,
    cx: &App,
) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    match check {
        Lookup::Pending => {
            note(format!("Looking for {}…", program.display_name()), muted).into_any_element()
        }
        Lookup::Failed(message) => note(message.clone(), cx.theme().danger).into_any_element(),
        Lookup::Ready(check) => v_flex()
            .gap_1()
            .child(note(
                format!(
                    "{} {} at {}",
                    check.tool.program.display_name(),
                    check.tool.version,
                    check.tool.path.display()
                ),
                muted,
            ))
            .when_some(check.compatibility.message(), |this, message| {
                let color = if check.compatibility.blocks() {
                    cx.theme().danger
                } else {
                    cx.theme().warning
                };
                this.child(note(message.to_string(), color))
            })
            .into_any_element(),
    }
}

fn default_directory(cx: &App) -> String {
    let remembered = &AppSettings::global(cx).settings.backup.directory;
    if !remembered.is_empty() {
        return remembered.clone();
    }
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn browse_into(
    input: Entity<InputState>,
    options: gpui::PathPromptOptions,
    window: &mut Window,
    cx: &mut App,
) {
    let paths = cx.prompt_for_paths(options);
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.first() else {
                return;
            };
            let value = path.display().to_string();
            cx.update(|window, cx| {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            })
            .log_err();
        })
        .detach();
}

pub struct BackupDialog {
    focus_handle: FocusHandle,
    context: ConnectionContext,
    tools: ClientTools,
    check: Lookup<ToolCheck>,
    server: Option<Version>,
    tables: Lookup<Vec<TableChoice>>,
    format_select: Entity<SelectState<Vec<String>>>,
    compression_select: Entity<SelectState<Vec<String>>>,
    directory_input: Entity<InputState>,
    file_name_input: Entity<InputState>,
    extra_arguments_input: Entity<InputState>,
    starting: bool,
    error: Option<String>,
    run: Option<ToolRun>,
    _subscriptions: Vec<Subscription>,
}

impl HostsRun for BackupDialog {
    fn tool_run(&mut self) -> Option<&mut ToolRun> {
        self.run.as_mut()
    }
}

impl BackupDialog {
    pub fn new(
        context: ConnectionContext,
        tools: ClientTools,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let formats: Vec<String> = BackupFormat::available(tools)
            .iter()
            .map(|format| format.label().to_string())
            .collect();
        let format_select =
            cx.new(|cx| SelectState::new(formats, Some(IndexPath::new(0)), window, cx));
        let compressions: Vec<String> = Compression::ALL
            .iter()
            .map(|compression| compression.label().to_string())
            .collect();
        let compression_select =
            cx.new(|cx| SelectState::new(compressions, Some(IndexPath::new(0)), window, cx));
        let directory = default_directory(cx);
        let directory_input = cx.new(|cx| InputState::new(window, cx).default_value(directory));
        let pattern = AppSettings::global(cx)
            .settings
            .backup
            .file_name_pattern
            .clone();
        let file_name_input = cx.new(|cx| InputState::new(window, cx).default_value(pattern));
        let extra_arguments_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Passed to the tool as is"));

        let subscriptions = vec![
            cx.subscribe_in(
                &format_select,
                window,
                |this, _, _: &SelectEvent<Vec<String>>, window, cx| {
                    // pg_dump compresses archives unless told otherwise, so
                    // they start out compressed and a script starts out plain.
                    let compression = match this.format(cx) {
                        BackupFormat::Plain => Compression::None,
                        BackupFormat::Custom | BackupFormat::Directory => Compression::Default,
                    };
                    let index = Compression::ALL
                        .iter()
                        .position(|candidate| *candidate == compression)
                        .unwrap_or(0);
                    this.compression_select.update(cx, |select, cx| {
                        select.set_selected_index(Some(IndexPath::new(index)), window, cx);
                    });
                    cx.notify();
                },
            ),
            cx.subscribe(
                &compression_select,
                |_, _, _: &SelectEvent<Vec<String>>, cx| cx.notify(),
            ),
            cx.subscribe(&file_name_input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];

        let db_service = DatabaseService::global(cx).clone();
        let settings = AppSettings::global(cx).settings.client_tools.clone();
        cx.spawn_in(window, {
            let context = context.clone();
            async move |this, cx| {
                let server = server_version(&db_service, &context, tools).await;
                let tables = list_tables(&db_service, &context).await;
                let check = match &server {
                    Ok(server) => check_tool(ToolProgram::dump(tools), server, &settings).await,
                    Err(error) => Err(anyhow!("Could not connect: {error:#}")),
                };
                this.update(cx, |this, cx| {
                    this.server = server.ok();
                    this.tables = Lookup::from_result(tables);
                    this.check = Lookup::from_result(check);
                    cx.notify();
                })
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            context,
            tools,
            check: Lookup::Pending,
            server: None,
            tables: Lookup::Pending,
            format_select,
            compression_select,
            directory_input,
            file_name_input,
            extra_arguments_input,
            starting: false,
            error: None,
            run: None,
            _subscriptions: subscriptions,
        }
    }

    fn format(&self, cx: &App) -> BackupFormat {
        BackupFormat::available(self.tools)
            .get(select_row(&self.format_select, cx))
            .copied()
            .unwrap_or(BackupFormat::Plain)
    }

    fn compression(&self, cx: &App) -> Compression {
        Compression::ALL
            .get(select_row(&self.compression_select, cx))
            .copied()
            .unwrap_or(Compression::None)
    }

    fn file_name(&self, cx: &App) -> Result<String> {
        let name = expand_file_name(
            &self.file_name_input.read(cx).value(),
            &self.context.database_name,
            &self.context.connection_name,
            &Local::now(),
        )?;
        Ok(format!(
            "{name}{}",
            self.format(cx).extension(self.compression(cx))
        ))
    }

    fn can_start(&self) -> bool {
        !self.starting
            && self.run.is_none()
            && self
                .check
                .ready()
                .is_some_and(|check| !check.compatibility.blocks())
            && !matches!(self.tables, Lookup::Pending)
    }

    fn set_all_tables(&mut self, include: bool, cx: &mut Context<Self>) {
        if let Lookup::Ready(tables) = &mut self.tables {
            for table in tables {
                table.include = include;
            }
        }
        cx.notify();
    }

    fn update_table(
        &mut self,
        index: usize,
        change: impl FnOnce(&mut TableChoice),
        cx: &mut Context<Self>,
    ) {
        if let Lookup::Ready(tables) = &mut self.tables
            && let Some(table) = tables.get_mut(index)
        {
            change(table);
        }
        cx.notify();
    }

    fn request(&self, cx: &App) -> Result<BackupRequest> {
        let directory = self.directory_input.read(cx).value().trim().to_string();
        if directory.is_empty() {
            return Err(anyhow!("Choose a folder for the backup"));
        }
        let directory = PathBuf::from(directory);
        if !directory.is_dir() {
            return Err(anyhow!("{} is not a folder", directory.display()));
        }
        let format = self.format(cx);
        let compression = self.compression(cx);
        let name = expand_file_name(
            &self.file_name_input.read(cx).value(),
            &self.context.database_name,
            &self.context.connection_name,
            &Local::now(),
        )?;
        Ok(BackupRequest {
            format,
            compression,
            tables: self.tables.ready().cloned().unwrap_or_default(),
            extra_arguments: split_arguments(&self.extra_arguments_input.read(cx).value())?,
            output: unique_output_path(&directory, &name, format.extension(compression)),
        })
    }

    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start() {
            return;
        }
        let Some(check) = self.check.ready().cloned() else {
            return;
        };
        let request = match self.request(cx) {
            Ok(request) => request,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };

        let directory = self.directory_input.read(cx).value().trim().to_string();
        let pattern = self.file_name_input.read(cx).value().trim().to_string();
        let backup_settings = &mut AppSettings::global_mut(cx).settings.backup;
        backup_settings.directory = directory.clone();
        backup_settings.file_name_pattern = pattern.clone();
        remember_setting("backup.directory", directory, cx);
        remember_setting("backup.file_name_pattern", pattern, cx);

        self.starting = true;
        self.error = None;
        cx.notify();

        let context = self.context.clone();
        let server = self.server.clone();
        let db_service = DatabaseService::global(cx).clone();
        cx.spawn_in(window, async move |this, cx| {
            let plan = async {
                let endpoint = db_service
                    .client_tool_endpoint(context.connection_id, &context.database_name)
                    .await?;
                backup_plan(&check.tool, server.as_ref(), &endpoint, &request)
            }
            .await;
            this.update_in(cx, |this, window, cx| {
                this.starting = false;
                match plan {
                    Ok(plan) => {
                        let database = &context.database_name;
                        let messages = RunMessages {
                            finished: format!(
                                "Backed up {database} to {}",
                                request.output.display()
                            ),
                            stopped: format!("Backup of {database} stopped"),
                            failed: format!("Backup of {database} failed"),
                        };
                        this.run = Some(start_run(plan, messages, window, cx));
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
        })
        .detach();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &mut self.run {
            run.request_stop();
        }
        cx.notify();
    }

    pub fn footer(dialog: &Entity<Self>, cx: &App) -> DialogFooter {
        let this = dialog.read(cx);
        let footer = DialogFooter::new();
        match &this.run {
            Some(run) if run.is_running() => footer
                .child(DialogClose::new().child(Button::new("backup-close").label("Close")))
                .child(
                    Button::new("backup-stop")
                        .label("Stop")
                        .disabled(run.state == RunState::Stopping)
                        .on_click({
                            let dialog = dialog.clone();
                            move |_, _, cx| dialog.update(cx, |dialog, cx| dialog.stop(cx))
                        }),
                ),
            Some(_) => footer.child(
                DialogClose::new().child(Button::new("backup-done").primary().label("Done")),
            ),
            None => footer
                .child(
                    DialogClose::new()
                        .child(Button::new("backup-cancel").label("Cancel").outline()),
                )
                .child(
                    Button::new("backup-start")
                        .primary()
                        .label("Back Up")
                        .loading(this.starting)
                        .disabled(!this.can_start())
                        .on_click({
                            let dialog = dialog.clone();
                            move |_, window, cx| {
                                dialog.update(cx, |dialog, cx| dialog.start(window, cx))
                            }
                        }),
                ),
        }
    }

    fn render_tables(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tables = match &self.tables {
            Lookup::Pending => {
                return note("Loading tables…", cx.theme().muted_foreground).into_any_element();
            }
            Lookup::Failed(message) => {
                return note(
                    format!(
                        "Tables could not be listed, the whole database is backed up: {message}"
                    ),
                    cx.theme().warning,
                )
                .into_any_element();
            }
            Lookup::Ready(tables) => tables,
        };
        if tables.is_empty() {
            return note("The database has no tables", cx.theme().muted_foreground)
                .into_any_element();
        }
        let all_included = tables.iter().all(|table| table.include);
        let header = h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .bg(cx.theme().table_head)
            .border_b_1()
            .border_color(cx.theme().table_row_border)
            .child(
                div().flex_1().child(
                    Checkbox::new("backup-all-tables")
                        .small()
                        .label(format!(
                            "{} of {} tables",
                            tables.iter().filter(|table| table.include).count(),
                            tables.len()
                        ))
                        .checked(all_included)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.set_all_tables(*checked, cx);
                        })),
                ),
            )
            .child(
                div()
                    .w(px(110.))
                    .text_xs()
                    .text_color(cx.theme().table_head_foreground)
                    .child("Structure only"),
            );
        let rows = tables.iter().enumerate().map(|(index, table)| {
            h_flex()
                .gap_2()
                .px_2()
                .py_0p5()
                .child(
                    div().flex_1().min_w_0().child(
                        Checkbox::new(("backup-table", index))
                            .small()
                            .label(table.display_name())
                            .checked(table.include)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                let checked = *checked;
                                this.update_table(index, |table| table.include = checked, cx);
                            })),
                    ),
                )
                .child(
                    div().w(px(110.)).child(
                        Checkbox::new(("backup-structure", index))
                            .small()
                            .checked(table.structure_only)
                            .disabled(!table.include)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                let checked = *checked;
                                this.update_table(
                                    index,
                                    |table| table.structure_only = checked,
                                    cx,
                                );
                            })),
                    ),
                )
        });
        v_flex()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .overflow_hidden()
            .child(header)
            .child(
                v_flex()
                    .id("backup-tables")
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}

impl Focusable for BackupDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BackupDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(run) = &self.run {
            return run.render(cx).into_any_element();
        }
        let program = ToolProgram::dump(self.tools);
        let formats = BackupFormat::available(self.tools);
        let file_name = self.file_name(cx);
        v_flex()
            .gap_4()
            .child(render_tool_check(program, &self.check, cx))
            .child(
                h_flex()
                    .gap_3()
                    .when(formats.len() > 1, |this| {
                        this.child(
                            div()
                                .flex_1()
                                .child(labelled("Format", Select::new(&self.format_select))),
                        )
                    })
                    .child(
                        div().flex_1().child(labelled(
                            "Compression",
                            Select::new(&self.compression_select),
                        )),
                    ),
            )
            .child(labelled(
                "Folder",
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.directory_input).flex_1())
                    .child(Button::new("backup-browse").label("Browse…").on_click({
                        let input = self.directory_input.clone();
                        move |_, window, cx| {
                            browse_into(
                                input.clone(),
                                gpui::PathPromptOptions {
                                    files: false,
                                    directories: true,
                                    multiple: false,
                                    prompt: Some("Choose".into()),
                                },
                                window,
                                cx,
                            );
                        }
                    })),
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(labelled("File name", Input::new(&self.file_name_input)))
                    .child(note(
                        match &file_name {
                            Ok(name) => format!(
                                "Saved as {name}. {{database}}, {{connection}}, {{date}} and {{time}} are filled in."
                            ),
                            Err(error) => format!("{error:#}"),
                        },
                        if file_name.is_ok() {
                            cx.theme().muted_foreground
                        } else {
                            cx.theme().danger
                        },
                    )),
            )
            .child(labelled("Tables", self.render_tables(cx)))
            .child(labelled(
                "Extra arguments",
                Input::new(&self.extra_arguments_input),
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(note(error, cx.theme().danger))
            })
            .into_any_element()
    }
}

pub struct RestoreDialog {
    focus_handle: FocusHandle,
    context: ConnectionContext,
    tools: ClientTools,
    file_input: Entity<InputState>,
    extra_arguments_input: Entity<InputState>,
    dump: Option<Result<DumpFile, String>>,
    checks: Vec<(ToolProgram, Lookup<ToolCheck>)>,
    confirming_prod: bool,
    starting: bool,
    error: Option<String>,
    run: Option<ToolRun>,
    _subscriptions: Vec<Subscription>,
}

impl HostsRun for RestoreDialog {
    fn tool_run(&mut self) -> Option<&mut ToolRun> {
        self.run.as_mut()
    }
}

impl RestoreDialog {
    pub fn new(
        context: ConnectionContext,
        tools: ClientTools,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("A .sql, .sql.gz or .dump file"));
        let extra_arguments_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Passed to the tool as is"));
        let subscriptions = vec![
            cx.subscribe(&file_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.detect(cx);
                }
            }),
        ];
        let programs = ToolProgram::restore(tools);
        let checks = programs
            .iter()
            .map(|program| (*program, Lookup::Pending))
            .collect();

        let db_service = DatabaseService::global(cx).clone();
        let settings = AppSettings::global(cx).settings.client_tools.clone();
        cx.spawn_in(window, {
            let context = context.clone();
            async move |this, cx| {
                let server = server_version(&db_service, &context, tools).await;
                let mut checks = Vec::new();
                for program in programs {
                    let check = match &server {
                        Ok(server) => check_tool(*program, server, &settings).await,
                        Err(error) => Err(anyhow!("Could not connect: {error:#}")),
                    };
                    checks.push((*program, Lookup::from_result(check)));
                }
                this.update(cx, |this, cx| {
                    this.checks = checks;
                    cx.notify();
                })
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            context,
            tools,
            file_input,
            extra_arguments_input,
            dump: None,
            checks,
            confirming_prod: false,
            starting: false,
            error: None,
            run: None,
            _subscriptions: subscriptions,
        }
    }

    fn detect(&mut self, cx: &mut Context<Self>) {
        let value = self.file_input.read(cx).value().trim().to_string();
        self.dump = if value.is_empty() {
            None
        } else {
            Some(detect_dump(Path::new(&value)).map_err(|error| format!("{error:#}")))
        };
        self.confirming_prod = false;
        self.error = None;
        cx.notify();
    }

    fn program(&self) -> Option<Result<ToolProgram, String>> {
        match self.dump.as_ref()? {
            Ok(dump) => Some(
                dump.program(self.tools)
                    .map_err(|error| format!("{error:#}")),
            ),
            Err(error) => Some(Err(error.clone())),
        }
    }

    fn check(&self, program: ToolProgram) -> Option<&Lookup<ToolCheck>> {
        self.checks
            .iter()
            .find(|(candidate, _)| *candidate == program)
            .map(|(_, check)| check)
    }

    fn can_start(&self) -> bool {
        !self.starting
            && self.run.is_none()
            && matches!(self.program(), Some(Ok(program))
                if self.check(program)
                    .and_then(Lookup::ready)
                    .is_some_and(|check| !check.compatibility.blocks()))
    }

    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start() {
            return;
        }
        let Some(Ok(dump)) = self.dump.clone() else {
            return;
        };
        let Some(Ok(program)) = self.program() else {
            return;
        };
        let Some(check) = self.check(program).and_then(Lookup::ready).cloned() else {
            return;
        };
        if self.context.is_prod() && !self.confirming_prod {
            self.confirming_prod = true;
            cx.notify();
            return;
        }
        let extra_arguments = match split_arguments(&self.extra_arguments_input.read(cx).value()) {
            Ok(arguments) => arguments,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };

        self.starting = true;
        self.error = None;
        cx.notify();

        let context = self.context.clone();
        let db_service = DatabaseService::global(cx).clone();
        cx.spawn_in(window, async move |this, cx| {
            let plan = async {
                let endpoint = db_service
                    .client_tool_endpoint(context.connection_id, &context.database_name)
                    .await?;
                let entries = if program == ToolProgram::PgRestore && !dump.gzip {
                    match capture_output(
                        &check.tool.path,
                        &["--list".to_string(), dump.path.display().to_string()],
                    )
                    .await
                    {
                        Ok(listing) => Some(count_archive_entries(&listing)),
                        Err(error) => {
                            tracing::warn!("Could not list the archive: {error:#}");
                            None
                        }
                    }
                } else {
                    None
                };
                restore_plan(&check.tool, &endpoint, &dump, &extra_arguments, entries)
            }
            .await;
            this.update_in(cx, |this, window, cx| {
                this.starting = false;
                this.confirming_prod = false;
                match plan {
                    Ok(plan) => {
                        let database = &context.database_name;
                        let file = dump
                            .path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| dump.path.display().to_string());
                        let messages = RunMessages {
                            finished: format!("Restored {file} into {database}"),
                            stopped: format!(
                                "Restore into {database} stopped, the database may be partly restored"
                            ),
                            failed: format!("Restore into {database} failed"),
                        };
                        this.run = Some(start_run(plan, messages, window, cx));
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
        })
        .detach();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &mut self.run {
            run.request_stop();
        }
        cx.notify();
    }

    fn cancel_confirmation(&mut self, cx: &mut Context<Self>) {
        self.confirming_prod = false;
        cx.notify();
    }

    pub fn footer(dialog: &Entity<Self>, cx: &App) -> DialogFooter {
        let this = dialog.read(cx);
        let footer = DialogFooter::new();
        let start = {
            let dialog = dialog.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                dialog.update(cx, |dialog, cx| dialog.start(window, cx))
            }
        };
        match &this.run {
            Some(run) if run.is_running() => footer
                .child(DialogClose::new().child(Button::new("restore-close").label("Close")))
                .child(
                    Button::new("restore-stop")
                        .label("Stop")
                        .disabled(run.state == RunState::Stopping)
                        .on_click({
                            let dialog = dialog.clone();
                            move |_, _, cx| dialog.update(cx, |dialog, cx| dialog.stop(cx))
                        }),
                ),
            Some(_) => footer.child(
                DialogClose::new().child(Button::new("restore-done").primary().label("Done")),
            ),
            None if this.confirming_prod => footer
                .child(
                    Button::new("restore-back")
                        .label("Back")
                        .outline()
                        .on_click({
                            let dialog = dialog.clone();
                            move |_, _, cx| {
                                dialog.update(cx, |dialog, cx| dialog.cancel_confirmation(cx))
                            }
                        }),
                )
                .child(
                    Button::new("restore-confirm")
                        .danger()
                        .label("Restore to PROD")
                        .loading(this.starting)
                        .on_click(start),
                ),
            None => footer
                .child(
                    DialogClose::new()
                        .child(Button::new("restore-cancel").label("Cancel").outline()),
                )
                .child(
                    Button::new("restore-start")
                        .primary()
                        .label(if this.context.is_prod() {
                            "Restore…"
                        } else {
                            "Restore"
                        })
                        .loading(this.starting)
                        .disabled(!this.can_start())
                        .on_click(start),
                ),
        }
    }

    fn render_confirmation(&self, cx: &App) -> impl IntoElement + use<> {
        let file = self
            .dump
            .as_ref()
            .and_then(|dump| dump.as_ref().ok())
            .map(|dump| dump.path.display().to_string())
            .unwrap_or_default();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Restore into PROD?"),
            )
            .child(div().text_sm().child(format!(
                "\"{}\" is tagged as a production connection. The statements in {file} will run against database \"{}\" and can overwrite or drop its objects.",
                self.context.connection_name, self.context.database_name
            )))
            .child(note(
                "A backup of the database first is the only way back.",
                cx.theme().muted_foreground,
            ))
    }
}

impl Focusable for RestoreDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RestoreDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(run) = &self.run {
            return run.render(cx).into_any_element();
        }
        if self.confirming_prod {
            return self.render_confirmation(cx).into_any_element();
        }
        let muted = cx.theme().muted_foreground;
        let detected = match &self.dump {
            None => None,
            Some(Ok(dump)) => Some(note(
                match dump.format {
                    DumpFormat::Directory => dump.description(),
                    DumpFormat::Plain | DumpFormat::Custom => {
                        format!("{}, {}", dump.description(), format_bytes(dump.size))
                    }
                },
                muted,
            )),
            Some(Err(error)) => Some(note(error.clone(), cx.theme().danger)),
        };
        let tool = match self.program() {
            Some(Ok(program)) => self
                .check(program)
                .map(|check| render_tool_check(program, check, cx).into_any_element()),
            Some(Err(error)) if self.dump.as_ref().is_some_and(|dump| dump.is_ok()) => {
                Some(note(error, cx.theme().danger).into_any_element())
            }
            _ => None,
        };
        v_flex()
            .gap_4()
            .child(div().text_sm().child(format!(
                "The dump runs against database \"{}\" on \"{}\". Objects it creates replace or collide with the ones already there.",
                self.context.database_name, self.context.connection_name
            )))
            .child(
                v_flex()
                    .gap_1()
                    .child(labelled(
                        "Dump",
                        h_flex()
                            .gap_2()
                            .child(Input::new(&self.file_input).flex_1())
                            .child(Button::new("restore-browse").label("Browse…").on_click({
                                let input = self.file_input.clone();
                                move |_, window, cx| {
                                    browse_into(
                                        input.clone(),
                                        gpui::PathPromptOptions {
                                            files: true,
                                            directories: true,
                                            multiple: false,
                                            prompt: Some("Restore".into()),
                                        },
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    ))
                    .children(detected),
            )
            .children(tool)
            .child(labelled(
                "Extra arguments",
                Input::new(&self.extra_arguments_input),
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(note(error, cx.theme().danger))
            })
            .into_any_element()
    }
}

/// Open the backup dialog for `context`'s database.
pub fn open_backup(context: ConnectionContext, window: &mut Window, cx: &mut App) {
    let Some(tools) = context.db_type.dialect().client_tools() else {
        return;
    };
    let title = format!("Back Up \"{}\"", context.database_name);
    let dialog = cx.new(|cx| BackupDialog::new(context, tools, window, cx));
    window.open_dialog(cx, move |modal, _, cx| {
        modal
            .title(title.clone())
            .w(px(600.))
            .overlay_closable(false)
            .child(dialog.clone())
            .footer(BackupDialog::footer(&dialog, cx))
    });
}

/// Open the restore dialog for `context`'s database.
pub fn open_restore(context: ConnectionContext, window: &mut Window, cx: &mut App) {
    let Some(tools) = context.db_type.dialect().client_tools() else {
        return;
    };
    let title = format!("Restore \"{}\"", context.database_name);
    let dialog = cx.new(|cx| RestoreDialog::new(context, tools, window, cx));
    window.open_dialog(cx, move |modal, _, cx| {
        modal
            .title(title.clone())
            .w(px(560.))
            .overlay_closable(false)
            .child(dialog.clone())
            .footer(RestoreDialog::footer(&dialog, cx))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
