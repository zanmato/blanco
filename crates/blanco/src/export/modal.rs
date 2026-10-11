use blanco_core::ConnectionContext;
use chrono::Local;
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Subscription, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, WindowExt,
    button::Button,
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use transformers::{
    CsvOptions, CsvQuoting, CsvTransformer, DataTransformer, JsonTransformer, MarkdownTransformer,
    SqlTransformer, TextEncoding, TsvTransformer,
};

use crate::app_settings::AppSettings;
use crate::settings::CsvDelimiter;

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
    pub encoding: TextEncoding,
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
    csv: CsvControls,
    /// SQL output as one INSERT holding every row.
    sql_multi_row: bool,
    is_exporting: bool,
    export_progress: f32,
    exported_rows: usize,
    total_rows: usize,
    is_cancelled: bool,
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
            csv,
            sql_multi_row: true,
            is_exporting: false,
            export_progress: 0.0,
            exported_rows: 0,
            total_rows: 0, // We'll estimate this later
            is_cancelled: false,
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

    pub fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_exporting {
            return;
        }

        // Validate inputs
        match self.get_full_file_path(cx) {
            Ok(file_path) => {
                let format = self.get_selected_format(cx);
                let options = ExportOptions {
                    encoding: chosen(&self.encoding_select, &TextEncoding::ALL, cx),
                };
                let csv_options = self.csv.options(cx);
                let sql_multi_row = self.sql_multi_row;

                self.is_exporting = true;
                self.is_cancelled = false;
                self.export_progress = 0.0;
                self.exported_rows = 0;
                cx.notify();

                let connection_id = self.context.connection_id;
                let database_name = self.context.database_name.clone();
                let schema_name = self.context.schema_name.clone();
                let table_name_param = self.table_name.clone();
                let db_type = self.context.db_type;

                let table_name_for_query = if let Some(schema) = &schema_name {
                    format!("{}.{}", schema, table_name_param)
                } else {
                    table_name_param.clone()
                };

                let select_query = format!("SELECT * FROM {}", table_name_for_query);

                let db_service = DatabaseService::global(cx).clone();

                cx.spawn_in(window, async move |entity, window| {
                    let result = window
                        .background_spawn(async move {
                            let connection = db_service
                                .get_or_create_connection(connection_id, None)
                                .await?;

                            let transformer: Box<dyn DataTransformer> = match format {
                                ExportFormat::Csv => Box::new(CsvTransformer::new(csv_options)),
                                ExportFormat::Tsv => Box::new(TsvTransformer),
                                ExportFormat::Markdown => Box::new(MarkdownTransformer),
                                ExportFormat::Json => Box::new(JsonTransformer::new()),
                                ExportFormat::Sql => {
                                    let table_name_for_sql = if let Some(schema) = &schema_name {
                                        format!("{}.{}", schema, table_name_param)
                                    } else {
                                        table_name_param.clone()
                                    };
                                    Box::new(
                                        SqlTransformer::with_table_name(
                                            table_name_for_sql,
                                            db_type,
                                        )
                                        .multi_row(sql_multi_row),
                                    )
                                }
                            };

                            let export_service = super::service::ExportService::new();

                            export_service
                                .export_data_streaming_with_transformer(
                                    connection.as_ref(),
                                    &select_query,
                                    Some(database_name.as_str()),
                                    transformer.as_ref(),
                                    &file_path,
                                    &options,
                                    move |progress| {
                                        tracing::info!(
                                            "Export progress: {}/{} rows",
                                            progress.exported_rows,
                                            progress.total_rows
                                        );
                                    },
                                )
                                .await?;

                            tracing::info!(
                                "Export completed successfully: {} -> {}",
                                table_name_for_query,
                                file_path.display()
                            );
                            Ok::<(), anyhow::Error>(())
                        })
                        .await;

                    if let Err(e) = &result {
                        tracing::error!("Export failed: {}", e);
                    }

                    entity
                        .update_in(window, |modal, _, cx| {
                            modal.is_exporting = false;
                            cx.notify();
                        })
                        .ok();

                    window
                        .update(|window, cx| {
                            window.close_dialog(cx);
                        })
                        .ok();
                })
                .detach();
            }
            Err(error) => {
                tracing::error!("Export error: {}", error);
            }
        }
    }

    fn render_progress_bar(&self, cx: &App) -> impl IntoElement {
        if self.is_exporting {
            v_flex()
                .gap_2()
                .child(div().text_sm().child(format!(
                    "Exporting... {}/{} rows ({:.1}%)",
                    self.exported_rows,
                    self.total_rows,
                    self.export_progress * 100.0
                )))
                .child(
                    div()
                        .w_full()
                        .h_2()
                        .bg(cx.theme().muted_foreground.opacity(0.2))
                        .rounded(cx.theme().radius)
                        .child(
                            div()
                                .h_full()
                                .bg(cx.theme().primary)
                                .rounded(cx.theme().radius)
                                .when(self.export_progress > 0.0, |this| {
                                    this.w(px((200.0 * self.export_progress).max(1.0)))
                                }),
                        ),
                )
        } else {
            div() // Empty div when not exporting
        }
    }
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
            .child(self.render_progress_bar(cx))
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
