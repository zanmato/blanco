use blanco_core::ConnectionContext;
use chrono::Local;
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, WindowExt,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
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
use transformers::{
    BinaryEncoding, CsvOptions, CsvQuoting, CsvTransformer, DataTransformer, HtmlTransformer,
    JsonTransformer, MarkdownTransformer, SqlTransformer, TextEncoding, TsvTransformer,
};

use super::service::ExportResult;
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt as _;
use crate::settings::CsvDelimiter;

/// Export format options
#[derive(Clone, Debug, PartialEq)]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    Sql,
    Markdown,
    Html,
}

impl ExportFormat {
    fn from_str(s: &str) -> Self {
        match s {
            "TSV" => ExportFormat::Tsv,
            "JSON" => ExportFormat::Json,
            "SQL" => ExportFormat::Sql,
            "Markdown" => ExportFormat::Markdown,
            "HTML" => ExportFormat::Html,
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
            ExportFormat::Html => "html",
        }
    }
}

#[derive(Clone, Default)]
pub struct ExportOptions {
    pub encoding: TextEncoding,
    /// Set to stop a running export.
    pub cancel: Arc<AtomicBool>,
}

type OptionSelect = Entity<SelectState<Vec<String>>>;

/// A select over `all`, labelled with `label`, starting at `selected`.
fn option_select<T: Copy + PartialEq>(
    all: &[T],
    label: fn(T) -> &'static str,
    selected: T,
    window: &mut Window,
    cx: &mut App,
) -> OptionSelect {
    let labels = all.iter().map(|item| label(*item).to_string()).collect();
    let selected = all.iter().position(|item| *item == selected).unwrap_or(0);
    cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(selected)), window, cx))
}

/// The item of `all` the select has chosen.
fn chosen<T: Copy + Default>(select: &OptionSelect, all: &[T], cx: &App) -> T {
    select
        .read(cx)
        .selected_index(cx)
        .and_then(|index| all.get(index.row).copied())
        .unwrap_or_default()
}

/// The CSV options in the dialog, filled in from the Copy as CSV settings.
struct CsvControls {
    delimiter: OptionSelect,
    custom_delimiter: Entity<InputState>,
    header: bool,
    null_text: Entity<InputState>,
    quote: Entity<InputState>,
    quoting: OptionSelect,
    replace_line_breaks: bool,
    line_break_replacement: Entity<InputState>,
}

impl CsvControls {
    fn new(window: &mut Window, cx: &mut App) -> Self {
        let settings = AppSettings::global(cx).settings.csv.clone();
        let input = |value: &str, window: &mut Window, cx: &mut App| {
            let value = value.to_string();
            cx.new(|cx| InputState::new(window, cx).default_value(value))
        };
        Self {
            delimiter: option_select(
                &CsvDelimiter::ALL,
                CsvDelimiter::label,
                settings.delimiter,
                window,
                cx,
            ),
            custom_delimiter: input(&settings.custom_delimiter, window, cx),
            header: settings.header,
            null_text: input(&settings.null_text, window, cx),
            quote: input(&settings.quote, window, cx),
            quoting: option_select(
                &CsvQuoting::ALL,
                CsvQuoting::label,
                settings.quoting,
                window,
                cx,
            ),
            replace_line_breaks: settings.replace_line_breaks,
            line_break_replacement: input(&settings.line_break_replacement, window, cx),
        }
    }

    fn options(&self, cx: &App) -> CsvOptions {
        let custom = self.custom_delimiter.read(cx).value();
        CsvOptions {
            delimiter: chosen(&self.delimiter, &CsvDelimiter::ALL, cx).char(&custom),
            header: self.header,
            null_text: self.null_text.read(cx).value().to_string(),
            quote: self.quote.read(cx).value().chars().next().unwrap_or('"'),
            quoting: chosen(&self.quoting, &CsvQuoting::ALL, cx),
            line_break_replacement: self
                .replace_line_breaks
                .then(|| self.line_break_replacement.read(cx).value().to_string()),
        }
    }
}

pub struct ExportModal {
    focus_handle: FocusHandle,
    context: ConnectionContext,
    table_name: String,
    format_select: Entity<SelectState<Vec<String>>>,
    directory_input: Entity<InputState>,
    filename_input: Entity<InputState>,
    encoding_select: OptionSelect,
    /// How CSV and JSON write binary columns.
    binary_select: OptionSelect,
    csv: CsvControls,
    /// SQL output as one INSERT holding every row.
    sql_multi_row: bool,
    is_exporting: bool,
    /// Rows written so far, counted by the export task.
    exported_rows: Arc<AtomicUsize>,
    /// Stops the running export when set.
    cancel: Arc<AtomicBool>,
    /// The dialog went away while the export ran, so finishing must not
    /// close whatever dialog is open by then.
    dialog_closed: bool,
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
        let formats = ["CSV", "TSV", "JSON", "SQL", "Markdown", "HTML"]
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
        let default_filename = generate_default_filename(&table_name, &ExportFormat::Csv);
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

        let csv = CsvControls::new(window, cx);
        // The custom delimiter field is enabled by the delimiter choice.
        let delimiter_subscription = cx
            .subscribe(&csv.delimiter, |_, _, _: &SelectEvent<Vec<String>>, cx| {
                cx.notify()
            });

        Self {
            focus_handle: cx.focus_handle(),
            context,
            table_name,
            format_select,
            directory_input,
            filename_input,
            encoding_select: option_select(
                &TextEncoding::ALL,
                TextEncoding::label,
                TextEncoding::default(),
                window,
                cx,
            ),
            binary_select: option_select(
                &BinaryEncoding::ALL,
                BinaryEncoding::label,
                BinaryEncoding::default(),
                window,
                cx,
            ),
            csv,
            sql_multi_row: true,
            is_exporting: false,
            exported_rows: Arc::new(AtomicUsize::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            dialog_closed: false,
            _progress_task: None,
            _subscriptions: vec![subscription, delimiter_subscription],
            format_changed: AtomicBool::new(false),
        }
    }

    pub fn is_exporting(&self) -> bool {
        self.is_exporting
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

    /// Stops a running export. Called when the dialog closes.
    pub fn cancel(&mut self) {
        self.dialog_closed = true;
        if self.is_exporting {
            self.cancel.store(true, Ordering::Relaxed);
        }
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
        self.cancel = Arc::new(AtomicBool::new(false));
        let options = ExportOptions {
            encoding: chosen(&self.encoding_select, &TextEncoding::ALL, cx),
            cancel: Arc::clone(&self.cancel),
        };
        let csv_options = self.csv.options(cx);
        let binary = chosen(&self.binary_select, &BinaryEncoding::ALL, cx);
        let sql_multi_row = self.sql_multi_row;

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

        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let schema_name = self.context.schema_name.clone();
        let table_name = self.table_name.clone();
        let db_type = self.context.db_type;
        // Quoted, so a name with capitals, spaces or a reserved word is read
        // as the table it is.
        let select_query = format!(
            "SELECT * FROM {}",
            db_type
                .dialect()
                .quote_qualified(schema_name.as_deref(), &table_name)
        );
        let exported_rows = Arc::clone(&self.exported_rows);
        let db_service = DatabaseService::global(cx).clone();

        cx.spawn_in(window, async move |entity, window| {
            let export_path = file_path.clone();
            let result = window
                .background_spawn(async move {
                    let connection = db_service
                        .get_or_create_connection(connection_id, None)
                        .await?;

                    let transformer: Box<dyn DataTransformer> = match format {
                        ExportFormat::Csv => {
                            Box::new(CsvTransformer::new(csv_options).binary_encoding(binary))
                        }
                        ExportFormat::Tsv => Box::new(TsvTransformer),
                        ExportFormat::Markdown => Box::new(MarkdownTransformer),
                        ExportFormat::Html => Box::new(HtmlTransformer),
                        ExportFormat::Json => {
                            Box::new(JsonTransformer::new().binary_encoding(binary))
                        }
                        ExportFormat::Sql => {
                            let table_name_for_sql = match &schema_name {
                                Some(schema) => format!("{schema}.{table_name}"),
                                None => table_name.clone(),
                            };
                            Box::new(
                                SqlTransformer::with_table_name(table_name_for_sql, db_type)
                                    .multi_row(sql_multi_row),
                            )
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
                }) => Some((
                    NotificationType::Success,
                    format!("Exported {rows_exported} rows to {file_path}"),
                )),
                // A cancelled or failed export leaves a partial file behind,
                // which is not something to keep.
                Ok(ExportResult::Cancelled) => {
                    remove_partial_file(&file_path).await;
                    None
                }
                Err(error) => {
                    remove_partial_file(&file_path).await;
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
                .log_err();
        })
        .detach();
    }

    /// The total is not known while rows stream in, so this counts up.
    fn render_progress(&self, cx: &App) -> impl IntoElement {
        div().when(self.is_exporting, |this| {
            this.text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "Exporting… {} rows written. Close the dialog to stop.",
                    self.exported_rows.load(Ordering::Relaxed)
                ))
        })
    }
}

async fn remove_partial_file(path: &std::path::Path) {
    if let Err(error) = smol::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::error!("Failed to remove {}: {error}", path.display());
    }
}

/// Opens the export dialog for `modal`. Closing the dialog while the export
/// runs cancels it.
pub fn open_export_dialog(modal: Entity<ExportModal>, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _window, cx| {
        let is_exporting = modal.read(cx).is_exporting();
        dialog
            .title("Export Table Data")
            .h(px(600.0))
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

impl ExportModal {
    fn render_csv_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let csv = &self.csv;
        let custom_delimiter =
            chosen(&csv.delimiter, &CsvDelimiter::ALL, cx) == CsvDelimiter::Custom;
        let field = |label: &'static str, control: gpui::AnyElement| {
            v_flex()
                .gap_2()
                .flex_1()
                .child(div().text_sm().child(label))
                .child(control)
        };

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(field(
                        "Delimiter",
                        Select::new(&csv.delimiter).into_any_element(),
                    ))
                    .child(field(
                        "Custom Delimiter",
                        Input::new(&csv.custom_delimiter)
                            .disabled(!custom_delimiter)
                            .into_any_element(),
                    )),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(field(
                        "Quote Values",
                        Select::new(&csv.quoting).into_any_element(),
                    ))
                    .child(field(
                        "Quote Character",
                        Input::new(&csv.quote).into_any_element(),
                    )),
            )
            .child(field(
                "NULL Text",
                Input::new(&csv.null_text).into_any_element(),
            ))
            .child(
                Checkbox::new("export-csv-header")
                    .label("Header row")
                    .checked(csv.header)
                    .on_click(cx.listener(|modal, checked: &bool, _, cx| {
                        modal.csv.header = *checked;
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Checkbox::new("export-csv-replace-line-breaks")
                            .label("Replace line breaks with")
                            .checked(csv.replace_line_breaks)
                            .on_click(cx.listener(|modal, checked: &bool, _, cx| {
                                modal.csv.replace_line_breaks = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Input::new(&csv.line_break_replacement)
                            .disabled(!csv.replace_line_breaks)
                            .flex_1(),
                    ),
            )
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
            let new_filename = generate_default_filename(&self.table_name, &export_format);

            self.filename_input.update(cx, |input, cx| {
                input.set_value(&new_filename, window, cx);
            });
        }

        let format = self.get_selected_format(cx);
        v_flex()
            .gap_4()
            .max_h(px(440.0))
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
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Encoding"))
                    .child(Select::new(&self.encoding_select)),
            )
            .when(
                matches!(format, ExportFormat::Csv | ExportFormat::Json),
                |this| {
                    this.child(
                        v_flex()
                            .gap_2()
                            .child(div().text_sm().child("Binary Values"))
                            .child(Select::new(&self.binary_select)),
                    )
                },
            )
            .map(|this| match format {
                ExportFormat::Csv => this.child(self.render_csv_options(cx)),
                ExportFormat::Sql => this.child(
                    Checkbox::new("export-sql-multi-row")
                        .label("One INSERT for all rows")
                        .checked(self.sql_multi_row)
                        .on_click(cx.listener(|modal, checked: &bool, _, cx| {
                            modal.sql_multi_row = *checked;
                            cx.notify();
                        })),
                ),
                _ => this,
            })
            .child(self.render_progress(cx))
            .overflow_y_scrollbar()
    }
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

    /// The dialog draws its CSV options, and the SQL option once SQL is
    /// chosen, from the Copy as CSV settings.
    #[gpui::test]
    async fn test_dialog_draws_the_format_options(cx: &mut gpui::TestAppContext) {
        let harness = crate::test_harness::TestHarness::new(cx);
        let mut cx = gpui::VisualTestContext::from_window(harness.window_handle.into(), cx);

        cx.update(|_, cx| AppSettings::global_mut(cx).settings.csv.delimiter = CsvDelimiter::Pipe);
        let modal = cx.update(|window, cx| {
            let context = ConnectionContext {
                connection_id: harness.connection_id,
                connection_name: "test".into(),
                db_type: database::DatabaseType::SQLite,
                database_name: "main".into(),
                schema_name: None,
                environment_type: None,
            };
            let modal = cx.new(|cx| ExportModal::new(context, "items".into(), window, cx));
            let content = modal.clone();
            window.open_dialog(cx, move |dialog, _, _| dialog.child(content.clone()));
            modal
        });
        cx.run_until_parked();

        modal.read_with(&cx, |modal, cx| {
            assert_eq!(modal.csv.options(cx).delimiter, '|');
            assert_eq!(modal.get_selected_format(cx), ExportFormat::Csv);
        });

        modal.update_in(&mut cx, |modal, window, cx| {
            modal.format_select.update(cx, |select, cx| {
                select.set_selected_index(Some(IndexPath::new(3)), window, cx)
            });
        });
        cx.run_until_parked();
        modal.read_with(&cx, |modal, cx| {
            assert_eq!(modal.get_selected_format(cx), ExportFormat::Sql);
        });
    }
}
