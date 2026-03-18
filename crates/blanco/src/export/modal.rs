use chrono::Utc;
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Subscription, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, WindowExt,
    button::Button,
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Export format options
#[derive(Clone, Debug, PartialEq)]
pub enum ExportFormat {
    Csv,
    Json,
    Sql,
}

impl ExportFormat {
    fn from_str(s: &str) -> Self {
        match s {
            "JSON" => ExportFormat::Json,
            "SQL" => ExportFormat::Sql,
            _ => ExportFormat::Csv,
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Sql => "sql",
        }
    }
}

#[derive(Clone)]
#[derive(Default)]
pub struct ExportOptions {}


pub struct ExportModal {
    focus_handle: FocusHandle,
    connection_id: i64,
    database_name: String,
    schema_name: Option<String>,
    table_name: String,
    format_select: Entity<SelectState<Vec<String>>>,
    directory_input: Entity<InputState>,
    filename_input: Entity<InputState>,
    options: ExportOptions,
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
        connection_id: i64,
        database_name: String,
        schema_name: Option<String>,
        table_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let formats = vec!["CSV".to_string(), "JSON".to_string(), "SQL".to_string()];
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

        Self {
            focus_handle: cx.focus_handle(),
            connection_id,
            database_name,
            schema_name,
            table_name,
            format_select,
            directory_input,
            filename_input,
            options: ExportOptions::default(),
            is_exporting: false,
            export_progress: 0.0,
            exported_rows: 0,
            total_rows: 0, // We'll estimate this later
            is_cancelled: false,
            _subscriptions: vec![subscription],
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
            .extension().is_none_or(|ext| ext != format.file_extension())
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
                    && let Some(dir_str) = dir_path.to_str() {
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
                let options = self.options.clone();

                self.is_exporting = true;
                self.is_cancelled = false;
                self.export_progress = 0.0;
                self.exported_rows = 0;
                cx.notify();

                let connection_id = self.connection_id;
                let database_name = self.database_name.clone();
                let schema_name = self.schema_name.clone();
                let table_name_param = self.table_name.clone();

                let table_name_for_query = if let Some(schema) = &schema_name {
                    format!("{}.{}", schema, table_name_param)
                } else {
                    table_name_param.clone()
                };

                let select_query = format!("SELECT * FROM {}", table_name_for_query);

                let db_service = DatabaseService::global(cx).clone();

                cx.spawn_in(window, async move |entity, mut window| {
                    let result = window
                        .background_spawn(async move {
                            let connection = db_service
                                .get_or_create_connection(connection_id, None)
                                .await?;

                            let transformer: Box<dyn crate::transformers::DataTransformer> =
                                match format {
                                    ExportFormat::Csv => {
                                        Box::new(crate::transformers::CsvTransformer)
                                    }
                                    ExportFormat::Json => {
                                        Box::new(crate::transformers::JsonTransformer::new())
                                    }
                                    ExportFormat::Sql => {
                                        let table_name_for_sql =
                                            if let Some(schema) = &schema_name {
                                                format!("{}.{}", schema, table_name_param)
                                            } else {
                                                table_name_param.clone()
                                            };
                                        Box::new(
                                            crate::transformers::SqlTransformer::with_table_name(
                                                table_name_for_sql,
                                            ),
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
            .child(self.render_progress_bar(cx))
            .overflow_y_scrollbar()
    }
}

/// Generate a default filename based on table name and current timestamp
fn generate_default_filename(table_name: &str, format: &ExportFormat) -> String {
    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S");
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
