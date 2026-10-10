//! Opening a SQLite database straight from a file: from a launch argument, a
//! file the platform asks the app to open, or a file dropped on the window.
//! Files are recognised by their header, so any extension (or none) works.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use gpui::Global;

/// The first 16 bytes of every SQLite 3 database file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Whether `path` is a SQLite database, judged by its header.
pub fn is_sqlite_file(path: &Path) -> bool {
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| &header == SQLITE_HEADER)
}

/// The file paths among the launch arguments, skipping the program name and
/// anything that looks like a flag.
pub fn paths_from_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<PathBuf> {
    args.into_iter()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(PathBuf::from)
        .collect()
}

/// The local paths among URLs the platform asked the app to open (macOS sends
/// opened documents as `file://` URLs).
pub fn paths_from_urls(urls: &[String]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| url::Url::parse(url).ok())
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}

/// Files waiting to be opened, sent before the window exists (launch
/// arguments) or at any time later (the platform's open requests).
pub struct OpenFileRequests {
    pub receiver: smol::channel::Receiver<Vec<PathBuf>>,
    /// Blanco was started to open files, so the first-run dialog stays shut.
    pub at_launch: bool,
}

impl Global for OpenFileRequests {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_sqlite_file_reads_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("data.anything");
        write_header_only_database(&database);
        assert!(is_sqlite_file(&database));

        let text = dir.path().join("notes.db");
        std::fs::write(&text, "not a database at all").unwrap();
        assert!(!is_sqlite_file(&text));
        assert!(!is_sqlite_file(&dir.path().join("missing.db")));
    }

    /// Writes the smallest valid header, enough for detection.
    fn write_header_only_database(path: &Path) {
        let mut bytes = SQLITE_HEADER.to_vec();
        bytes.resize(100, 0);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn test_paths_from_args_and_urls() {
        let args = ["blanco", "--verbose", "/tmp/a.db", "b.sqlite"].map(std::ffi::OsString::from);
        assert_eq!(
            paths_from_args(args),
            [PathBuf::from("/tmp/a.db"), PathBuf::from("b.sqlite")]
        );
        assert_eq!(
            paths_from_urls(&[
                "file:///tmp/my%20data.db".to_string(),
                "https://example.com/x.db".to_string(),
            ]),
            [PathBuf::from("/tmp/my data.db")]
        );
    }
}
