use crate::import::detect::{self, DetectedFile};
use crate::import::mapping::{
    ColumnMapping, ConflictStrategy, Transform, compile_mappings, preview_statements,
    quote_qualified,
};
use crate::import::service::{
    DEFAULT_BATCH_SIZE, ImportError, ImportReport, ImportRequest, run_import,
};
use crate::result_ext::ResultExt;
use blanco_core::ColumnInfo;
use database::{DatabaseService, DatabaseType};
use encoding_rs::Encoding;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, StyledText,
    Subscription, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dialog::DialogClose,
    h_flex,
    highlighter::SyntaxHighlighter,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const ENCODINGS: &[(&str, &str)] = &[
    ("Auto-detect", "auto"),
    ("UTF-8", "utf-8"),
    ("UTF-16 LE", "utf-16le"),
    ("UTF-16 BE", "utf-16be"),
    ("Windows-1252", "windows-1252"),
    ("ISO-8859-1", "iso-8859-1"),
    ("ISO-8859-15", "iso-8859-15"),
];

const DELIMITERS: &[(&str, u8)] = &[
    ("Auto-detect", 0),
    ("Comma ,", b','),
    ("Semicolon ;", b';'),
    ("Tab \\t", b'\t'),
    ("Pipe |", b'|'),
];

const CONFLICT_LABELS: &[&str] = &["Fail (abort + rollback)", "Do nothing", "Update"];

fn mapping_options(csv_headers: &[String]) -> Vec<String> {
    let mut options = vec!["Skip".to_string()];
    for (idx, name) in csv_headers.iter().enumerate() {
        options.push(format!("CSV: {} (#{})", name, idx + 1));
    }
    options.push("Fixed value".to_string());
    options
}

fn decode_mapping_index(
    selected: usize,
    csv_header_count: usize,
    fixed_value: &str,
) -> ColumnMapping {
    if selected == 0 {
        ColumnMapping::Skip
    } else if selected == csv_header_count + 1 {
        ColumnMapping::Fixed(fixed_value.to_string())
    } else {
        ColumnMapping::CsvColumn(selected - 1)
    }
}

fn effective_headers(detected: &DetectedFile) -> Vec<String> {
    if detected.headers.is_empty() {
        (0..detected.sample_rows.first().map(|r| r.len()).unwrap_or(0))
            .map(|i| format!("column_{}", i + 1))
            .collect()
    } else {
        detected.headers.clone()
    }
}

pub struct ImportModal {
    focus_handle: FocusHandle,
    connection_id: i64,
    database_name: String,
    schema_name: Option<String>,
    table_name: String,
    db_type: DatabaseType,

    file_input: Entity<InputState>,
    encoding_select: Entity<SelectState<Vec<String>>>,
    delimiter_select: Entity<SelectState<Vec<String>>>,
    conflict_select: Entity<SelectState<Vec<String>>>,
    conflict_keys_input: Entity<InputState>,

    has_header: bool,
    table_columns: Vec<ColumnInfo>,
    detected: Option<DetectedFile>,
    mapping_selects: Vec<Entity<SelectState<Vec<String>>>>,
    fixed_inputs: Vec<Entity<InputState>>,
    mappings: Vec<ColumnMapping>,
    transforms: Vec<Transform>,
    transform_selects: Vec<Entity<SelectState<Vec<String>>>>,
    update_column_checks: Vec<bool>,

    is_importing: bool,
    imported_rows: Arc<std::sync::atomic::AtomicU64>,
    import_result: Option<ImportReport>,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    _subscriptions: Vec<Subscription>,
    _mapping_subscriptions: Vec<Subscription>,
}

impl ImportModal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection_id: i64,
        database_name: String,
        schema_name: Option<String>,
        table_name: String,
        db_type: DatabaseType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("CSV file path (click Browse)"));

        let encoding_labels: Vec<String> = ENCODINGS.iter().map(|(l, _)| l.to_string()).collect();
        let encoding_select =
            cx.new(|cx| SelectState::new(encoding_labels, Some(IndexPath::new(0)), window, cx));

        let delimiter_labels: Vec<String> = DELIMITERS.iter().map(|(l, _)| l.to_string()).collect();
        let delimiter_select =
            cx.new(|cx| SelectState::new(delimiter_labels, Some(IndexPath::new(0)), window, cx));

        let conflict_labels: Vec<String> = CONFLICT_LABELS.iter().map(|l| l.to_string()).collect();
        let conflict_select =
            cx.new(|cx| SelectState::new(conflict_labels, Some(IndexPath::new(0)), window, cx));
        let conflict_keys_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Conflict keys (comma-separated)"));

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(
            &encoding_select,
            window,
            |modal, _s, _event: &SelectEvent<Vec<String>>, window, cx| {
                modal.redetect(window, cx);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &delimiter_select,
            window,
            |modal, _s, _event: &SelectEvent<Vec<String>>, window, cx| {
                modal.redetect(window, cx);
            },
        ));
        subscriptions.push(cx.subscribe(
            &conflict_select,
            |_modal, _s, _event: &SelectEvent<Vec<String>>, cx| {
                cx.notify();
            },
        ));

        cx.spawn_in(window, async move |handle, window| {
            let (connection_id_copy, database_name_copy, schema_name_copy, table_name_copy) =
                handle
                    .read_with(&*window, |modal: &Self, _| {
                        (
                            modal.connection_id,
                            modal.database_name.clone(),
                            modal.schema_name.clone(),
                            modal.table_name.clone(),
                        )
                    })
                    .ok()
                    .unwrap_or_default();
            let db_service = window
                .update(|_window, cx| DatabaseService::global(cx).clone())
                .ok();
            let Some(db_service) = db_service else {
                return;
            };
            let columns = async {
                let connection = db_service
                    .get_or_create_connection(connection_id_copy, Some(database_name_copy.as_str()))
                    .await?;
                connection
                    .get_columns_for_table(&table_name_copy, schema_name_copy.as_deref())
                    .await
            }
            .await;
            handle
                .update_in(window, |modal, window, cx| match columns {
                    Ok(cols) => modal.set_table_columns(cols, window, cx),
                    Err(err) => {
                        modal.error = Some(format!("Failed to load columns: {}", err));
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            connection_id,
            database_name,
            schema_name,
            table_name,
            db_type,
            file_input,
            encoding_select,
            delimiter_select,
            conflict_select,
            conflict_keys_input,
            has_header: true,
            table_columns: Vec::new(),
            detected: None,
            mapping_selects: Vec::new(),
            fixed_inputs: Vec::new(),
            mappings: Vec::new(),
            transforms: Vec::new(),
            transform_selects: Vec::new(),
            update_column_checks: Vec::new(),
            is_importing: false,
            imported_rows: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            import_result: None,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
            _subscriptions: subscriptions,
            _mapping_subscriptions: Vec::new(),
        }
    }

    fn set_table_columns(
        &mut self,
        columns: Vec<ColumnInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mappings = columns.iter().map(|_| ColumnMapping::Skip).collect();
        self.update_column_checks = vec![true; columns.len()];
        self.transforms = vec![Transform::None; columns.len()];
        self.table_columns = columns;
        self.rebuild_mapping_widgets(window, cx);
        cx.notify();
    }

    fn rebuild_mapping_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let headers: Vec<String> = self
            .detected
            .as_ref()
            .map(effective_headers)
            .unwrap_or_default();
        let options = mapping_options(&headers);

        let mut selects = Vec::with_capacity(self.table_columns.len());
        let mut fixed_inputs = Vec::with_capacity(self.table_columns.len());
        let mut transform_selects = Vec::with_capacity(self.table_columns.len());
        let mut new_subscriptions = Vec::new();

        let transform_labels: Vec<String> = vec!["None".to_string(), "Slugify".to_string()];

        for (i, (col, current)) in self
            .table_columns
            .iter()
            .zip(self.mappings.iter())
            .enumerate()
        {
            let initial_index = match current {
                ColumnMapping::Skip => guess_csv_match(&col.name, &headers)
                    .map(|i| i + 1)
                    .unwrap_or(0),
                ColumnMapping::CsvColumn(idx) => {
                    let candidate = *idx + 1;
                    if candidate < options.len() - 1 {
                        candidate
                    } else {
                        0
                    }
                }
                ColumnMapping::Fixed(_) => options.len() - 1,
            };

            let opts = options.clone();
            let state = cx
                .new(|cx| SelectState::new(opts, Some(IndexPath::new(initial_index)), window, cx));

            let fixed_text = match current {
                ColumnMapping::Fixed(v) => v.clone(),
                _ => String::new(),
            };
            let fixed_input = cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                if !fixed_text.is_empty() {
                    s.set_value(fixed_text, window, cx);
                }
                s
            });

            new_subscriptions.push(cx.subscribe(
                &state,
                move |modal, select_entity, event: &SelectEvent<Vec<String>>, cx| {
                    let SelectEvent::Confirm(_) = event;
                    let selected = selected_row(&select_entity, cx);
                    modal.update_mapping(i, selected, cx);
                },
            ));

            new_subscriptions.push(cx.subscribe(
                &fixed_input,
                move |modal, _input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        tracing::debug!("fixed input changed for column {}", i);
                        modal.sync_fixed_values(cx);
                        cx.notify();
                    }
                },
            ));

            let transform_idx = match self.transforms.get(i) {
                Some(Transform::Slugify) => 1,
                _ => 0,
            };
            let transform_select = cx.new(|cx| {
                SelectState::new(
                    transform_labels.clone(),
                    Some(IndexPath::new(transform_idx)),
                    window,
                    cx,
                )
            });

            let transform_i = i;
            new_subscriptions.push(cx.subscribe(
                &transform_select,
                move |modal, _select, _event: &SelectEvent<Vec<String>>, cx| {
                    if let Some(slot) = modal.transforms.get_mut(transform_i) {
                        let idx = _select
                            .read(cx)
                            .selected_index(cx)
                            .map(|ip| ip.row)
                            .unwrap_or(0);
                        *slot = match idx {
                            1 => Transform::Slugify,
                            _ => Transform::None,
                        };
                        cx.notify();
                    }
                },
            ));

            selects.push(state);
            fixed_inputs.push(fixed_input);
            transform_selects.push(transform_select);
        }

        // Also infer initial mappings from matching headers so the UI and state agree.
        for (i, col) in self.table_columns.iter().enumerate() {
            if matches!(self.mappings.get(i), Some(ColumnMapping::Skip))
                && let Some(h) = guess_csv_match(&col.name, &headers)
            {
                self.mappings[i] = ColumnMapping::CsvColumn(h);
            }
        }

        self._mapping_subscriptions = new_subscriptions;
        self.mapping_selects = selects;
        self.fixed_inputs = fixed_inputs;
        self.transform_selects = transform_selects;
    }

    fn update_mapping(&mut self, row: usize, selected: usize, cx: &mut Context<Self>) {
        let Some(detected) = &self.detected else {
            return;
        };
        let headers = effective_headers(detected);
        let fixed = self
            .fixed_inputs
            .get(row)
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        if let Some(m) = self.mappings.get_mut(row) {
            *m = decode_mapping_index(selected, headers.len(), &fixed);
        }
        cx.notify();
    }

    fn sync_fixed_values(&mut self, cx: &App) {
        for (i, mapping) in self.mappings.iter_mut().enumerate() {
            if let ColumnMapping::Fixed(_) = mapping {
                let value = self
                    .fixed_inputs
                    .get(i)
                    .map(|input| input.read(cx).value().to_string())
                    .unwrap_or_default();
                *mapping = ColumnMapping::Fixed(value);
            }
        }
    }

    fn browse_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select CSV file".into()),
        });
        let file_input = self.file_input.clone();
        cx.spawn_in(window, async move |handle, window| {
            if let Some(path) = path.await.ok()?.ok()?
                && let Some(file) = path.first()
                && let Some(file_str) = file.to_str()
            {
                let path_str = file_str.to_string();
                window
                    .update(|window, cx| {
                        file_input.update(cx, |input, cx| {
                            input.set_value(path_str.clone(), window, cx);
                        });
                    })
                    .ok();
                handle
                    .update_in(window, |modal, window, cx| {
                        modal.redetect(window, cx);
                    })
                    .ok();
            }
            Some(())
        })
        .detach();
    }

    fn redetect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path_str = self.file_input.read(cx).value().to_string();
        if path_str.is_empty() {
            self.detected = None;
            cx.notify();
            return;
        }
        let path = PathBuf::from(&path_str);

        let selected_encoding_idx = selected_row(&self.encoding_select, cx);
        let selected_delim_idx = selected_row(&self.delimiter_select, cx);

        let detect_result = if selected_encoding_idx == 0 && selected_delim_idx == 0 {
            detect::detect_file(&path)
        } else {
            let encoding = encoding_from_index(selected_encoding_idx);
            let delimiter = delimiter_from_index(selected_delim_idx);
            let resolved_encoding = match encoding {
                Some(e) => e,
                None => {
                    let raw = std::fs::read(&path).ok();
                    match raw {
                        Some(bytes) => {
                            let mut d = chardetng::EncodingDetector::new(
                                chardetng::Iso2022JpDetection::Allow,
                            );
                            d.feed(&bytes, true);
                            d.guess(None, chardetng::Utf8Detection::Allow)
                        }
                        None => encoding_rs::UTF_8,
                    }
                }
            };
            let resolved_delim = delimiter.unwrap_or(b',');
            match detect::detect_file(&path) {
                // Delimiter and header overrides only make sense for CSV.
                Ok(detected) if detected.format == detect::ImportFormat::Json => Ok(detected),
                _ => detect::read_sample_with(
                    &path,
                    resolved_encoding,
                    resolved_delim,
                    self.has_header,
                ),
            }
        };

        match detect_result {
            Ok(detected) => {
                self.detected = Some(detected);
                self.error = None;
                self.rebuild_mapping_widgets(window, cx);
            }
            Err(err) => {
                self.error = Some(format!("Failed to read file: {}", err));
            }
        }
        cx.notify();
    }

    fn resolved_encoding(&self, cx: &App) -> &'static Encoding {
        if let Some(d) = &self.detected {
            return d.encoding;
        }
        let idx = selected_row(&self.encoding_select, cx);
        encoding_from_index(idx).unwrap_or(encoding_rs::UTF_8)
    }

    fn resolved_delimiter(&self, cx: &App) -> u8 {
        if let Some(d) = &self.detected {
            return d.delimiter;
        }
        let idx = selected_row(&self.delimiter_select, cx);
        delimiter_from_index(idx).unwrap_or(b',')
    }

    fn resolved_conflict(&self, cx: &App) -> ConflictStrategy {
        let idx = selected_row(&self.conflict_select, cx);
        let keys_text = self.conflict_keys_input.read(cx).value().to_string();
        let keys: Vec<String> = keys_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        match idx {
            1 => ConflictStrategy::DoNothing { keys },
            2 => {
                let update_columns: Vec<String> = self
                    .table_columns
                    .iter()
                    .zip(self.mappings.iter())
                    .zip(self.update_column_checks.iter())
                    .filter_map(|((col, mapping), checked)| {
                        if !checked || matches!(mapping, ColumnMapping::Skip) {
                            return None;
                        }
                        if keys.iter().any(|k| k.eq_ignore_ascii_case(&col.name)) {
                            return None;
                        }
                        Some(col.name.clone())
                    })
                    .collect();
                ConflictStrategy::Update {
                    keys,
                    update_columns,
                }
            }
            _ => ConflictStrategy::Fail,
        }
    }

    pub fn start_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("start_import called, is_importing={}", self.is_importing);
        if self.is_importing {
            return;
        }
        self.sync_fixed_values(cx);
        let path_str = self.file_input.read(cx).value().to_string();
        tracing::info!("file path: {:?}", path_str);
        if path_str.is_empty() {
            tracing::warn!("no file selected");
            self.error = Some("Please select a file".into());
            cx.notify();
            return;
        }
        let compiled = compile_mappings(&self.table_columns, &self.mappings, &self.transforms);
        tracing::info!("compiled {} mappings", compiled.len());
        if compiled.is_empty() {
            tracing::warn!("no columns mapped");
            self.error = Some("Map at least one column".into());
            cx.notify();
            return;
        }
        let conflict = self.resolved_conflict(cx);
        if let ConflictStrategy::Update { update_columns, .. } = &conflict
            && update_columns.is_empty()
        {
            self.error = Some("Select at least one column to update on conflict".into());
            cx.notify();
            return;
        }

        let encoding = self.resolved_encoding(cx);
        let delimiter = self.resolved_delimiter(cx);
        let has_header = self.has_header;
        let (format, source_headers) = self
            .detected
            .as_ref()
            .map(|detected| (detected.format, detected.headers.clone()))
            .unwrap_or((detect::ImportFormat::Csv, Vec::new()));
        let db_type = self.db_type;
        let connection_id = self.connection_id;
        let database_name = self.database_name.clone();
        let schema_name = self.schema_name.clone();
        let table_name = self.table_name.clone();
        let fq_table = quote_qualified(schema_name.as_deref(), &table_name, db_type);
        let db_service = DatabaseService::global(cx).clone();
        self.cancel
            .store(false, std::sync::atomic::Ordering::Release);
        let cancel = self.cancel.clone();

        self.is_importing = true;
        self.imported_rows
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.error = None;
        tracing::info!("starting import spawn");
        cx.notify();

        let path = PathBuf::from(&path_str);
        let progress = self.imported_rows.clone();

        cx.spawn_in(window, {
            let cancel = self.cancel.clone();
            async move |handle, window| {
                while !cancel.load(std::sync::atomic::Ordering::Acquire) {
                    smol::Timer::after(std::time::Duration::from_millis(200)).await;
                    let still_importing = handle
                        .read_with(&*window, |m, _| m.is_importing)
                        .unwrap_or(false);
                    if !still_importing {
                        break;
                    }
                    let _ = handle.update_in(window, |_, _window, cx| {
                        cx.notify();
                    });
                }
            }
        })
        .detach();

        cx.spawn_in(window, async move |handle, window| {
            tracing::info!("import async task started");
            let result: Result<crate::import::service::ImportReport, ImportError> = async {
                let connection = db_service
                    .get_or_create_connection(connection_id, Some(database_name.as_str()))
                    .await
                    .map_err(ImportError::from)?;
                let request = ImportRequest {
                    connection: connection.as_ref(),
                    database: Some(database_name.as_str()),
                    fq_table: fq_table.as_str(),
                    db_type,
                    mappings: &compiled,
                    conflict: &conflict,
                    file: path.as_path(),
                    format,
                    source_headers: &source_headers,
                    encoding,
                    delimiter,
                    has_header,
                    batch_size: DEFAULT_BATCH_SIZE,
                    cancel,
                };
                run_import(request, {
                    let progress = progress.clone();
                    move |rows| {
                        progress.store(rows, std::sync::atomic::Ordering::Relaxed);
                    }
                })
                .await
            }
            .await;

            handle
                .update_in(window, |modal, _window, cx| {
                    modal.is_importing = false;
                    match &result {
                        Ok(report) => {
                            tracing::info!("import completed: {} rows", report.rows_inserted);
                            modal.import_result = Some(report.clone());
                        }
                        Err(err) => {
                            tracing::error!("import failed: {}", err);
                            modal.error = Some(format!("Import failed: {}", err));
                        }
                    }
                    cx.notify();
                })
                .log_err();
        })
        .detach();
    }

    fn render_preview(&self, cx: &App) -> Option<String> {
        let detected = self.detected.as_ref()?;
        // Read live fixed values from inputs before compiling
        let live_mappings: Vec<ColumnMapping> = self
            .mappings
            .iter()
            .enumerate()
            .map(|(i, m)| match m {
                ColumnMapping::Fixed(_) => {
                    let value = self
                        .fixed_inputs
                        .get(i)
                        .map(|input| input.read(cx).value().to_string())
                        .unwrap_or_default();
                    ColumnMapping::Fixed(value)
                }
                other => other.clone(),
            })
            .collect();
        let compiled = compile_mappings(&self.table_columns, &live_mappings, &self.transforms);
        if compiled.is_empty() {
            return Some("-- map at least one column --".into());
        }
        let fq_table = quote_qualified(self.schema_name.as_deref(), &self.table_name, self.db_type);
        let conflict = self.resolved_conflict(cx);
        let statements = preview_statements(
            &fq_table,
            &compiled,
            &detected.sample_rows,
            self.db_type,
            &conflict,
            3,
        );
        Some(statements.join("\n"))
    }
}

fn encoding_from_index(idx: usize) -> Option<&'static Encoding> {
    let (_label, name) = ENCODINGS.get(idx)?;
    if *name == "auto" {
        return None;
    }
    Encoding::for_label(name.as_bytes())
}

fn delimiter_from_index(idx: usize) -> Option<u8> {
    let (_label, byte) = DELIMITERS.get(idx)?;
    if *byte == 0 { None } else { Some(*byte) }
}

fn guess_csv_match(column_name: &str, headers: &[String]) -> Option<usize> {
    headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case(column_name))
}

fn selected_row(state: &Entity<SelectState<Vec<String>>>, cx: &App) -> usize {
    state
        .read(cx)
        .selected_index(cx)
        .map(|ip| ip.row)
        .unwrap_or(0)
}

impl Focusable for ImportModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImportModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(report) = &self.import_result {
            return v_flex()
                .gap_4()
                .pt_2()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Import complete"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} rows imported into {} in {:.1}s",
                                    report.rows_inserted,
                                    self.table_name,
                                    report.duration_ms as f64 / 1000.0,
                                )),
                        ),
                )
                .child(
                    h_flex().gap_2().justify_end().child(
                        Button::new("import-done")
                            .label("Done")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                            }),
                    ),
                )
                .into_any_element();
        }
        let detected = self.detected.clone();
        let is_json = detected
            .as_ref()
            .is_some_and(|detected| detected.format == detect::ImportFormat::Json);
        let preview = self.render_preview(cx).unwrap_or_default();
        let conflict_idx = selected_row(&self.conflict_select, cx);

        let conflict_keys_text = self.conflict_keys_input.read(cx).value().to_string();
        let conflict_keys: Vec<String> = conflict_keys_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut preview_highlighter = SyntaxHighlighter::new("sql");
        let preview_rope = ropey::Rope::from(&preview[..]);
        preview_highlighter.update(None, &preview_rope, None);
        let preview_text = SharedString::from(preview);
        let preview_range = 0..preview_text.len();
        let preview_highlights =
            preview_highlighter.styles(&preview_range, cx.theme().highlight_theme.as_ref());

        v_flex()
            .gap_4()
            .pt_2()
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("CSV or JSON file"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Input::new(&self.file_input).flex_1())
                            .child(Button::new("import-browse").label("Browse").on_click(
                                cx.listener(|modal, _event, window, cx| {
                                    modal.browse_file(window, cx);
                                }),
                            )),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .child(div().text_sm().child("Encoding"))
                            .child(Select::new(&self.encoding_select)),
                    )
                    .when(!is_json, |this| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .flex_1()
                                .child(div().text_sm().child("Delimiter"))
                                .child(Select::new(&self.delimiter_select)),
                        )
                        .child(
                            div().pb_1().child(
                                Checkbox::new("import-has-header")
                                    .label("First row is header")
                                    .small()
                                    .checked(self.has_header)
                                    .on_click(cx.listener(|modal, checked: &bool, window, cx| {
                                        modal.has_header = *checked;
                                        modal.redetect(window, cx);
                                    })),
                            ),
                        )
                    })
                    .when(is_json, |this| {
                        this.child(
                            div()
                                .pb_1()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("JSON: object keys become columns"),
                        )
                    }),
            )
            .when_some(detected.as_ref(), |this, d| {
                let headers = if d.headers.is_empty() {
                    "(no headers)".to_string()
                } else {
                    d.headers.join(", ")
                };
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "Detected {} | delimiter '{}' | columns: {}",
                            d.encoding.name(),
                            d.delimiter as char,
                            headers
                        )),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Column mapping"))
                    .child(
                        v_flex()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius)
                            .overflow_hidden()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .px_2()
                                    .py_1()
                                    .bg(cx.theme().table_head)
                                    .border_b_1()
                                    .border_color(cx.theme().table_row_border)
                                    .child(
                                        div()
                                            .w(px(160.0))
                                            .text_xs()
                                            .text_color(cx.theme().table_head_foreground)
                                            .child("Table column"),
                                    )
                                    .child(
                                        div()
                                            .w(px(200.0))
                                            .text_xs()
                                            .text_color(cx.theme().table_head_foreground)
                                            .child("Source"),
                                    )
                                    .child(
                                        div()
                                            .w(px(140.0))
                                            .text_xs()
                                            .text_color(cx.theme().table_head_foreground)
                                            .child("Fixed value"),
                                    )
                                    .child(
                                        div()
                                            .w(px(120.0))
                                            .text_xs()
                                            .text_color(cx.theme().table_head_foreground)
                                            .child("Transform"),
                                    ),
                            )
                            .children(self.table_columns.iter().enumerate().map(|(i, col)| {
                                let flags = if col.is_primary_key {
                                    " (PK)"
                                } else if !col.is_nullable {
                                    " NOT NULL"
                                } else {
                                    ""
                                };
                                let fixed_visible =
                                    matches!(self.mappings.get(i), Some(ColumnMapping::Fixed(_)));
                                let is_skip =
                                    matches!(self.mappings.get(i), Some(ColumnMapping::Skip));
                                let select = self.mapping_selects.get(i).cloned();
                                let fixed = self.fixed_inputs.get(i).cloned();
                                let transform = self.transform_selects.get(i).cloned();
                                let is_even = i % 2 == 1;
                                h_flex()
                                    .gap_2()
                                    .px_2()
                                    .py_1()
                                    .when(is_even, |row| row.bg(cx.theme().table_even))
                                    .border_b_1()
                                    .border_color(cx.theme().table_row_border)
                                    .child(
                                        div()
                                            .w(px(160.0))
                                            .text_xs()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(format!("{}{}", col.name, flags)),
                                    )
                                    .child(div().w(px(200.0)).when_some(select, |row, state| {
                                        row.child(Select::new(&state))
                                    }))
                                    .child(div().w(px(140.0)).when(fixed_visible, move |row| {
                                        if let Some(input) = &fixed {
                                            row.child(Input::new(input))
                                        } else {
                                            row
                                        }
                                    }))
                                    .child(div().w(px(120.0)).when(!is_skip, move |row| {
                                        if let Some(ts) = &transform {
                                            row.child(Select::new(ts))
                                        } else {
                                            row
                                        }
                                    }))
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("On conflict"))
                    .child(Select::new(&self.conflict_select))
                    .when(conflict_idx > 0, |this| {
                        this.child(Input::new(&self.conflict_keys_input))
                    })
                    .when(conflict_idx == 2, |this| {
                        let update_checkboxes: Vec<_> = self
                            .table_columns
                            .iter()
                            .enumerate()
                            .filter_map(|(i, col)| {
                                let mapping = self.mappings.get(i);
                                if matches!(mapping, Some(ColumnMapping::Skip)) {
                                    return None;
                                }
                                if conflict_keys
                                    .iter()
                                    .any(|k| k.eq_ignore_ascii_case(&col.name))
                                {
                                    return None;
                                }
                                let checked =
                                    self.update_column_checks.get(i).copied().unwrap_or(true);
                                Some(
                                    Checkbox::new(("import-upd", i))
                                        .label(col.name.clone())
                                        .checked(checked)
                                        .on_click(cx.listener(
                                            move |modal, checked: &bool, _, cx| {
                                                if let Some(slot) =
                                                    modal.update_column_checks.get_mut(i)
                                                {
                                                    *slot = *checked;
                                                    cx.notify();
                                                }
                                            },
                                        )),
                                )
                            })
                            .collect();
                        this.child(
                            v_flex()
                                .gap_1()
                                .child(div().text_xs().child("Update columns"))
                                .children(update_checkboxes),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Preview (first 3 rows)"))
                    .child(
                        div()
                            .id("import-preview")
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(12.0))
                            .p_2()
                            .rounded(cx.theme().radius)
                            .max_h(px(160.0))
                            .overflow_y_scroll()
                            .child(
                                StyledText::new(preview_text).with_highlights(preview_highlights),
                            ),
                    ),
            )
            .when(self.is_importing, |this| {
                let rows = self
                    .imported_rows
                    .load(std::sync::atomic::Ordering::Relaxed);
                this.child(div().text_sm().child(format!("Importing... {} rows", rows)))
            })
            .when_some(self.error.clone(), |this, err| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(err))
            })
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        div().flex_shrink(1.).child(
                            DialogClose::new().child(
                                Button::new("import-cancel")
                                    .label("Cancel")
                                    .outline()
                                    .flex_shrink(1.),
                            ),
                        ),
                    )
                    .when(!self.is_importing, |this| {
                        this.child(
                            Button::new("import-submit")
                                .primary()
                                .label("Import")
                                .on_click(cx.listener(|modal, _event, window, cx| {
                                    modal.start_import(window, cx);
                                })),
                        )
                    }),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }
}
