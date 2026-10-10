//! The Export as SQL dialog: pick tables, choose structure and data per
//! table, and write one SQL file.

use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use blanco_core::ConnectionContext;
use database::DatabaseService;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    v_flex,
};

use super::sql_dump::{DumpOptions, DumpProgress, DumpTable, write_dump};
use crate::result_ext::ResultExt as _;

struct TableRow {
    schema: Option<String>,
    name: String,
    include: bool,
    structure: bool,
    data: bool,
}

impl TableRow {
    fn label(&self, show_schema: bool) -> String {
        match (&self.schema, show_schema) {
            (Some(schema), true) => format!("{schema}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

enum Status {
    Loading,
    Ready,
    Running(Arc<DumpProgress>, usize),
    Done(SharedString),
    Failed(SharedString),
}

pub struct SqlDumpModal {
    context: ConnectionContext,
    rows: Vec<TableRow>,
    /// Tables from several schemas are listed with their schema.
    show_schema: bool,
    drop_existing: bool,
    disable_foreign_keys: bool,
    rows_per_insert: Entity<InputState>,
    status: Status,
    focus_handle: FocusHandle,
}

impl SqlDumpModal {
    /// List the tables of `schema`, or of every schema when `None`, with
    /// only `preselect` included when one is given.
    pub fn new(
        context: ConnectionContext,
        schema: Option<String>,
        preselect: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows_per_insert = cx.new(|cx| InputState::new(window, cx).default_value("100"));
        let db_service = DatabaseService::global(cx).clone();
        let connection_id = context.connection_id;
        let database = context.database_name.clone();
        let load = gpui_tokio::Tokio::spawn_result(cx, async move {
            let connection = db_service
                .get_or_create_connection(connection_id, Some(&database))
                .await?;
            let schemas = match schema {
                Some(schema) => vec![Some(schema)],
                None if connection.supports_schemas() => connection
                    .get_schemas()
                    .await?
                    .into_iter()
                    .map(Some)
                    .collect(),
                None => vec![None],
            };
            let mut tables = Vec::new();
            for schema in schemas {
                for table in connection.get_tables(schema.as_deref()).await? {
                    tables.push((schema.clone(), table));
                }
            }
            anyhow::Ok(tables)
        });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(tables) => {
                        this.show_schema = tables
                            .iter()
                            .filter_map(|(schema, _)| schema.as_ref())
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            > 1;
                        this.rows = tables
                            .into_iter()
                            .map(|(schema, name)| TableRow {
                                include: preselect.as_ref().is_none_or(|table| *table == name),
                                schema,
                                name,
                                structure: true,
                                data: true,
                            })
                            .collect();
                        this.status = Status::Ready;
                    }
                    Err(error) => this.status = Status::Failed(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();

        Self {
            context,
            rows: Vec::new(),
            show_schema: false,
            drop_existing: false,
            disable_foreign_keys: false,
            rows_per_insert,
            status: Status::Loading,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, Status::Running(..))
    }

    /// Stop a running export; returns whether one was running.
    pub fn cancel(&mut self) -> bool {
        if let Status::Running(progress, _) = &self.status {
            progress.cancelled.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn selected(&self) -> Vec<DumpTable> {
        self.rows
            .iter()
            .filter(|row| row.include && (row.structure || row.data))
            .map(|row| DumpTable {
                schema: row.schema.clone(),
                name: row.name.clone(),
                structure: row.structure,
                data: row.data,
            })
            .collect()
    }

    fn set_all(&mut self, include: bool, cx: &mut Context<Self>) {
        for row in &mut self.rows {
            row.include = include;
        }
        cx.notify();
    }

    /// Ask for the file and write the dump.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_running() {
            return;
        }
        let tables = self.selected();
        if tables.is_empty() {
            self.status = Status::Failed("Pick at least one table with structure or data".into());
            cx.notify();
            return;
        }
        let rows_per_insert = match self
            .rows_per_insert
            .read(cx)
            .value()
            .trim()
            .parse::<usize>()
        {
            Ok(rows) if rows > 0 => rows,
            _ => {
                self.status = Status::Failed("Rows per INSERT must be a positive number".into());
                cx.notify();
                return;
            }
        };
        let options = DumpOptions {
            drop_existing: self.drop_existing,
            disable_foreign_keys: self.disable_foreign_keys,
            rows_per_insert,
        };
        let directory = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let suggested = format!("{}.sql", self.context.database_name);
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = path.await.ok().and_then(Result::ok).flatten() else {
                return;
            };
            this.update(cx, |this, cx| this.run(path, tables, options, cx))
                .log_err();
        })
        .detach();
    }

    fn run(
        &mut self,
        path: PathBuf,
        tables: Vec<DumpTable>,
        options: DumpOptions,
        cx: &mut Context<Self>,
    ) {
        let progress = Arc::new(DumpProgress::default());
        self.status = Status::Running(progress.clone(), tables.len());
        cx.notify();

        let db_service = DatabaseService::global(cx).clone();
        let context = self.context.clone();
        let file_path = path.clone();
        let dump = gpui_tokio::Tokio::spawn_result(cx, {
            async move {
                let connection = db_service
                    .get_or_create_connection(context.connection_id, Some(&context.database_name))
                    .await?;
                let file = std::fs::File::create(&file_path)?;
                let mut out = BufWriter::new(file);
                let result = write_dump(
                    connection.as_ref(),
                    context.dialect(),
                    &context.database_name,
                    &tables,
                    &options,
                    &mut out,
                    &progress,
                )
                .await;
                if result.is_err() {
                    // A partial dump is worse than none.
                    drop(out);
                    std::fs::remove_file(&file_path).ok();
                }
                result
            }
        });
        cx.spawn(async move |this, cx| {
            let ticker = cx.spawn({
                let this = this.clone();
                async move |cx| loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            });
            let result = dump.await;
            drop(ticker);
            this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(summary) => Status::Done(
                        format!(
                            "Exported {} table(s) and {} row(s) to {}",
                            summary.tables,
                            summary.rows,
                            path.display()
                        )
                        .into(),
                    ),
                    Err(error) => Status::Failed(format!("{error:#}").into()),
                };
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn render_row(&self, ix: usize, row: &TableRow, cx: &mut Context<Self>) -> gpui::AnyElement {
        let structure_supported = self.context.dialect().supports_create_table_ddl();
        h_flex()
            .gap_3()
            .py_0p5()
            .child(
                div().flex_1().min_w_0().child(
                    Checkbox::new(("dump-include", ix))
                        .label(row.label(self.show_schema))
                        .checked(row.include)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(row) = this.rows.get_mut(ix) {
                                row.include = *checked;
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(
                div().w(px(90.)).child(
                    Checkbox::new(("dump-structure", ix))
                        .label("Structure")
                        .checked(row.structure && structure_supported)
                        .disabled(!row.include || !structure_supported)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(row) = this.rows.get_mut(ix) {
                                row.structure = *checked;
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(
                div().w(px(70.)).child(
                    Checkbox::new(("dump-data", ix))
                        .label("Data")
                        .checked(row.data)
                        .disabled(!row.include)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(row) = this.rows.get_mut(ix) {
                                row.data = *checked;
                            }
                            cx.notify();
                        })),
                ),
            )
            .into_any_element()
    }
}

impl Focusable for SqlDumpModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SqlDumpModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let border = theme.border;
        let structure_supported = self.context.dialect().supports_create_table_ddl();
        let included = self.rows.iter().filter(|row| row.include).count();

        v_flex()
            .track_focus(&self.focus_handle)
            .gap_3()
            .size_full()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_sm().text_color(muted).child(format!(
                        "{included} of {} table(s) from {}",
                        self.rows.len(),
                        self.context.database_name
                    )))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("dump-select-all")
                                    .ghost()
                                    .xsmall()
                                    .label("Select All")
                                    .on_click(cx.listener(|this, _, _, cx| this.set_all(true, cx))),
                            )
                            .child(
                                Button::new("dump-select-none")
                                    .ghost()
                                    .xsmall()
                                    .label("Select None")
                                    .on_click(cx.listener(|this, _, _, cx| this.set_all(false, cx))),
                            ),
                    ),
            )
            .child(
                div()
                    .id("dump-tables")
                    .flex_1()
                    .min_h(px(160.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded(theme.radius)
                    .px_2()
                    .py_1()
                    .map(|this| match self.status {
                        Status::Loading => this.child(
                            div().text_sm().text_color(muted).child("Loading tables…"),
                        ),
                        _ if self.rows.is_empty() => this.child(
                            div().text_sm().text_color(muted).child("No tables"),
                        ),
                        _ => this.children(
                            (0..self.rows.len())
                                .map(|ix| self.render_row(ix, &self.rows[ix], cx))
                                .collect::<Vec<_>>(),
                        ),
                    }),
            )
            .when(!structure_supported, |this| {
                this.child(div().text_xs().text_color(muted).child(
                    "This database cannot produce CREATE TABLE statements here, so the export holds data only.",
                ))
            })
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        Checkbox::new("dump-drop-existing")
                            .label("Add DROP TABLE IF EXISTS before each CREATE TABLE")
                            .checked(self.drop_existing)
                            .disabled(!structure_supported)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.drop_existing = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("dump-disable-foreign-keys")
                            .label("Turn foreign key checks off while loading")
                            .checked(self.disable_foreign_keys)
                            .disabled(self.context.dialect().foreign_key_check_toggle().is_none())
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.disable_foreign_keys = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        div().text_xs().text_color(muted).child(
                            "Tables are written after the tables they reference. When they reference each other in a cycle, foreign key checks are turned off regardless.",
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_sm().child("Rows per INSERT"))
                            .child(div().w(px(90.)).child(Input::new(&self.rows_per_insert).small())),
                    ),
            )
            .map(|this| match &self.status {
                Status::Running(progress, total) => this.child(
                    div().text_sm().text_color(muted).child(format!(
                        "{} {} of {total} table(s) · {} row(s)",
                        if progress.cancelled.load(Ordering::Relaxed) { "Cancelling…" } else { "Exporting" },
                        progress.tables_done.load(Ordering::Relaxed).min(*total),
                        progress.rows.load(Ordering::Relaxed)
                    )),
                ),
                Status::Done(message) => {
                    this.child(div().text_sm().text_color(muted).child(message.clone()))
                }
                Status::Failed(error) => {
                    this.child(div().text_sm().text_color(danger).child(error.clone()))
                }
                _ => this,
            })
    }
}
