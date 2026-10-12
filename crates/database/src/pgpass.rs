//! PostgreSQL's password file (`~/.pgpass`), read the way libpq reads it.
//!
//! Each line is `hostname:port:database:username:password`. The first four
//! fields may be `*` to match anything, `\:` and `\\` escape a colon and a
//! backslash, `#` starts a comment line, and the first matching line wins.
//! `PGPASSFILE` names the file instead of the default location. The file is
//! read on every lookup, never cached, so an edit applies to the next connect.

use std::path::{Path, PathBuf};

/// The password file in effect: `PGPASSFILE` when set, otherwise
/// `~/.pgpass`, or `%APPDATA%\postgresql\pgpass.conf` on Windows.
pub fn password_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("PGPASSFILE").filter(|file| !file.is_empty()) {
        return Some(PathBuf::from(file));
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|app_data| {
            PathBuf::from(app_data)
                .join("postgresql")
                .join("pgpass.conf")
        })
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".pgpass"))
    }
}

/// Look up the password for a connection in the password file. A missing or
/// unreadable file finds nothing.
pub fn lookup(host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    let path = password_file()?;
    let contents = read_password_file(&path)?;
    find_password(&contents, host, port, database, user)
}

fn read_password_file(path: &Path) -> Option<String> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!("Cannot read password file {}: {error}", path.display());
            return None;
        }
    };
    // libpq ignores a file others can read, since it holds passwords.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path).ok()?.permissions().mode();
        if mode & 0o077 != 0 {
            tracing::warn!(
                "Ignoring password file {}: it is readable by others; run chmod 0600 on it",
                path.display()
            );
            return None;
        }
    }
    Some(contents)
}

/// The password of the first line in `contents` matching the connection.
pub fn find_password(
    contents: &str,
    host: &str,
    port: u16,
    database: &str,
    user: &str,
) -> Option<String> {
    let port = port.to_string();
    let wanted = [host, port.as_str(), database, user];
    contents
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(|line| {
            let fields = split_fields(line)?;
            fields[..4]
                .iter()
                .zip(wanted)
                .all(|(field, wanted)| field == "*" || field == wanted)
                .then(|| fields[4].clone())
        })
}

/// Split a line into its five fields, undoing `\:` and `\\`. A line with
/// fewer fields is skipped; colons in the password need no escape.
fn split_fields(line: &str) -> Option<[String; 5]> {
    let mut fields: Vec<String> = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(escaped) = chars.next() {
                    fields.last_mut()?.push(escaped);
                }
            }
            ':' if fields.len() < 5 => fields.push(String::new()),
            c => fields.last_mut()?.push(c),
        }
    }
    fields.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_matching_line_wins_with_wildcards() {
        let contents = "\
# comment
db.example.com:5432:app:alice:exact
*:5432:app:alice:any-host
*:*:*:bob:bob-everywhere
db.example.com:5432:*:*:fallback
";
        let find = |host, port, database, user| find_password(contents, host, port, database, user);
        assert_eq!(
            find("db.example.com", 5432, "app", "alice").as_deref(),
            Some("exact")
        );
        assert_eq!(
            find("other.host", 5432, "app", "alice").as_deref(),
            Some("any-host")
        );
        assert_eq!(
            find("other.host", 6543, "other", "bob").as_deref(),
            Some("bob-everywhere")
        );
        assert_eq!(
            find("db.example.com", 5432, "reports", "carol").as_deref(),
            Some("fallback")
        );
        assert_eq!(find("db.example.com", 5433, "app", "alice"), None);
        assert_eq!(find("other.host", 5432, "app", "carol"), None);
    }

    #[test]
    fn escapes_and_colons_in_the_password() {
        let contents = "h\\:1:5432:d\\\\b:u:pa:ss\\:wo\\\\rd\nshort:line\n";
        assert_eq!(
            find_password(contents, "h:1", 5432, "d\\b", "u").as_deref(),
            Some("pa:ss:wo\\rd")
        );
        assert_eq!(find_password(contents, "short", 5432, "line", "u"), None);
    }

    #[test]
    fn windows_line_endings_and_empty_passwords() {
        let contents = "localhost:5432:app:alice:secret\r\nlocalhost:5432:app:bob:\r\n";
        assert_eq!(
            find_password(contents, "localhost", 5432, "app", "alice").as_deref(),
            Some("secret")
        );
        assert_eq!(
            find_password(contents, "localhost", 5432, "app", "bob").as_deref(),
            Some("")
        );
    }
}
