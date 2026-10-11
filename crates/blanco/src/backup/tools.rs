use anyhow::{Context as _, Result, anyhow};
use blanco_core::ClientTools;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::settings::ClientToolSettings;

/// One of the native programs Blanco runs to back up and restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolProgram {
    PgDump,
    PgRestore,
    Psql,
    MysqlDump,
    Mysql,
}

impl ToolProgram {
    /// The name the program is known by, also the name of its path setting.
    pub fn display_name(self) -> &'static str {
        match self {
            ToolProgram::PgDump => "pg_dump",
            ToolProgram::PgRestore => "pg_restore",
            ToolProgram::Psql => "psql",
            ToolProgram::MysqlDump => "mysqldump",
            ToolProgram::Mysql => "mysql",
        }
    }

    /// Executable names to look for. MariaDB 11 ships the MySQL names only as
    /// deprecated symlinks and some packages leave them out.
    fn executable_names(self) -> &'static [&'static str] {
        match self {
            ToolProgram::PgDump => &["pg_dump"],
            ToolProgram::PgRestore => &["pg_restore"],
            ToolProgram::Psql => &["psql"],
            ToolProgram::MysqlDump => &["mysqldump", "mariadb-dump"],
            ToolProgram::Mysql => &["mysql", "mariadb"],
        }
    }

    /// The program that writes a backup.
    pub fn dump(tools: ClientTools) -> ToolProgram {
        match tools {
            ClientTools::PostgreSql => ToolProgram::PgDump,
            ClientTools::MySql => ToolProgram::MysqlDump,
        }
    }

    /// The programs that restore a backup, by the dump's format.
    pub fn restore(tools: ClientTools) -> &'static [ToolProgram] {
        match tools {
            ClientTools::PostgreSql => &[ToolProgram::Psql, ToolProgram::PgRestore],
            ClientTools::MySql => &[ToolProgram::Mysql],
        }
    }

    pub fn family(self) -> ClientTools {
        match self {
            ToolProgram::PgDump | ToolProgram::PgRestore | ToolProgram::Psql => {
                ClientTools::PostgreSql
            }
            ToolProgram::MysqlDump | ToolProgram::Mysql => ClientTools::MySql,
        }
    }

    pub fn configured_path_mut(self, settings: &mut ClientToolSettings) -> &mut String {
        match self {
            ToolProgram::PgDump => &mut settings.pg_dump_path,
            ToolProgram::PgRestore => &mut settings.pg_restore_path,
            ToolProgram::Psql => &mut settings.psql_path,
            ToolProgram::MysqlDump => &mut settings.mysqldump_path,
            ToolProgram::Mysql => &mut settings.mysql_path,
        }
    }

    pub fn configured_path(self, settings: &ClientToolSettings) -> &str {
        match self {
            ToolProgram::PgDump => &settings.pg_dump_path,
            ToolProgram::PgRestore => &settings.pg_restore_path,
            ToolProgram::Psql => &settings.psql_path,
            ToolProgram::MysqlDump => &settings.mysqldump_path,
            ToolProgram::Mysql => &settings.mysql_path,
        }
    }
}

/// Which server a version belongs to. MySQL and MariaDB share the client
/// tool names but not their version numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    PostgreSql,
    MySql,
    MariaDb,
}

impl Product {
    pub fn display_name(self) -> &'static str {
        match self {
            Product::PostgreSql => "PostgreSQL",
            Product::MySql => "MySQL",
            Product::MariaDb => "MariaDB",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub product: Product,
    pub numbers: [u32; 3],
}

impl Version {
    /// Parse the server's version string as `ClientTools::server_version_query`
    /// returns it.
    pub fn parse_server(tools: ClientTools, text: &str) -> Option<Version> {
        let text = text.trim();
        let product = match tools {
            ClientTools::PostgreSql => Product::PostgreSql,
            ClientTools::MySql if text.contains("MariaDB") => Product::MariaDb,
            ClientTools::MySql => Product::MySql,
        };
        let numbers = leading_numbers(text)?;
        Some(Version { product, numbers })
    }

    /// Parse the first line a tool prints for `--version`:
    ///
    /// - `pg_dump (PostgreSQL) 16.4 (Ubuntu 16.4-1)`
    /// - `mysqldump  Ver 8.0.36 for Linux on x86_64 (MySQL Community Server - GPL)`
    /// - `mysqldump  Ver 10.19 Distrib 10.11.6-MariaDB, for debian-linux-gnu`
    /// - `mariadb-dump from 11.4.2-MariaDB, client 10.19 for Linux`
    ///
    /// The MariaDB forms carry the client protocol version first, so the
    /// server release after `Distrib` or `from` wins.
    pub fn parse_tool(tools: ClientTools, output: &str) -> Option<Version> {
        let line = output.lines().find(|line| !line.trim().is_empty())?;
        match tools {
            ClientTools::PostgreSql => {
                let after = line.split_once("(PostgreSQL)")?.1;
                Some(Version {
                    product: Product::PostgreSql,
                    numbers: leading_numbers(after.trim_start())?,
                })
            }
            ClientTools::MySql => {
                let release = ["Distrib ", " from ", "Ver "]
                    .iter()
                    .find_map(|marker| line.split_once(marker).map(|(_, rest)| rest))?
                    .trim_start();
                let product = if release
                    .split(|character: char| character == ',' || character.is_whitespace())
                    .next()
                    .is_some_and(|token| token.contains("MariaDB"))
                {
                    Product::MariaDb
                } else {
                    Product::MySql
                };
                Some(Version {
                    product,
                    numbers: leading_numbers(release)?,
                })
            }
        }
    }

    /// The part of the version that decides compatibility: PostgreSQL 10 and
    /// later are major versions on their own, 9.6 is two numbers.
    fn release(&self) -> (u32, u32) {
        let [major, minor, _] = self.numbers;
        match self.product {
            Product::PostgreSql if major >= 10 => (major, 0),
            _ => (major, minor),
        }
    }

    pub fn release_label(&self) -> String {
        let (major, minor) = self.release();
        match self.product {
            Product::PostgreSql if major >= 10 => major.to_string(),
            _ => format!("{major}.{minor}"),
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [major, minor, patch] = self.numbers;
        match self.product {
            Product::PostgreSql if major >= 10 => write!(formatter, "{major}.{minor}"),
            _ => write!(formatter, "{major}.{minor}.{patch}"),
        }
    }
}

/// Up to three dot separated numbers at the start of `text`; missing parts
/// are zero.
fn leading_numbers(text: &str) -> Option<[u32; 3]> {
    let mut numbers = [0u32; 3];
    let mut parsed = 0;
    for part in text.split('.') {
        let digits: String = part
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            break;
        }
        let Some(slot) = numbers.get_mut(parsed) else {
            break;
        };
        *slot = digits.parse().ok()?;
        parsed += 1;
        if digits.len() != part.len() {
            break;
        }
    }
    (parsed > 0).then_some(numbers)
}

/// What the version check concluded for a tool against the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compatibility {
    Compatible,
    /// The tool works but the result may be incomplete or need extra care.
    Warning(String),
    /// The tool refuses to work with this server.
    Incompatible(String),
}

impl Compatibility {
    pub fn blocks(&self) -> bool {
        matches!(self, Compatibility::Incompatible(_))
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Compatibility::Compatible => None,
            Compatibility::Warning(message) | Compatibility::Incompatible(message) => Some(message),
        }
    }
}

pub fn compatibility(program: ToolProgram, tool: &Version, server: &Version) -> Compatibility {
    let name = program.display_name();
    match program.family() {
        ClientTools::PostgreSql => {
            if tool.release() >= server.release() {
                return Compatibility::Compatible;
            }
            let explanation = format!(
                "{name} {} is older than the PostgreSQL {} server.",
                tool.release_label(),
                server.release_label(),
            );
            if program == ToolProgram::PgDump {
                Compatibility::Incompatible(format!(
                    "{explanation} pg_dump refuses to dump a newer server; install the PostgreSQL {} client tools or set the path to a newer pg_dump in Settings.",
                    server.release_label()
                ))
            } else {
                Compatibility::Warning(format!(
                    "{explanation} It may not understand everything in a dump made by a newer pg_dump."
                ))
            }
        }
        ClientTools::MySql => {
            if tool.product != server.product {
                return Compatibility::Warning(format!(
                    "{name} comes from {} {} but the server is {} {}. Most dumps work across the two, but server specific features may be left out.",
                    tool.product.display_name(),
                    tool.release_label(),
                    server.product.display_name(),
                    server.release_label(),
                ));
            }
            match tool.release().cmp(&server.release()) {
                Ordering::Less => Compatibility::Warning(format!(
                    "{name} {} is older than the {} {} server. Install newer client tools if the dump misses newer features.",
                    tool.release_label(),
                    server.product.display_name(),
                    server.release_label(),
                )),
                Ordering::Equal | Ordering::Greater => Compatibility::Compatible,
            }
        }
    }
}

/// A located program and the version it reported.
#[derive(Clone, Debug)]
pub struct LocatedTool {
    pub program: ToolProgram,
    pub path: PathBuf,
    pub version: Version,
}

/// Find `program`. A configured path is used as is. Otherwise every copy on
/// `PATH` and in the usual install locations is asked for its version, and the
/// one that suits `server` best wins: the server's own product first, then the
/// newest release. GUI apps on macOS start without the shell's `PATH`, which
/// is why the install locations are searched as well.
pub async fn locate(
    program: ToolProgram,
    settings: &ClientToolSettings,
    server: Option<&Version>,
) -> Result<LocatedTool> {
    let configured = program.configured_path(settings).trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        let version = query_version(program, &path).await.with_context(|| {
            format!(
                "The {} path in Settings, {}, does not work",
                program.display_name(),
                path.display()
            )
        })?;
        return Ok(LocatedTool {
            program,
            path,
            version,
        });
    }

    let mut best: Option<LocatedTool> = None;
    for path in candidate_paths(program) {
        let version = match query_version(program, &path).await {
            Ok(version) => version,
            Err(error) => {
                tracing::debug!("Skipping {}: {error:#}", path.display());
                continue;
            }
        };
        let candidate = LocatedTool {
            program,
            path,
            version,
        };
        let better = match &best {
            None => true,
            Some(current) => preference(&candidate.version, server)
                .cmp(&preference(&current.version, server))
                .is_gt(),
        };
        if better {
            best = Some(candidate);
        }
    }
    best.ok_or_else(|| {
        anyhow!(
            "{} was not found. Install the {} client tools or set the path to {} in Settings.",
            program.display_name(),
            match program.family() {
                ClientTools::PostgreSql => "PostgreSQL",
                ClientTools::MySql => "MySQL or MariaDB",
            },
            program.display_name()
        )
    })
}

fn preference(version: &Version, server: Option<&Version>) -> (bool, [u32; 3]) {
    let same_product = server.is_none_or(|server| server.product == version.product);
    (same_product, version.numbers)
}

async fn query_version(program: ToolProgram, path: &Path) -> Result<Version> {
    let mut command = smol::process::Command::new(path);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    super::runner::hide_console_window(&mut command);
    let output = futures::future::select(
        Box::pin(command.output()),
        Box::pin(smol::Timer::after(Duration::from_secs(10))),
    )
    .await;
    let output = match output {
        futures::future::Either::Left((output, _)) => output?,
        futures::future::Either::Right(_) => {
            return Err(anyhow!("{} --version did not answer", path.display()));
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    Version::parse_tool(program.family(), &stdout).ok_or_else(|| {
        anyhow!(
            "{} printed an unexpected version: {}",
            path.display(),
            stdout.trim()
        )
    })
}

/// Every existing executable for `program` on `PATH` and in the platform's
/// usual install directories, without duplicates. Downloaded tools would join
/// this list as one more directory.
fn candidate_paths(program: ToolProgram) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    directories.extend(install_directories(program.family()));

    let mut paths: Vec<PathBuf> = Vec::new();
    for directory in directories {
        for name in program.executable_names() {
            let path = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if !path.is_file() {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if paths.iter().any(|existing| {
                std::fs::canonicalize(existing).unwrap_or_else(|_| existing.clone()) == canonical
            }) {
                continue;
            }
            paths.push(path);
        }
    }
    paths
}

fn install_directories(tools: ClientTools) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if cfg!(target_os = "macos") {
        let fixed: &[&str] = match tools {
            ClientTools::PostgreSql => &[
                "/opt/homebrew/bin",
                "/opt/homebrew/opt/libpq/bin",
                "/usr/local/bin",
                "/usr/local/opt/libpq/bin",
                "/Applications/Postgres.app/Contents/Versions/latest/bin",
            ],
            ClientTools::MySql => &[
                "/opt/homebrew/bin",
                "/opt/homebrew/opt/mysql-client/bin",
                "/usr/local/bin",
                "/usr/local/opt/mysql-client/bin",
                "/usr/local/mysql/bin",
            ],
        };
        directories.extend(fixed.iter().map(PathBuf::from));
        if tools == ClientTools::PostgreSql {
            directories.extend(versioned_directories(
                Path::new("/opt/homebrew/opt"),
                "postgresql@",
                "bin",
            ));
        }
    } else if cfg!(target_os = "windows") {
        let program_files = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        match tools {
            ClientTools::PostgreSql => {
                directories.extend(versioned_directories(
                    &program_files.join("PostgreSQL"),
                    "",
                    "bin",
                ));
            }
            ClientTools::MySql => {
                directories.extend(versioned_directories(
                    &program_files.join("MySQL"),
                    "MySQL",
                    "bin",
                ));
                directories.extend(versioned_directories(&program_files, "MariaDB", "bin"));
            }
        }
    } else if tools == ClientTools::PostgreSql {
        directories.extend(versioned_directories(
            Path::new("/usr/lib/postgresql"),
            "",
            "bin",
        ));
        directories.extend(versioned_directories(Path::new("/usr"), "pgsql-", "bin"));
    }
    directories
}

/// `parent/<prefix>*/<child>` for every entry of `parent` whose name starts
/// with `prefix`, such as `/usr/lib/postgresql/16/bin`.
fn versioned_directories(parent: &Path, prefix: &str, child: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut directories: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .map(|entry| entry.path().join(child))
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    directories
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(product: Product, numbers: [u32; 3]) -> Version {
        Version { product, numbers }
    }

    #[test]
    fn parses_postgres_tool_versions() {
        assert_eq!(
            Version::parse_tool(
                ClientTools::PostgreSql,
                "pg_dump (PostgreSQL) 16.15 (Ubuntu 16.15-0ubuntu0.24.04.1)\n"
            ),
            Some(version(Product::PostgreSql, [16, 15, 0]))
        );
        assert_eq!(
            Version::parse_tool(ClientTools::PostgreSql, "psql (PostgreSQL) 9.6.24"),
            Some(version(Product::PostgreSql, [9, 6, 24]))
        );
        assert_eq!(
            Version::parse_tool(ClientTools::PostgreSql, "pg_restore (PostgreSQL) 17beta2"),
            Some(version(Product::PostgreSql, [17, 0, 0]))
        );
        assert_eq!(
            Version::parse_tool(ClientTools::PostgreSql, "something else"),
            None
        );
    }

    #[test]
    fn parses_mysql_and_mariadb_tool_versions() {
        let cases = [
            (
                "mysqldump  Ver 8.0.36 for Linux on x86_64 (MySQL Community Server - GPL)",
                version(Product::MySql, [8, 0, 36]),
            ),
            (
                "mysqldump  Ver 10.13 Distrib 5.7.44, for Linux (x86_64)",
                version(Product::MySql, [5, 7, 44]),
            ),
            (
                "mysqldump  Ver 10.19 Distrib 10.11.14-MariaDB, for debian-linux-gnu (x86_64)",
                version(Product::MariaDb, [10, 11, 14]),
            ),
            (
                "mariadb-dump from 11.4.2-MariaDB, client 10.19 for Linux (x86_64)",
                version(Product::MariaDb, [11, 4, 2]),
            ),
            (
                "mysql  Ver 8.4.0 for macos14 on arm64 (Homebrew)",
                version(Product::MySql, [8, 4, 0]),
            ),
        ];
        for (output, expected) in cases {
            assert_eq!(
                Version::parse_tool(ClientTools::MySql, output),
                Some(expected),
                "{output}"
            );
        }
    }

    #[test]
    fn parses_server_versions() {
        assert_eq!(
            Version::parse_server(
                ClientTools::PostgreSql,
                "16.15 (Ubuntu 16.15-0ubuntu0.24.04.1)"
            ),
            Some(version(Product::PostgreSql, [16, 15, 0]))
        );
        assert_eq!(
            Version::parse_server(ClientTools::MySql, "10.11.14-MariaDB-0ubuntu0.24.04.1"),
            Some(version(Product::MariaDb, [10, 11, 14]))
        );
        assert_eq!(
            Version::parse_server(ClientTools::MySql, "8.0.36-0ubuntu0.22.04.1"),
            Some(version(Product::MySql, [8, 0, 36]))
        );
    }

    #[test]
    fn postgres_compatibility_follows_the_major_release() {
        let server = version(Product::PostgreSql, [16, 2, 0]);
        assert_eq!(
            compatibility(
                ToolProgram::PgDump,
                &version(Product::PostgreSql, [16, 0, 0]),
                &server
            ),
            Compatibility::Compatible
        );
        assert_eq!(
            compatibility(
                ToolProgram::PgDump,
                &version(Product::PostgreSql, [17, 1, 0]),
                &server
            ),
            Compatibility::Compatible
        );
        let too_old = compatibility(
            ToolProgram::PgDump,
            &version(Product::PostgreSql, [15, 9, 0]),
            &server,
        );
        assert!(too_old.blocks());
        assert!(too_old.message().is_some_and(
            |message| message.contains("pg_dump 15") && message.contains("PostgreSQL 16")
        ));
        let restore = compatibility(
            ToolProgram::PgRestore,
            &version(Product::PostgreSql, [15, 9, 0]),
            &server,
        );
        assert!(matches!(restore, Compatibility::Warning(_)));

        let old_server = version(Product::PostgreSql, [9, 6, 24]);
        assert!(
            compatibility(
                ToolProgram::PgDump,
                &version(Product::PostgreSql, [9, 5, 0]),
                &old_server
            )
            .blocks()
        );
        assert_eq!(
            compatibility(
                ToolProgram::PgDump,
                &version(Product::PostgreSql, [10, 0, 0]),
                &old_server
            ),
            Compatibility::Compatible
        );
    }

    #[test]
    fn mysql_compatibility_warns_on_older_tools_and_mixed_products() {
        let server = version(Product::MySql, [8, 0, 36]);
        assert_eq!(
            compatibility(
                ToolProgram::MysqlDump,
                &version(Product::MySql, [8, 4, 0]),
                &server
            ),
            Compatibility::Compatible
        );
        assert!(matches!(
            compatibility(
                ToolProgram::MysqlDump,
                &version(Product::MySql, [5, 7, 44]),
                &server
            ),
            Compatibility::Warning(_)
        ));
        let mixed = compatibility(
            ToolProgram::MysqlDump,
            &version(Product::MariaDb, [10, 11, 6]),
            &server,
        );
        assert!(!mixed.blocks());
        assert!(
            mixed
                .message()
                .is_some_and(|message| message.contains("MariaDB"))
        );
    }

    #[test]
    fn prefers_the_server_product_then_the_newest_release() {
        let server = version(Product::MariaDb, [10, 11, 0]);
        let mysql = version(Product::MySql, [8, 4, 0]);
        let mariadb = version(Product::MariaDb, [10, 6, 0]);
        assert!(preference(&mariadb, Some(&server)) > preference(&mysql, Some(&server)));
        assert!(
            preference(&version(Product::MySql, [8, 4, 0]), None)
                > preference(&version(Product::MySql, [8, 0, 36]), None)
        );
    }
}
