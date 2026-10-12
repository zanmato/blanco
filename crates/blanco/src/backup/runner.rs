use anyhow::{Context as _, Result, anyhow};
use futures::future::Either;
use futures::io::{AsyncBufReadExt as _, AsyncRead, AsyncWriteExt as _, BufReader};
use smol::channel::{Receiver, Sender};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::command::{Input, StandardOutput, ToolCommand, ToolPlan};

/// How many of the last error output lines a failure message quotes.
const ERROR_TAIL_LINES: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEvent {
    /// A step is starting, with its command line (no password).
    Command(String),
    /// A line the tool printed.
    Output(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Finished,
    Stopped,
}

/// Run every step of `plan` in turn until one fails or a message arrives on
/// `stop`. Output lines are sent to `events`; `input_read` counts the bytes of
/// the input file fed to the tool so far. A failed or stopped backup removes
/// its partial output.
pub async fn run_plan(
    plan: &ToolPlan,
    events: &Sender<RunEvent>,
    stop: &Receiver<()>,
    input_read: &Arc<AtomicU64>,
) -> Result<RunOutcome> {
    let result = run_steps(plan, events, stop, input_read).await;
    if !matches!(result, Ok(RunOutcome::Finished))
        && let Some(path) = &plan.remove_on_failure
        && let Err(error) = remove_path(path)
    {
        tracing::warn!("Could not remove {}: {error}", path.display());
    }
    result
}

async fn run_steps(
    plan: &ToolPlan,
    events: &Sender<RunEvent>,
    stop: &Receiver<()>,
    input_read: &Arc<AtomicU64>,
) -> Result<RunOutcome> {
    let mut dump = match &plan.dump_file {
        Some((path, level)) => Some(smol::Unblock::new(DumpSink::create(path, *level)?)),
        None => None,
    };
    for (index, step) in plan.steps.iter().enumerate() {
        let input = if index == 0 {
            &plan.input
        } else {
            &Input::None
        };
        let outcome = run_step(step, input, dump.as_mut(), events, stop, input_read).await?;
        if outcome == RunOutcome::Stopped {
            return Ok(RunOutcome::Stopped);
        }
    }
    if let Some(mut dump) = dump {
        dump.flush().await.context("Could not write the backup")?;
        let sink = dump.into_inner().await;
        smol::unblock(move || sink.finish())
            .await
            .context("Could not write the backup")?;
    }
    Ok(RunOutcome::Finished)
}

async fn run_step(
    step: &ToolCommand,
    input: &Input,
    dump: Option<&mut smol::Unblock<DumpSink>>,
    events: &Sender<RunEvent>,
    stop: &Receiver<()>,
    input_read: &Arc<AtomicU64>,
) -> Result<RunOutcome> {
    emit(events, RunEvent::Command(step.display()));
    let program_name = step
        .program
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| step.program.display().to_string());

    let mut command = smol::process::Command::new(&step.program);
    command
        .args(&step.arguments)
        .envs(
            step.environment
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .stdin(match input {
            Input::None => Stdio::null(),
            Input::File { .. } => Stdio::piped(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console_window(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("Could not start {}", step.program.display()))?;

    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("{program_name} has no output pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("{program_name} has no error pipe"))?;
    let error_tail = Mutex::new(VecDeque::with_capacity(ERROR_TAIL_LINES));

    let feed = async {
        let (Some(mut stdin), Input::File { path, gzip }) = (stdin, input) else {
            return Ok(());
        };
        let reader = open_input(path, *gzip, Arc::clone(input_read))?;
        futures::io::copy(smol::Unblock::new(reader), &mut stdin).await?;
        stdin.close().await?;
        anyhow::Ok(())
    };
    let read_output = async {
        match (step.standard_output, dump) {
            (StandardOutput::Dump, Some(dump)) => {
                futures::io::copy(stdout, dump)
                    .await
                    .context("Could not write the backup")?;
            }
            _ => forward_lines(stdout, events, None).await?,
        }
        anyhow::Ok(())
    };
    let read_errors = forward_lines(stderr, events, Some(&error_tail));
    let mut work = Box::pin(async {
        let (fed, output, errors) = futures::join!(feed, read_output, read_errors);
        (fed, output, errors, child.status().await)
    });

    let (fed, output, errors, status) =
        match futures::future::select(work.as_mut(), Box::pin(wait_for_stop(stop))).await {
            Either::Left((finished, _)) => finished,
            Either::Right(_) => {
                drop(work);
                if let Err(error) = child.kill() {
                    tracing::warn!("Could not stop {program_name}: {error}");
                }
                if let Err(error) = child.status().await {
                    tracing::warn!("{program_name} did not exit: {error}");
                }
                return Ok(RunOutcome::Stopped);
            }
        };

    let status = status.with_context(|| format!("{program_name} did not exit"))?;
    if !status.success() {
        let tail: Vec<String> = error_tail
            .lock()
            .map(|tail| tail.iter().cloned().collect())
            .unwrap_or_default();
        let exit = status
            .code()
            .map(|code| format!("exited with code {code}"))
            .unwrap_or_else(|| "was stopped".to_string());
        return Err(if tail.is_empty() {
            anyhow!("{program_name} {exit}")
        } else {
            anyhow!("{program_name} {exit}: {}", tail.join("\n"))
        });
    }
    output?;
    errors?;
    fed.context("Could not read the dump")?;
    Ok(RunOutcome::Finished)
}

async fn wait_for_stop(stop: &Receiver<()>) {
    // A closed channel means nobody can ask to stop any more, not a stop.
    if stop.recv().await.is_err() {
        futures::future::pending::<()>().await;
    }
}

/// Send each line of `reader` as output, keeping the last few in `tail`.
/// Bytes that are not UTF-8 are replaced rather than ending the stream.
async fn forward_lines(
    reader: impl AsyncRead + Unpin,
    events: &Sender<RunEvent>,
    tail: Option<&Mutex<VecDeque<String>>>,
) -> Result<()> {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer).await? == 0 {
            return Ok(());
        }
        let line = String::from_utf8_lossy(&buffer)
            .trim_end_matches(['\n', '\r'])
            .to_string();
        if let Some(tail) = tail
            && let Ok(mut tail) = tail.lock()
        {
            if tail.len() == ERROR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line.clone());
        }
        emit(events, RunEvent::Output(line));
    }
}

fn emit(events: &Sender<RunEvent>, event: RunEvent) {
    if events.try_send(event).is_err() {
        tracing::debug!("Nobody is listening to the tool's output any more");
    }
}

/// Run `program` with `arguments` and return what it printed, for short
/// helper runs like `pg_restore --list`.
pub async fn capture_output(program: &Path, arguments: &[String]) -> Result<String> {
    let mut command = smol::process::Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    hide_console_window(&mut command);
    let output = command
        .output()
        .await
        .with_context(|| format!("Could not start {}", program.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "{} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Keep a console window from flashing up for every tool run on Windows.
#[cfg_attr(not(windows), allow(unused_variables))]
pub fn hide_console_window(command: &mut smol::process::Command) {
    #[cfg(windows)]
    {
        use smol::process::windows::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

fn open_input(path: &Path, gzip: bool, read: Arc<AtomicU64>) -> Result<Box<dyn Read + Send>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let counted = CountingReader { inner: file, read };
    Ok(if gzip {
        Box::new(flate2::read::GzDecoder::new(std::io::BufReader::new(
            counted,
        )))
    } else {
        Box::new(counted)
    })
}

/// Counts the bytes read from the file, before any decompression, so the
/// progress bar measures against the file's size.
struct CountingReader<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.read.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}

/// The file a dump written to standard output goes to.
enum DumpSink {
    Plain(std::io::BufWriter<std::fs::File>),
    Gzip(flate2::write::GzEncoder<std::io::BufWriter<std::fs::File>>),
}

impl DumpSink {
    fn create(path: &Path, gzip_level: Option<u32>) -> Result<Self> {
        let file = std::io::BufWriter::new(
            std::fs::File::create(path)
                .with_context(|| format!("Cannot create {}", path.display()))?,
        );
        Ok(match gzip_level {
            Some(level) => DumpSink::Gzip(flate2::write::GzEncoder::new(
                file,
                flate2::Compression::new(level),
            )),
            None => DumpSink::Plain(file),
        })
    }

    fn finish(self) -> std::io::Result<()> {
        let mut file = match self {
            DumpSink::Plain(file) => file,
            DumpSink::Gzip(encoder) => encoder.finish()?,
        };
        file.flush()?;
        file.get_ref().sync_all()
    }
}

impl Write for DumpSink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            DumpSink::Plain(file) => file.write(buffer),
            DumpSink::Gzip(encoder) => encoder.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            DumpSink::Plain(file) => file.flush(),
            DumpSink::Gzip(encoder) => encoder.flush(),
        }
    }
}

fn remove_path(path: &PathBuf) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::backup::command::ProgressSource;

    fn shell(script: &str, standard_output: StandardOutput) -> ToolCommand {
        ToolCommand {
            program: PathBuf::from("/bin/sh"),
            arguments: vec!["-c".to_string(), script.to_string()],
            environment: vec![("BLANCO_SECRET".to_string(), "hunter2".to_string())],
            standard_output,
        }
    }

    fn plan(steps: Vec<ToolCommand>) -> ToolPlan {
        ToolPlan {
            steps,
            input: Input::None,
            dump_file: None,
            remove_on_failure: None,
            progress: ProgressSource::Unknown,
        }
    }

    fn collect(receiver: &Receiver<RunEvent>) -> Vec<RunEvent> {
        std::iter::from_fn(|| receiver.try_recv().ok()).collect()
    }

    #[test]
    fn streams_output_and_reads_the_environment() {
        smol::block_on(async {
            let (events, received) = smol::channel::unbounded();
            let (_stop_sender, stop) = smol::channel::bounded(1);
            let outcome = run_plan(
                &plan(vec![shell(
                    "echo one; echo \"secret=$BLANCO_SECRET\" >&2",
                    StandardOutput::Log,
                )]),
                &events,
                &stop,
                &Arc::new(AtomicU64::new(0)),
            )
            .await
            .expect("run");
            assert_eq!(outcome, RunOutcome::Finished);
            let output = collect(&received);
            assert!(output.contains(&RunEvent::Output("one".into())));
            assert!(output.contains(&RunEvent::Output("secret=hunter2".into())));
        });
    }

    #[test]
    fn failure_quotes_the_last_error_lines_and_removes_partial_output() {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("temp dir");
            let output = directory.path().join("partial.sql");
            let mut plan = plan(vec![shell(
                "echo data; echo 'pg_dump: error: connection refused' >&2; exit 1",
                StandardOutput::Dump,
            )]);
            plan.dump_file = Some((output.clone(), None));
            plan.remove_on_failure = Some(output.clone());
            let (events, _received) = smol::channel::unbounded();
            let (_stop_sender, stop) = smol::channel::bounded(1);
            let error = run_plan(&plan, &events, &stop, &Arc::new(AtomicU64::new(0)))
                .await
                .expect_err("should fail");
            let message = format!("{error:#}");
            assert!(message.contains("exited with code 1"), "{message}");
            assert!(message.contains("connection refused"), "{message}");
            assert!(!output.exists());
        });
    }

    #[test]
    fn dump_output_of_every_step_is_gzipped_into_one_file() {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("temp dir");
            let output = directory.path().join("dump.sql.gz");
            let mut plan = plan(vec![
                shell("echo first", StandardOutput::Dump),
                shell("echo second", StandardOutput::Dump),
            ]);
            plan.dump_file = Some((output.clone(), Some(6)));
            let (events, _received) = smol::channel::unbounded();
            let (_stop_sender, stop) = smol::channel::bounded(1);
            run_plan(&plan, &events, &stop, &Arc::new(AtomicU64::new(0)))
                .await
                .expect("run");
            let mut text = String::new();
            flate2::read::GzDecoder::new(std::fs::File::open(&output).expect("open"))
                .read_to_string(&mut text)
                .expect("read");
            assert_eq!(text, "first\nsecond\n");
        });
    }

    #[test]
    fn feeds_a_gzipped_input_file_and_counts_its_bytes() {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("temp dir");
            let input = directory.path().join("dump.sql.gz");
            let mut encoder = flate2::write::GzEncoder::new(
                std::fs::File::create(&input).expect("create"),
                flate2::Compression::default(),
            );
            encoder.write_all(b"select 1;\nselect 2;\n").expect("write");
            encoder.finish().expect("finish");
            let size = std::fs::metadata(&input).expect("metadata").len();

            let mut plan = plan(vec![shell("wc -l", StandardOutput::Log)]);
            plan.input = Input::File {
                path: input,
                gzip: true,
            };
            let (events, received) = smol::channel::unbounded();
            let (_stop_sender, stop) = smol::channel::bounded(1);
            let read = Arc::new(AtomicU64::new(0));
            run_plan(&plan, &events, &stop, &read).await.expect("run");
            assert_eq!(read.load(Ordering::Relaxed), size);
            assert!(
                collect(&received)
                    .iter()
                    .any(|event| matches!(event, RunEvent::Output(line) if line.trim() == "2"))
            );
        });
    }

    #[test]
    fn stopping_kills_the_tool() {
        smol::block_on(async {
            let directory = tempfile::tempdir().expect("temp dir");
            let output = directory.path().join("slow.sql");
            std::fs::write(&output, "partial").expect("write");
            let mut plan = plan(vec![shell("sleep 30", StandardOutput::Log)]);
            plan.remove_on_failure = Some(output.clone());
            let (events, _received) = smol::channel::unbounded();
            let (stop_sender, stop) = smol::channel::bounded(1);
            stop_sender.try_send(()).expect("stop");
            let started = std::time::Instant::now();
            let outcome = run_plan(&plan, &events, &stop, &Arc::new(AtomicU64::new(0)))
                .await
                .expect("run");
            assert_eq!(outcome, RunOutcome::Stopped);
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            assert!(!output.exists());
        });
    }
}
