//! Run SQL File: stream a script from disk and run it statement by statement
//! on a query tab's connection, without loading it into the editor.

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use blanco_core::statement_stream::StatementSplitter;
use database::DatabaseService;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    Styled as _, Task, Window, div,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::EditorState,
};

use super::tabs::{TabType, file_name};
use super::{EditorPanel, ProdWritePrompt};
use crate::result_ext::ResultExt as _;
use crate::status_bar::{ActivityReporter, ActivityResult};

/// Bytes read from the file per step.
const CHUNK_BYTES: usize = 64 * 1024;

/// A file being run on a tab's connection.
pub struct FileRun {
    /// The editor of the tab the file runs for.
    editor: Entity<EditorState>,
    file_name: String,
    total_bytes: u64,
    progress: Arc<FileRunProgress>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

#[derive(Default)]
pub struct FileRunProgress {
    statements: AtomicUsize,
    /// Bytes read from the file as stored, compressed or not.
    bytes_read: AtomicU64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileRunOutcome {
    Completed {
        statements: usize,
    },
    Cancelled {
        statements: usize,
    },
    Failed {
        /// The 1-based number of the statement that failed, 0 when the file
        /// could not be read or the connection opened.
        statement_number: usize,
        statement: Option<String>,
        error: String,
    },
}

/// Counts the bytes read through it, for the progress percentage.
struct CountingReader<R> {
    inner: R,
    progress: Arc<FileRunProgress>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.progress
            .bytes_read
            .fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

/// Open `path` for streaming, decompressing gzip by its magic bytes.
fn open_script(
    path: &Path,
    progress: Arc<FileRunProgress>,
) -> std::io::Result<Box<dyn Read + Send>> {
    let mut magic = [0u8; 2];
    let gzip = std::fs::File::open(path)?.read(&mut magic)? == 2 && magic == [0x1f, 0x8b];
    let counting = CountingReader {
        inner: std::fs::File::open(path)?,
        progress,
    };
    Ok(if gzip {
        Box::new(flate2::read::GzDecoder::new(BufReader::new(counting)))
    } else {
        Box::new(BufReader::new(counting))
    })
}

/// Read `reader` and hand each complete statement to `run` as soon as it is
/// complete, stopping at the first error or when `cancel` is raised.
pub async fn stream_statements<F, Fut>(
    mut reader: impl Read,
    splitter: StatementSplitter,
    cancel: &AtomicBool,
    progress: &FileRunProgress,
    mut run: F,
) -> FileRunOutcome
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let mut chunk = vec![0u8; CHUNK_BYTES];
    let mut pending: Vec<u8> = Vec::new();
    let mut count = 0usize;
    let mut finished = false;
    let mut splitter = Some(splitter);
    loop {
        let statements = if finished {
            splitter
                .take()
                .map(StatementSplitter::finish)
                .unwrap_or_default()
        } else {
            let read = match reader.read(&mut chunk) {
                Ok(read) => read,
                Err(error) => {
                    return FileRunOutcome::Failed {
                        statement_number: 0,
                        statement: None,
                        error: format!("Could not read the file: {error}"),
                    };
                }
            };
            if read == 0 {
                finished = true;
                if !pending.is_empty() {
                    return FileRunOutcome::Failed {
                        statement_number: 0,
                        statement: None,
                        error: "The file is not UTF-8 text".to_string(),
                    };
                }
                continue;
            }
            pending.extend_from_slice(&chunk[..read]);
            // A multi-byte character may be cut at the chunk's end; keep its
            // first bytes for the next round.
            let valid = match std::str::from_utf8(&pending) {
                Ok(text) => text.len(),
                Err(error) if error.error_len().is_none() => error.valid_up_to(),
                Err(_) => {
                    return FileRunOutcome::Failed {
                        statement_number: 0,
                        statement: None,
                        error: "The file is not UTF-8 text".to_string(),
                    };
                }
            };
            let text = String::from_utf8(pending.drain(..valid).collect()).unwrap_or_default();
            splitter
                .as_mut()
                .map(|splitter| splitter.push(&text))
                .unwrap_or_default()
        };
        for statement in statements {
            if cancel.load(Ordering::Relaxed) {
                return FileRunOutcome::Cancelled { statements: count };
            }
            if let Err(error) = run(statement.clone()).await {
                return FileRunOutcome::Failed {
                    statement_number: count + 1,
                    statement: Some(statement),
                    error: format!("{error:#}"),
                };
            }
            count += 1;
            progress.statements.store(count, Ordering::Relaxed);
        }
        if finished {
            return FileRunOutcome::Completed { statements: count };
        }
        if cancel.load(Ordering::Relaxed) {
            return FileRunOutcome::Cancelled { statements: count };
        }
    }
}

impl EditorPanel {
    /// Ask for a script and run it on the active query tab's connection.
    pub fn prompt_run_sql_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active_tab_ix;
        let Some(tab) = self.tabs.get(ix).and_then(TabType::query) else {
            Self::push_error(
                "Run SQL File runs on a query tab's connection; open a query tab first",
                window,
                cx,
            );
            return;
        };
        if !tab.context.db_type.supports_sql() {
            Self::push_error("Run SQL File needs a SQL connection", window, cx);
            return;
        }
        if self.file_run.is_some() {
            Self::push_error("A file is already running", window, cx);
            return;
        }
        let is_prod = tab.context.is_prod();
        let connection_name = tab.context.connection_name.clone();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Run".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = paths
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.into_iter().next())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if is_prod {
                    let name = file_name(&path);
                    this.confirm_prod_write(
                        ProdWritePrompt {
                            title: "Run SQL file on PROD?",
                            message: format!(
                                "Every statement in {name} runs on {connection_name}, a PROD connection."
                            ),
                            confirm_label: "Run on PROD",
                            tab_index: ix,
                        },
                        move |this, window, cx| this.start_file_run(ix, path.clone(), window, cx),
                        None,
                        window,
                        cx,
                    );
                } else {
                    this.start_file_run(ix, path, window, cx);
                }
            })
            .log_err();
        })
        .detach();
    }

    fn push_error(message: &str, window: &mut Window, cx: &mut App) {
        use gpui_component::WindowExt as _;
        window.push_notification(
            (
                gpui_component::notification::NotificationType::Error,
                message.to_string(),
            ),
            cx,
        );
    }

    pub fn start_file_run(
        &mut self,
        ix: usize,
        path: PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(ix).and_then(TabType::query) else {
            return;
        };
        let context = tab.context.clone();
        let sql_view = tab.sql_view.clone();
        let editor = tab.editor.clone();
        let name = file_name(&path);
        let total_bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let progress = Arc::new(FileRunProgress::default());
        let cancel = Arc::new(AtomicBool::new(false));

        sql_view.update(cx, |log, cx| {
            log.append_text(
                &blanco_ui::SqlViewMessage::Comment(format!(
                    "Running {name} ({}) statement by statement",
                    format_bytes(total_bytes)
                )),
                cx,
            );
        });
        let activity = ActivityReporter::global(cx)
            .begin(format!("{}: running {name}", context.connection_name));

        let db_service = DatabaseService::global(cx).clone();
        let started = Instant::now();
        let log_name = name.clone();
        let run = cx.background_spawn({
            let progress = progress.clone();
            let cancel = cancel.clone();
            let database_name = context.database_name.clone();
            let dialect = context.dialect();
            async move {
                let reader = match open_script(&path, progress.clone()) {
                    Ok(reader) => reader,
                    Err(error) => {
                        return FileRunOutcome::Failed {
                            statement_number: 0,
                            statement: None,
                            error: format!("Could not open {}: {error}", path.display()),
                        };
                    }
                };
                let connection = match db_service
                    .get_or_create_connection(context.connection_id, Some(&database_name))
                    .await
                {
                    Ok(connection) => connection,
                    Err(error) => {
                        return FileRunOutcome::Failed {
                            statement_number: 0,
                            statement: None,
                            error: format!("{error:#}"),
                        };
                    }
                };
                stream_statements(
                    reader,
                    StatementSplitter::new(dialect),
                    &cancel,
                    &progress,
                    |statement| {
                        let connection = connection.clone();
                        let database_name = database_name.clone();
                        async move {
                            connection
                                .execute_query(&statement, Some(&database_name), None)
                                .await
                                .map(|_| ())
                        }
                    },
                )
                .await
            }
        });

        let task = cx.spawn(async move |this, cx| {
            // Repaint the progress while the run goes on.
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
            let outcome = run.await;
            drop(ticker);
            let elapsed = started.elapsed();
            let (message, result) = describe_outcome(&log_name, &outcome, elapsed);
            activity.finish(result);
            this.update(cx, |this, cx| {
                this.file_run = None;
                sql_view.update(cx, |log, cx| {
                    log.append_text(&blanco_ui::SqlViewMessage::Comment(message), cx);
                });
                cx.notify();
            })
            .log_err();
        });

        self.file_run = Some(FileRun {
            editor,
            file_name: name,
            total_bytes,
            progress,
            cancel,
            _task: task,
        });
        cx.notify();
    }

    /// Stop the running file after the statement in flight.
    pub fn cancel_file_run(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &self.file_run {
            run.cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    /// Progress and a Stop button for the file running on the tab of
    /// `editor`.
    pub(super) fn render_file_run_status(
        &self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let run = self.file_run.as_ref().filter(|run| run.editor == *editor)?;
        let statements = run.progress.statements.load(Ordering::Relaxed);
        let read = run.progress.bytes_read.load(Ordering::Relaxed);
        let percent = (read * 100)
            .checked_div(run.total_bytes)
            .unwrap_or(0)
            .min(100);
        let stopping = run.cancel.load(Ordering::Relaxed);
        Some(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} {} · {} · {percent}%",
                            if stopping { "Stopping" } else { "Running" },
                            run.file_name,
                            statement_count(statements),
                        )),
                )
                .child(
                    Button::new("stop-file-run")
                        .danger()
                        .small()
                        .icon(blanco_ui::IconName::SquareStop)
                        .label("Stop")
                        .tooltip("Stop after the statement in flight")
                        .on_click(cx.listener(|panel, _, _, cx| panel.cancel_file_run(cx))),
                )
                .into_any_element(),
        )
    }
}

fn statement_count(count: usize) -> String {
    match count {
        1 => "1 statement".to_string(),
        count => format!("{count} statements"),
    }
}

fn format_bytes(bytes: u64) -> String {
    match bytes {
        bytes if bytes >= 1024 * 1024 => format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
        bytes if bytes >= 1024 => format!("{:.1} KB", bytes as f64 / 1024.0),
        bytes => format!("{bytes} bytes"),
    }
}

/// The log line and status bar outcome of a run.
fn describe_outcome(
    name: &str,
    outcome: &FileRunOutcome,
    elapsed: Duration,
) -> (String, ActivityResult) {
    let seconds = elapsed.as_secs_f64();
    match outcome {
        FileRunOutcome::Completed { statements } => {
            let text = format!(
                "✓ Ran {} from {name} in {seconds:.1}s",
                statement_count(*statements)
            );
            (text.clone(), ActivityResult::Ok(text.into()))
        }
        FileRunOutcome::Cancelled { statements } => {
            let text = format!(
                "Stopped {name} after {} in {seconds:.1}s",
                statement_count(*statements)
            );
            (text.clone(), ActivityResult::Ok(text.into()))
        }
        FileRunOutcome::Failed {
            statement_number,
            statement,
            error,
        } => {
            let text = match statement {
                Some(statement) => {
                    let mut shown: String = statement.chars().take(500).collect();
                    if shown.len() < statement.len() {
                        shown.push('…');
                    }
                    format!(
                        "✗ {name}: statement {statement_number} failed after {} ran: {error}\n{shown}",
                        statement_count(statement_number.saturating_sub(1))
                    )
                }
                None => format!("✗ {name}: {error}"),
            };
            (text, ActivityResult::Err(error.clone().into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::DatabaseType;

    fn run_script(script: &[u8], fail_on: Option<&str>) -> (FileRunOutcome, Vec<String>) {
        let cancel = AtomicBool::new(false);
        let progress = FileRunProgress::default();
        let ran = std::cell::RefCell::new(Vec::new());
        let outcome = futures::executor::block_on(stream_statements(
            script,
            StatementSplitter::new(DatabaseType::PostgreSQL.dialect()),
            &cancel,
            &progress,
            |statement| {
                let fails = fail_on.is_some_and(|fail| statement.contains(fail));
                ran.borrow_mut().push(statement);
                async move { if fails { anyhow::bail!("boom") } else { Ok(()) } }
            },
        ));
        (outcome, ran.into_inner())
    }

    #[test]
    fn runs_every_statement_in_order() {
        let (outcome, ran) = run_script(b"SELECT 1;\nSELECT 'a;b';\nSELECT 3", None);
        assert_eq!(outcome, FileRunOutcome::Completed { statements: 3 });
        assert_eq!(ran, ["SELECT 1", "SELECT 'a;b'", "SELECT 3"]);
    }

    #[test]
    fn stops_on_the_first_error() {
        let (outcome, ran) = run_script(b"SELECT 1; SELECT 2; SELECT 3;", Some("2"));
        assert_eq!(
            outcome,
            FileRunOutcome::Failed {
                statement_number: 2,
                statement: Some("SELECT 2".to_string()),
                error: "boom".to_string(),
            }
        );
        assert_eq!(ran.len(), 2, "nothing runs after the failure");
    }

    #[test]
    fn rejects_text_that_is_not_utf8() {
        let (outcome, _) = run_script(b"SELECT 1; SELECT '\xff';", None);
        assert!(matches!(
            outcome,
            FileRunOutcome::Failed {
                statement_number: 0,
                ..
            }
        ));
    }

    #[test]
    fn streams_gzip_files_and_counts_progress() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("dump.sql.gz");
        let script: String = (0..5000)
            .map(|i| format!("INSERT INTO t VALUES ({i}, 'å');\n"))
            .collect();
        super::super::sql_files::write_sql_file(&path, &script).expect("write");
        let progress = Arc::new(FileRunProgress::default());
        let reader = open_script(&path, progress.clone()).expect("open");
        let cancel = AtomicBool::new(false);
        let count = std::cell::Cell::new(0);
        let outcome = futures::executor::block_on(stream_statements(
            reader,
            StatementSplitter::new(DatabaseType::SQLite.dialect()),
            &cancel,
            &progress,
            |_| {
                count.set(count.get() + 1);
                async { Ok(()) }
            },
        ));
        assert_eq!(outcome, FileRunOutcome::Completed { statements: 5000 });
        assert_eq!(
            progress.bytes_read.load(Ordering::Relaxed),
            std::fs::metadata(&path).unwrap().len()
        );
    }

    #[test]
    fn cancelling_stops_before_the_next_statement() {
        let cancel = AtomicBool::new(false);
        let progress = FileRunProgress::default();
        let ran = std::cell::Cell::new(0);
        let outcome = futures::executor::block_on(stream_statements(
            b"SELECT 1; SELECT 2; SELECT 3;".as_slice(),
            StatementSplitter::new(DatabaseType::SQLite.dialect()),
            &cancel,
            &progress,
            |_| {
                ran.set(ran.get() + 1);
                cancel.store(true, Ordering::Relaxed);
                async { Ok(()) }
            },
        ));
        assert_eq!(outcome, FileRunOutcome::Cancelled { statements: 1 });
        assert_eq!(ran.get(), 1);
    }

    #[test]
    fn outcome_messages() {
        let (text, _) = describe_outcome(
            "a.sql",
            &FileRunOutcome::Failed {
                statement_number: 3,
                statement: Some("DROP x".into()),
                error: "nope".into(),
            },
            Duration::from_secs(1),
        );
        assert_eq!(
            text,
            "✗ a.sql: statement 3 failed after 2 statements ran: nope\nDROP x"
        );
        let (text, _) = describe_outcome(
            "a.sql",
            &FileRunOutcome::Completed { statements: 1 },
            Duration::from_millis(1500),
        );
        assert_eq!(text, "✓ Ran 1 statement from a.sql in 1.5s");
    }
}
