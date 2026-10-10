use blanco_core::ConnectionContext;
use chrono::Local;
use database::{DatabaseService, DatabaseType};
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    IndexPath, WindowExt,
    button::{Button, ButtonVariants as _},
    dialog::{DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputState},
    notification::NotificationType,
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use super::service::ExportResult;
use transformers::{
    CsvTransformer, DataTransformer, JsonTransformer, MarkdownTransformer, SqlTransformer,
    TsvTransformer,
};

/// Export format options
#[derive(Clone, Debug, PartialEq)]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    Sql,
    Markdown,
}

impl ExportFormat {
    fn from_str(s: &str) -> Self {
        match s {
            "TSV" => ExportFormat::Tsv,
            "JSON" => ExportFormat::Json,
            "SQL" => ExportFormat::Sql,
            "Markdown" => ExportFormat::Markdown,
            _ => ExportFormat::Csv,
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::Sql => "sql",
            ExportFormat::Markdown => "md",
        }
    }
}

#[derive(Clone, Default)]
pub struct ExportOptions {
    /// Set to stop a running export.
    pub cancel: Arc<AtomicBool>,
}

/// What an export reads its rows from.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportSource {
    /// Every row of a table.
    Table {
        schema: Option<String>,
        table: String,
    },
    /// Every row of a read-only query, which runs again for the export.
    Query {
        sql: String,
        /// Names the file and the table of SQL INSERT output.
        name: String,
    },
}

impl ExportSource {
    /// The statement that streams the rows.
    pub fn select_query(&self) -> String {
        match self {
            Self::Table { schema, table } => {
                format!("SELECT * FROM {}", Self::qualified(schema, table))
            }
            Self::Query { sql, .. } => sql.trim().trim_end_matches(';').trim_end().to_string(),
        }
    }

    /// The name used for the default file name and SQL INSERT output.
    pub fn name(&self) -> String {
        match self {
            Self::Table { schema, table } => Self::qualified(schema, table),
            Self::Query { name, .. } => name.clone(),
        }
    }

    fn file_stem(&self) -> &str {
        match self {
            Self::Table { table, .. } => table,
            Self::Query { name, .. } => name,
        }
    }

    fn qualified(schema: &Option<String>, table: &str) -> String {
        match schema {
            Some(schema) => format!("{schema}.{table}"),
            None => table.to_string(),
        }
    }
}

pub struct ExportModal {
    focus_handle: FocusHandle,
    connection_id: i64,
    database_name: String,
    db_type: DatabaseType,
    source: ExportSource,
    format_select: Entity<SelectState<Vec<String>>>,
    directory_input: Entity<InputState>,
    filename_input: Entity<InputState>,
    options: ExportOptions,
    is_exporting: bool,
    /// The dialog went away while the export ran, so finishing must not
    /// close whatever dialog is open by then.
    dialog_closed: bool,
    exported_rows: Arc<AtomicUsize>,
    _progress_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    format_changed: AtomicBool,
}

impl ExportModal {
    pub fn new(
        context: ConnectionContext,
        table_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = ExportSource::Table {
            schema: context.schema_name.clone(),
            table: table_name,
        };
        Self::with_source(
            context.connection_id,
            context.database_name,
            context.db_type,
            source,
            window,
            cx,
        )
    }

    pub fn with_source(
        connection_id: i64,
        database_name: String,
        db_type: DatabaseType,
        source: ExportSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let formats = ["CSV", "TSV", "JSON", "SQL", "Markdown"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let format_select =
            cx.new(|cx| SelectState::new(formats.clone(), Some(IndexPath::new(0)), window, cx));

        // Initialize directory input
        let directory_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Export directory (e.g., /home/user/exports)")
        });

        // Generate default filename and prepopulate it
        let default_filename = generate_default_filename(source.file_stem(), &ExportFormat::Csv);
        let filename_input =
            cx.new(|cx| InputState::new(window, cx).default_value(&default_filename));

        // Set up subscription for format change events
        let subscription = cx.subscribe(
            &format_select,
            move |_modal, _format_select, event: &SelectEvent<Vec<String>>, cx| {
                // Just set a flag to indicate format changed
                match event {
                    SelectEvent::Confirm(_) => {
                        _modal.format_changed.store(true, Ordering::Relaxed);
                        cx.notify();
                    }
                }
            },
        );

        Self {
            focus_handle: cx.focus_handle(),
            connection_id,
            database_name,
            db_type,
            source,
            format_select,
            directory_input,
            filename_input,
            options: ExportOptions::default(),
            is_exporting: false,
            dialog_closed: false,
            exported_rows: Arc::new(AtomicUsize::new(0)),
            _progress_task: None,
            _subscriptions: vec![subscription],
            format_changed: AtomicBool::new(false),
        }
    }

    pub fn is_exporting(&self) -> bool {
        self.is_exporting
    }

    /// Stops a running export. Called when the dialog closes.
    pub fn cancel(&mut self) {
        self.dialog_closed = true;
        if self.is_exporting {
            self.options.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn get_selected_format(&self, cx: &App) -> ExportFormat {
        let selected = self
            .format_select
            .read(cx)
            .selected_value()
            .unwrap_or(&"CSV".to_string())
            .clone();
        ExportFormat::from_str(&selected)
    }

    pub fn get_full_file_path(&self, cx: &App) -> Result<PathBuf, String> {
        let directory = self.directory_input.read(cx).value();
        let filename = self.filename_input.read(cx).value();
        let format = self.get_selected_format(cx);

        if directory.is_empty() {
            return Err("Please select a directory".to_string());
        }

        if filename.is_empty() {
            return Err("Please enter a filename".to_string());
        }

        let dir_path = PathBuf::from(directory.as_ref());
        let mut file_path = dir_path.join(filename.as_ref());

        // Ensure the filename has the correct extension
        if file_path
            .extension()
            .is_none_or(|ext| ext != format.file_extension())
        {
            file_path.set_extension(format.file_extension());
        }

        Ok(file_path)
    }

    fn browse_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select export directory".into()),
        });

        let directory_input = self.directory_input.clone();
        cx.spawn_in(window, async move |_, window| {
            if let Some(path) = path.await.ok()?.ok()?
                && let Some(dir_path) = path.first()
                && let Some(dir_str) = dir_path.to_str()
            {
                window
                    .update(|window, cx| {
                        directory_input.update(cx, |input, cx| {
                            input.set_value(dir_str.to_string(), window, cx);
                        });
                    })
                    .ok();
            }
            Some(())
        })
        .detach();
    }

    pub fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_exporting {
            return;
        }

        let file_path = match self.get_full_file_path(cx) {
            Ok(file_path) => file_path,
            Err(error) => {
                window
                    .push_notification((NotificationType::Warning, SharedString::from(error)), cx);
                return;
            }
        };
        let format = self.get_selected_format(cx);
        self.options = ExportOptions::default();
        let options = self.options.clone();

        self.is_exporting = true;
        self.exported_rows.store(0, Ordering::Relaxed);
        cx.notify();

        // The row count is written from the export task, so redraw on a timer
        // while it runs.
        self._progress_task = Some(cx.spawn(async move |modal, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let running = modal
                    .update(cx, |modal, cx| {
                        cx.notify();
                        modal.is_exporting
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        }));

        let connection_id = self.connection_id;
        let database_name = self.database_name.clone();
        let db_type = self.db_type;
        let select_query = self.source.select_query();
        let source_name = self.source.name();
        let exported_rows = self.exported_rows.clone();
        let db_service = DatabaseService::global(cx).clone();

        cx.spawn_in(window, async move |entity, window| {
            let export_path = file_path.clone();
            let result = window
                .background_spawn(async move {
                    let connection = db_service
                        .get_or_create_connection(connection_id, None)
                        .await?;

                    let transformer: Box<dyn DataTransformer> = match format {
                        ExportFormat::Csv => Box::new(CsvTransformer),
                        ExportFormat::Tsv => Box::new(TsvTransformer),
                        ExportFormat::Markdown => Box::new(MarkdownTransformer),
                        ExportFormat::Json => Box::new(JsonTransformer::new()),
                        ExportFormat::Sql => {
                            Box::new(SqlTransformer::with_table_name(source_name, db_type))
                        }
                    };

                    super::service::ExportService::new()
                        .export_data_streaming_with_transformer(
                            connection.as_ref(),
                            &select_query,
                            Some(database_name.as_str()),
                            transformer.as_ref(),
                            &export_path,
                            &options,
                            move |progress| {
                                exported_rows.store(progress.exported_rows, Ordering::Relaxed);
                            },
                        )
                        .await
                })
                .await;

            let message = match &result {
                Ok(ExportResult::Success {
                    file_path,
                    rows_exported,
                    ..
                }) => {
                    tracing::info!("Export completed: {rows_exported} rows -> {file_path}");
                    Some((
                        NotificationType::Success,
                        format!("Exported {rows_exported} rows to {file_path}"),
                    ))
                }
                Ok(ExportResult::Cancelled) => {
                    // A cancelled export leaves a partial file behind; it is
                    // not something to keep.
                    let _ = smol::fs::remove_file(&file_path).await;
                    None
                }
                Err(error) => {
                    tracing::error!("Export failed: {error:#}");
                    Some((NotificationType::Error, format!("Export failed: {error:#}")))
                }
            };

            entity
                .update_in(window, |modal, window, cx| {
                    modal.is_exporting = false;
                    if let Some((kind, message)) = message {
                        window.push_notification((kind, SharedString::from(message)), cx);
                    }
                    if !modal.dialog_closed {
                        window.close_dialog(cx);
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn render_progress(&self) -> impl IntoElement {
        div().text_sm().when(self.is_exporting, |this| {
            let rows = self.exported_rows.load(Ordering::Relaxed);
            this.child(format!("Exporting… {rows} rows written"))
        })
    }
}

impl Focusable for ExportModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ExportModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check if format changed and update filename
        if self
            .format_changed
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            let export_format = self.get_selected_format(cx);
            let new_filename = generate_default_filename(self.source.file_stem(), &export_format);

            self.filename_input.update(cx, |input, cx| {
                input.set_value(&new_filename, window, cx);
            });
        }

        v_flex()
            .gap_4()
            .max_h(px(500.0))
            .p_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Export Format"))
                    .child(Select::new(&self.format_select)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Export Directory"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Input::new(&self.directory_input).flex_1())
                            .child(Button::new("browse-btn").label("Browse").on_click(
                                cx.listener(|modal: &mut Self, _event, window, cx| {
                                    modal.browse_directory(window, cx);
                                }),
                            )),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("File Name"))
                    .child(Input::new(&self.filename_input)),
            )
            .child(self.render_progress())
            .overflow_y_scrollbar()
    }
}

/// Opens the export dialog for `modal`. Closing the dialog while the export
/// runs cancels it.
pub fn open_export_dialog(
    title: &'static str,
    modal: Entity<ExportModal>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _window, cx| {
        let is_exporting = modal.read(cx).is_exporting();
        dialog
            .title(title)
            .h(px(450.0))
            .child(modal.clone())
            .on_close({
                let modal = modal.clone();
                move |_, _, cx| modal.update(cx, |modal, _| modal.cancel())
            })
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new()
                            .child(Button::new("export-cancel").label("Cancel").outline()),
                    )
                    .child(
                        Button::new("export-submit")
                            .primary()
                            .label("Export")
                            .loading(is_exporting)
                            .on_click({
                                let modal = modal.clone();
                                move |_, window, cx| {
                                    modal.update(cx, |modal, cx| modal.start_export(window, cx));
                                }
                            }),
                    ),
            )
    });
}

/// Generate a default filename based on table name and current timestamp
fn generate_default_filename(table_name: &str, format: &ExportFormat) -> String {
    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    format!("{}_{}.{}", table_name, timestamp, format.file_extension())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_default_filename() {
        let filename = generate_default_filename("users", &ExportFormat::Csv);
        assert!(filename.starts_with("users_"));
        assert!(filename.ends_with(".csv"));

        let filename = generate_default_filename("users", &ExportFormat::Json);
        assert!(filename.starts_with("users_"));
        assert!(filename.ends_with(".json"));

        let filename = generate_default_filename("test_table", &ExportFormat::Sql);
        assert!(filename.starts_with("test_table_"));
        assert!(filename.ends_with(".sql"));
    }
}
