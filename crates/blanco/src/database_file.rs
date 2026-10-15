//! Opening a database file (SQLite or DuckDB) straight from a file: from a
//! launch argument, a file the platform asks the app to open, or a file
//! dropped on the window. Files are recognised by their header, so any
//! extension (or none) works.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use database::DatabaseType;
use gpui::Global;

/// The first 16 bytes of every SQLite 3 database file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// DuckDB's magic bytes, after the 8-byte checksum that starts its files.
const DUCKDB_MAGIC: &[u8; 4] = b"DUCK";
const DUCKDB_MAGIC_OFFSET: usize = 8;

/// The backend whose database `path` is, judged by its header.
pub fn database_file_type(path: &Path) -> Option<DatabaseType> {
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .ok()?;
    if &header == SQLITE_HEADER {
        Some(DatabaseType::SQLite)
    } else if &header[DUCKDB_MAGIC_OFFSET..DUCKDB_MAGIC_OFFSET + DUCKDB_MAGIC.len()] == DUCKDB_MAGIC
    {
        Some(DatabaseType::DuckDb)
    } else {
        None
    }
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
    fn test_database_file_type_reads_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("data.anything");
        write_header_only_database(&database);
        assert_eq!(database_file_type(&database), Some(DatabaseType::SQLite));

        let duckdb = dir.path().join("analytics.db");
        let mut bytes = vec![0u8; 12288];
        bytes[DUCKDB_MAGIC_OFFSET..DUCKDB_MAGIC_OFFSET + 4].copy_from_slice(DUCKDB_MAGIC);
        std::fs::write(&duckdb, bytes).unwrap();
        assert_eq!(database_file_type(&duckdb), Some(DatabaseType::DuckDb));

        let text = dir.path().join("notes.db");
        std::fs::write(&text, "not a database at all").unwrap();
        assert_eq!(database_file_type(&text), None);
        assert_eq!(database_file_type(&dir.path().join("missing.db")), None);
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
