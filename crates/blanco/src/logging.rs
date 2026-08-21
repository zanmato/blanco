//! Log file and panic hook. A GUI launched from a `.desktop` entry has no
//! visible stderr, so without a file on disk a crash report from a user is
//! undiagnosable. Logs go to both stderr (for terminal runs) and
//! `<state dir>/blanco/blanco.log`, and panics are written there with a
//! backtrace before the default hook runs.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

/// Rotate once the current log exceeds this size, keeping one previous file.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub fn log_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("blanco"))
}

pub fn log_file_path() -> Option<PathBuf> {
    log_dir().map(|dir| dir.join("blanco.log"))
}

/// Open the log file for appending, rotating `blanco.log` to `blanco.log.1`
/// when it has grown past [`MAX_LOG_BYTES`]. Returns `None` (after reporting
/// on stderr) when the file cannot be opened, so logging still works on
/// stderr alone.
// Runs before the tracing subscriber exists, so stderr is the only channel.
#[allow(clippy::print_stderr)]
pub fn open_log_file() -> Option<File> {
    let path = log_file_path()?;
    if let Some(dir) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(dir) {
            eprintln!(
                "blanco: cannot create log directory {}: {error}",
                dir.display()
            );
            return None;
        }
    }
    if let Ok(metadata) = std::fs::metadata(&path) {
        if metadata.len() > MAX_LOG_BYTES {
            let rotated = path.with_extension("log.1");
            if let Err(error) = std::fs::rename(&path, &rotated) {
                eprintln!("blanco: cannot rotate log file {}: {error}", path.display());
            }
        }
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!("blanco: cannot open log file {}: {error}", path.display());
            None
        }
    }
}

/// `MakeWriter` that duplicates every line to stderr and the log file.
#[derive(Clone)]
pub struct StderrAndFile {
    file: Arc<Mutex<File>>,
}

impl StderrAndFile {
    pub fn new(file: File) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
        }
    }
}

pub struct TeeWriter {
    file: Arc<Mutex<File>>,
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Never let a full disk or a closed stderr take the process down:
        // the log is best effort.
        io::stderr().write_all(buf).ok();
        if let Ok(mut file) = self.file.lock() {
            file.write_all(buf).ok();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush().ok();
        if let Ok(mut file) = self.file.lock() {
            file.flush().ok();
        }
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for StderrAndFile {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter {
            file: Arc::clone(&self.file),
        }
    }
}

/// Log panics through tracing (so they land in the log file) with a forced
/// backtrace, then defer to the previous hook for the usual stderr output.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic at {location}: {message}\n{backtrace}");
        previous(info);
    }));
}
