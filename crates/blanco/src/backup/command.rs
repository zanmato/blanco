use anyhow::{Context as _, Result, anyhow, bail};
use blanco_core::ClientTools;
use chrono::{DateTime, TimeZone};
use database::ClientToolEndpoint;
use std::path::{Path, PathBuf};

use super::tools::{LocatedTool, Product, ToolProgram, Version};

/// The address the SSH tunnel listens on (`database::SshTunnel`).
const TUNNEL_ADDRESS: &str = "127.0.0.1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupFormat {
    /// A SQL script, restored with `psql` or `mysql`.
    Plain,
    /// pg_dump's compressed archive, restored with `pg_restore`.
    Custom,
    /// pg_dump's archive as a folder with one file per table.
    Directory,
}

impl BackupFormat {
    pub fn available(tools: ClientTools) -> &'static [BackupFormat] {
        match tools {
            ClientTools::PostgreSql => &[
                BackupFormat::Plain,
                BackupFormat::Custom,
                BackupFormat::Directory,
            ],
            ClientTools::MySql => &[BackupFormat::Plain],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BackupFormat::Plain => "Plain SQL",
            BackupFormat::Custom => "Custom archive",
            BackupFormat::Directory => "Directory archive",
        }
    }

    /// Appended to the expanded file name pattern. A directory archive is a
    /// folder and gets none.
    pub fn extension(self, compression: Compression) -> &'static str {
        match (self, compression.level()) {
            (BackupFormat::Plain, None) => ".sql",
            (BackupFormat::Plain, Some(_)) => ".sql.gz",
            (BackupFormat::Custom, _) => ".dump",
            (BackupFormat::Directory, _) => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Fast,
    Default,
    Best,
}

impl Compression {
    pub const ALL: [Compression; 4] = [
        Compression::None,
        Compression::Fast,
        Compression::Default,
        Compression::Best,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Compression::None => "None",
            Compression::Fast => "Fast",
            Compression::Default => "Default",
            Compression::Best => "Smallest",
        }
    }

    /// gzip level, `None` for an uncompressed dump.
    pub fn level(self) -> Option<u32> {
        match self {
            Compression::None => None,
            Compression::Fast => Some(1),
            Compression::Default => Some(6),
            Compression::Best => Some(9),
        }
    }
}

/// A table offered in the backup dialog. `name` is schema qualified on
/// PostgreSQL (`public.users`) and bare on MySQL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableChoice {
    pub schema: Option<String>,
    pub name: String,
    pub include: bool,
    pub structure_only: bool,
}

impl TableChoice {
    pub fn new(schema: Option<String>, name: String) -> Self {
        Self {
            schema,
            name,
            include: true,
            structure_only: false,
        }
    }

    pub fn display_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackupRequest {
    pub format: BackupFormat,
    pub compression: Compression,
    pub tables: Vec<TableChoice>,
    pub extra_arguments: Vec<String>,
    pub output: PathBuf,
}

/// Where a step's standard output goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardOutput {
    /// Shown as live output.
    Log,
    /// Appended to the plan's output file: the dump itself.
    Dump,
}

/// One run of a tool. The password and other connection settings that must
/// not show up in a process listing travel in `environment`.
#[derive(Clone, Debug)]
pub struct ToolCommand {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub standard_output: StandardOutput,
}

impl ToolCommand {
    /// The command line as shown in the live output. The password is never
    /// part of it.
    pub fn display(&self) -> String {
        let program = self
            .program
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.program.display().to_string());
        std::iter::once(program)
            .chain(self.arguments.iter().map(|argument| {
                shlex::try_quote(argument)
                    .map(|quoted| quoted.into_owned())
                    .unwrap_or_else(|_| argument.clone())
            }))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// How the progress bar is driven.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressSource {
    /// Bytes of the input file fed to the tool, out of `total`.
    InputBytes {
        total: u64,
    },
    /// Output lines that `counted` recognizes, one per unit of work, out of
    /// `total`.
    Lines {
        counted: ProgressLine,
        total: usize,
    },
    Unknown,
}

/// The line a tool prints in verbose mode as it starts on the next unit of
/// work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressLine {
    /// `pg_dump: dumping contents of table "public.users"`
    PgDumpTable,
    /// `-- Retrieving table structure for table users...`
    MysqlDumpTable,
    /// `pg_restore: creating TABLE "public.users"` and its siblings, one per
    /// archive entry.
    PgRestoreEntry,
}

impl ProgressLine {
    pub fn matches(self, line: &str) -> bool {
        match self {
            ProgressLine::PgDumpTable => line.starts_with("pg_dump: dumping contents of table "),
            ProgressLine::MysqlDumpTable => {
                line.starts_with("-- Retrieving table structure for table ")
            }
            ProgressLine::PgRestoreEntry => line.strip_prefix("pg_restore: ").is_some_and(|rest| {
                rest.starts_with("creating ")
                    || rest.starts_with("processing data for table ")
                    || rest.starts_with("executing ")
            }),
        }
    }
}

/// What the restore reads from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    None,
    /// The file is fed to the tool's standard input, decompressed on the way
    /// when `gzip` is set.
    File {
        path: PathBuf,
        gzip: bool,
    },
}

#[derive(Clone, Debug)]
pub struct ToolPlan {
    pub steps: Vec<ToolCommand>,
    pub input: Input,
    /// Receives the steps' `StandardOutput::Dump`, gzip compressed at the
    /// given level.
    pub dump_file: Option<(PathBuf, Option<u32>)>,
    /// Removed when the run fails or is stopped, so a partial backup is not
    /// mistaken for a complete one.
    pub remove_on_failure: Option<PathBuf>,
    pub progress: ProgressSource,
}

pub fn backup_plan(
    tool: &LocatedTool,
    server: Option<&Version>,
    endpoint: &ClientToolEndpoint,
    request: &BackupRequest,
) -> Result<ToolPlan> {
    let (connection_arguments, environment) = connection_settings(tool, endpoint)?;
    let excluded = request.tables.iter().filter(|table| !table.include);
    let structure_only: Vec<&TableChoice> = request
        .tables
        .iter()
        .filter(|table| table.include && table.structure_only)
        .collect();
    let with_data = request
        .tables
        .iter()
        .filter(|table| table.include && !table.structure_only)
        .count();
    let included = request.tables.iter().filter(|table| table.include).count();
    if !request.tables.is_empty() && included == 0 {
        bail!("Choose at least one table to back up");
    }

    match tool.program.family() {
        ClientTools::PostgreSql => {
            let format = match request.format {
                BackupFormat::Plain => "plain",
                BackupFormat::Custom => "custom",
                BackupFormat::Directory => "directory",
            };
            let mut arguments = connection_arguments;
            arguments.extend([
                format!("--format={format}"),
                format!("--file={}", request.output.display()),
                "--verbose".to_string(),
            ]);
            match (request.format, request.compression.level()) {
                (BackupFormat::Plain, None) => {}
                (_, level) => arguments.push(format!("--compress={}", level.unwrap_or(0))),
            }
            arguments.extend(excluded.map(|table| format!("--exclude-table={}", pattern(table))));
            arguments.extend(
                structure_only
                    .iter()
                    .map(|table| format!("--exclude-table-data={}", pattern(table))),
            );
            arguments.extend(request.extra_arguments.iter().cloned());
            arguments.push(endpoint.database.clone());
            Ok(ToolPlan {
                steps: vec![ToolCommand {
                    program: tool.path.clone(),
                    arguments,
                    environment,
                    standard_output: StandardOutput::Log,
                }],
                input: Input::None,
                dump_file: None,
                remove_on_failure: Some(request.output.clone()),
                progress: progress_over_tables(ProgressLine::PgDumpTable, with_data, request),
            })
        }
        ClientTools::MySql => {
            if request.format != BackupFormat::Plain {
                bail!("{} only writes plain SQL", tool.program.display_name());
            }
            let database = &endpoint.database;
            let mut common = connection_arguments;
            common.push("--verbose".to_string());
            if needs_column_statistics_off(&tool.version, server) {
                common.push("--column-statistics=0".to_string());
            }

            let mut arguments = common.clone();
            arguments.extend([
                "--single-transaction".to_string(),
                "--routines".to_string(),
                "--triggers".to_string(),
            ]);
            arguments.extend(
                excluded
                    .chain(structure_only.iter().copied())
                    .map(|table| format!("--ignore-table={database}.{}", table.name)),
            );
            arguments.extend(request.extra_arguments.iter().cloned());
            arguments.push(database.clone());
            let mut steps = vec![ToolCommand {
                program: tool.path.clone(),
                arguments,
                environment: environment.clone(),
                standard_output: StandardOutput::Dump,
            }];

            // mysqldump has no per-table "structure only" switch that both
            // MySQL and MariaDB understand, so those tables are dumped by a
            // second run that appends their definitions.
            if !structure_only.is_empty() {
                let mut arguments = common;
                arguments.extend(["--no-data".to_string(), "--triggers".to_string()]);
                arguments.extend(request.extra_arguments.iter().cloned());
                arguments.push(database.clone());
                arguments.extend(structure_only.iter().map(|table| table.name.clone()));
                steps.push(ToolCommand {
                    program: tool.path.clone(),
                    arguments,
                    environment,
                    standard_output: StandardOutput::Dump,
                });
            }

            Ok(ToolPlan {
                steps,
                input: Input::None,
                dump_file: Some((request.output.clone(), request.compression.level())),
                remove_on_failure: Some(request.output.clone()),
                progress: progress_over_tables(ProgressLine::MysqlDumpTable, included, request),
            })
        }
    }
}

fn progress_over_tables(
    counted: ProgressLine,
    total: usize,
    request: &BackupRequest,
) -> ProgressSource {
    if request.tables.is_empty() || total == 0 {
        ProgressSource::Unknown
    } else {
        ProgressSource::Lines { counted, total }
    }
}

/// A pg_dump table pattern that matches exactly this table: quoted names
/// keep their case and pattern characters like `*` lose their meaning.
fn pattern(table: &TableChoice) -> String {
    let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
    match &table.schema {
        Some(schema) => format!("{}.{}", quote(schema), quote(&table.name)),
        None => quote(&table.name),
    }
}

/// MySQL 8's mysqldump reads `information_schema.COLUMN_STATISTICS`, which
/// MariaDB and MySQL before 8.0 lack, and gives up without this switch.
fn needs_column_statistics_off(tool: &Version, server: Option<&Version>) -> bool {
    tool.product == Product::MySql
        && tool.numbers[0] >= 8
        && server.is_some_and(|server| server.product == Product::MariaDb || server.numbers[0] < 8)
}

/// The kind of file a restore reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DumpFormat {
    Plain,
    Custom,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpFile {
    pub path: PathBuf,
    pub format: DumpFormat,
    pub gzip: bool,
    pub size: u64,
}

impl DumpFile {
    pub fn description(&self) -> String {
        let format = match self.format {
            DumpFormat::Plain => "SQL script",
            DumpFormat::Custom => "pg_dump custom archive",
            DumpFormat::Directory => "pg_dump directory archive",
        };
        if self.gzip {
            format!("{format}, gzip compressed")
        } else {
            format.to_string()
        }
    }

    /// The program that restores this dump.
    pub fn program(&self, tools: ClientTools) -> Result<ToolProgram> {
        match (tools, self.format) {
            (ClientTools::PostgreSql, DumpFormat::Plain) => Ok(ToolProgram::Psql),
            (ClientTools::PostgreSql, DumpFormat::Custom | DumpFormat::Directory) => {
                Ok(ToolProgram::PgRestore)
            }
            (ClientTools::MySql, DumpFormat::Plain) => Ok(ToolProgram::Mysql),
            (ClientTools::MySql, DumpFormat::Custom | DumpFormat::Directory) => Err(anyhow!(
                "This is a pg_dump archive. Only a PostgreSQL connection can restore it."
            )),
        }
    }
}

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const PG_ARCHIVE_MAGIC: &[u8] = b"PGDMP";

/// Look at the first bytes of `path` to tell a SQL script from a pg_dump
/// archive, looking through gzip compression.
pub fn detect_dump(path: &Path) -> Result<DumpFile> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    if metadata.is_dir() {
        if !path.join("toc.dat").is_file() {
            bail!(
                "{} is a folder but not a pg_dump directory archive",
                path.display()
            );
        }
        return Ok(DumpFile {
            path: path.to_path_buf(),
            format: DumpFormat::Directory,
            gzip: false,
            size: 0,
        });
    }

    let mut file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let mut head = [0u8; 5];
    let read = read_up_to(&mut file, &mut head)?;
    let head = head.get(..read).unwrap_or_default();
    let gzip = head.starts_with(&GZIP_MAGIC);
    let content_head = if gzip {
        let mut decoder = flate2::read::GzDecoder::new(
            std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?,
        );
        let mut content = [0u8; 5];
        let read = read_up_to(&mut decoder, &mut content)
            .with_context(|| format!("{} is not a readable gzip file", path.display()))?;
        content.get(..read).unwrap_or_default().to_vec()
    } else {
        head.to_vec()
    };
    let format = if content_head.starts_with(PG_ARCHIVE_MAGIC) {
        DumpFormat::Custom
    } else {
        DumpFormat::Plain
    };
    Ok(DumpFile {
        path: path.to_path_buf(),
        format,
        gzip,
        size: metadata.len(),
    })
}

fn read_up_to(reader: &mut impl std::io::Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let Some(rest) = buffer.get_mut(filled..) else {
            break;
        };
        match reader.read(rest)? {
            0 => break,
            count => filled += count,
        }
    }
    Ok(filled)
}

/// `entries` is the number of entries `pg_restore --list` printed for a
/// custom archive read from its path, used to drive the progress bar.
pub fn restore_plan(
    tool: &LocatedTool,
    endpoint: &ClientToolEndpoint,
    dump: &DumpFile,
    extra_arguments: &[String],
    entries: Option<usize>,
) -> Result<ToolPlan> {
    let (mut arguments, environment) = connection_settings(tool, endpoint)?;
    let database = endpoint.database.clone();
    let streamed = Input::File {
        path: dump.path.clone(),
        gzip: dump.gzip,
    };
    let bytes = ProgressSource::InputBytes { total: dump.size };

    let (input, progress) = match tool.program {
        ToolProgram::Psql => {
            arguments.extend([
                "--no-psqlrc".to_string(),
                "--set=ON_ERROR_STOP=1".to_string(),
                format!("--dbname={}", database),
            ]);
            arguments.extend(extra_arguments.iter().cloned());
            (streamed, bytes)
        }
        ToolProgram::Mysql => {
            arguments.push(format!("--database={database}"));
            arguments.extend(extra_arguments.iter().cloned());
            (streamed, bytes)
        }
        ToolProgram::PgRestore => {
            arguments.extend(["--verbose".to_string(), format!("--dbname={database}")]);
            arguments.extend(extra_arguments.iter().cloned());
            // A compressed archive is decompressed into pg_restore's standard
            // input, which it reads front to back; a plain archive is opened
            // by path so pg_restore can seek in it.
            if dump.gzip {
                (streamed, bytes)
            } else {
                arguments.push(dump.path.display().to_string());
                let progress = match entries {
                    Some(total) if total > 0 => ProgressSource::Lines {
                        counted: ProgressLine::PgRestoreEntry,
                        total,
                    },
                    _ => ProgressSource::Unknown,
                };
                (Input::None, progress)
            }
        }
        ToolProgram::PgDump | ToolProgram::MysqlDump => {
            bail!("{} cannot restore", tool.program.display_name())
        }
    };

    Ok(ToolPlan {
        steps: vec![ToolCommand {
            program: tool.path.clone(),
            arguments,
            environment,
            standard_output: StandardOutput::Log,
        }],
        input,
        dump_file: None,
        remove_on_failure: None,
        progress,
    })
}

/// The number of entries in a `pg_restore --list` listing; comment lines
/// start with `;`.
pub fn count_archive_entries(listing: &str) -> usize {
    listing
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with(';'))
        .count()
}

/// Arguments and environment that point a tool at `endpoint`. Through an SSH
/// tunnel the tool connects to the tunnel's local port. PostgreSQL's
/// `verify-full` keeps the real host name to check the certificate against and
/// sends the connection to the tunnel with `PGHOSTADDR`.
pub fn connection_settings(
    tool: &LocatedTool,
    endpoint: &ClientToolEndpoint,
) -> Result<(Vec<String>, Vec<(String, String)>)> {
    let config = &endpoint.config;
    let ssl_mode = config.ssl_mode.as_deref().filter(|mode| !mode.is_empty());
    let verifies_host = ssl_mode == Some("verify-full");
    let (host, port) = match endpoint.tunnel_port {
        Some(port) if verifies_host && tool.program.family() == ClientTools::PostgreSql => {
            (config.host.clone(), port)
        }
        Some(_) if verifies_host => bail!(
            "verify-full cannot check the server's name through an SSH tunnel with {}. Use verify-ca for this connection to back up or restore it.",
            tool.program.display_name()
        ),
        Some(port) => (TUNNEL_ADDRESS.to_string(), port),
        None => (config.host.clone(), config.port),
    };
    let password = config
        .password
        .clone()
        .filter(|password| !password.is_empty());

    let mut arguments = Vec::new();
    let mut environment = Vec::new();
    match tool.program.family() {
        ClientTools::PostgreSql => {
            arguments.extend([
                format!("--host={host}"),
                format!("--port={port}"),
                format!("--username={}", config.username),
                "--no-password".to_string(),
            ]);
            if let Some(password) = password {
                environment.push(("PGPASSWORD".to_string(), password));
            }
            if endpoint.tunnel_port.is_some() && verifies_host {
                environment.push(("PGHOSTADDR".to_string(), TUNNEL_ADDRESS.to_string()));
            }
            if let Some(mode) = ssl_mode {
                let mode = match mode {
                    "disabled" => "disable",
                    "preferred" => "prefer",
                    "required" => "require",
                    other => other,
                };
                environment.push(("PGSSLMODE".to_string(), mode.to_string()));
            }
            for (variable, path) in [
                ("PGSSLCERT", &config.ssl_cert_path),
                ("PGSSLKEY", &config.ssl_key_path),
                ("PGSSLROOTCERT", &config.ssl_ca_cert_path),
            ] {
                if let Some(path) = path.as_ref().filter(|path| !path.is_empty()) {
                    environment.push((variable.to_string(), path.clone()));
                }
            }
            environment.push(("PGAPPNAME".to_string(), "Blanco".to_string()));
            environment.push((
                "PGCONNECT_TIMEOUT".to_string(),
                blanco_core::connect_timeout().as_secs().to_string(),
            ));
        }
        ClientTools::MySql => {
            arguments.extend([
                format!("--host={host}"),
                format!("--port={port}"),
                format!("--user={}", config.username),
                "--protocol=TCP".to_string(),
            ]);
            if let Some(password) = password {
                environment.push(("MYSQL_PWD".to_string(), password));
            }
            arguments.extend(mysql_ssl_arguments(tool.version.product, ssl_mode));
            for (option, path) in [
                ("--ssl-ca", &config.ssl_ca_cert_path),
                ("--ssl-cert", &config.ssl_cert_path),
                ("--ssl-key", &config.ssl_key_path),
            ] {
                if let Some(path) = path.as_ref().filter(|path| !path.is_empty()) {
                    arguments.push(format!("{option}={path}"));
                }
            }
        }
    }
    Ok((arguments, environment))
}

/// MySQL's tools take `--ssl-mode`, MariaDB's never did and spell the same
/// choices with `--ssl` and `--ssl-verify-server-cert`.
fn mysql_ssl_arguments(product: Product, ssl_mode: Option<&str>) -> Vec<String> {
    let Some(mode) = ssl_mode else {
        return Vec::new();
    };
    let arguments: &[&str] = match (product, mode) {
        (Product::MariaDb, "disabled") => &["--skip-ssl"],
        (Product::MariaDb, "preferred") => &["--skip-ssl-verify-server-cert"],
        (Product::MariaDb, "required") => &["--ssl", "--skip-ssl-verify-server-cert"],
        (Product::MariaDb, "verify-ca" | "verify-full") => &["--ssl", "--ssl-verify-server-cert"],
        (_, "disabled") => &["--ssl-mode=DISABLED"],
        (_, "preferred") => &["--ssl-mode=PREFERRED"],
        (_, "required") => &["--ssl-mode=REQUIRED"],
        (_, "verify-ca") => &["--ssl-mode=VERIFY_CA"],
        (_, "verify-full") => &["--ssl-mode=VERIFY_IDENTITY"],
        _ => &[],
    };
    arguments
        .iter()
        .map(|argument| argument.to_string())
        .collect()
}

/// Split the extra arguments field like a shell would, so quoted values may
/// contain spaces.
pub fn split_arguments(text: &str) -> Result<Vec<String>> {
    shlex::split(text).ok_or_else(|| anyhow!("The extra arguments have an unclosed quote"))
}

/// Expand a backup file name pattern. `{database}`, `{connection}`, `{date}`
/// (`2026-10-11`) and `{time}` (`14-05-09`) are replaced; dates are written
/// largest unit first so backups sort by name in time order. Characters that
/// are not allowed in file names become `_`.
pub fn expand_file_name<Zone: TimeZone>(
    pattern: &str,
    database: &str,
    connection: &str,
    now: &DateTime<Zone>,
) -> Result<String>
where
    Zone::Offset: std::fmt::Display,
{
    let pattern = if pattern.trim().is_empty() {
        crate::settings::BackupSettings::DEFAULT_FILE_NAME_PATTERN
    } else {
        pattern.trim()
    };
    let expanded = pattern
        .replace("{database}", database)
        .replace("{connection}", connection)
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{time}", &now.format("%H-%M-%S").to_string());
    let name: String = expanded
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let name = name.trim().trim_end_matches('.').to_string();
    if name.is_empty() {
        bail!("The file name is empty");
    }
    Ok(name)
}

/// `directory/name+extension`, or the first free `name_2+extension`,
/// `name_3+extension`, ... so an earlier backup is never overwritten.
pub fn unique_output_path(directory: &Path, name: &str, extension: &str) -> PathBuf {
    let first = directory.join(format!("{name}{extension}"));
    if !first.exists() {
        return first;
    }
    (2usize..)
        .map(|number| directory.join(format!("{name}_{number}{extension}")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use database::{ConnectionConfig, DatabaseType};

    const PASSWORD: &str = "s3cret pass";

    fn tool(program: ToolProgram, product: Product, numbers: [u32; 3]) -> LocatedTool {
        LocatedTool {
            program,
            path: PathBuf::from(format!("/usr/bin/{}", program.display_name())),
            version: Version { product, numbers },
        }
    }

    fn endpoint(db_type: DatabaseType, tunnel_port: Option<u16>) -> ClientToolEndpoint {
        let config = ConnectionConfig::new(
            1,
            "local".to_string(),
            db_type,
            "db.example.com".to_string(),
            5432,
            "app".to_string(),
            "admin".to_string(),
            Some(PASSWORD.to_string()),
        );
        ClientToolEndpoint {
            config,
            database: "app".to_string(),
            tunnel_port,
        }
    }

    fn request(format: BackupFormat, tables: Vec<TableChoice>) -> BackupRequest {
        BackupRequest {
            format,
            compression: Compression::None,
            tables,
            extra_arguments: Vec::new(),
            output: PathBuf::from("/backups/app_2026-10-11.sql"),
        }
    }

    fn assert_password_only_in_environment(plan: &ToolPlan, variable: &str) {
        for step in &plan.steps {
            assert!(
                step.arguments
                    .iter()
                    .all(|argument| !argument.contains(PASSWORD)),
                "password leaked into {:?}",
                step.arguments
            );
            assert!(!step.display().contains(PASSWORD));
            assert!(
                step.environment
                    .iter()
                    .any(|(name, value)| name == variable && value == PASSWORD)
            );
        }
    }

    #[test]
    fn pg_dump_plan_passes_the_password_in_the_environment() {
        let tool = tool(ToolProgram::PgDump, Product::PostgreSql, [16, 4, 0]);
        let mut tables = vec![
            TableChoice::new(Some("public".into()), "users".into()),
            TableChoice::new(Some("public".into()), "audit_log".into()),
            TableChoice::new(Some("Sales".into()), "Orders".into()),
        ];
        tables[1].structure_only = true;
        tables[2].include = false;
        let mut request = request(BackupFormat::Custom, tables);
        request.compression = Compression::Best;
        request.extra_arguments = vec!["--no-owner".into()];
        let plan = backup_plan(
            &tool,
            None,
            &endpoint(DatabaseType::PostgreSQL, None),
            &request,
        )
        .expect("plan");

        assert_password_only_in_environment(&plan, "PGPASSWORD");
        let step = &plan.steps[0];
        assert_eq!(
            step.arguments,
            vec![
                "--host=db.example.com",
                "--port=5432",
                "--username=admin",
                "--no-password",
                "--format=custom",
                "--file=/backups/app_2026-10-11.sql",
                "--verbose",
                "--compress=9",
                "--exclude-table=\"Sales\".\"Orders\"",
                "--exclude-table-data=\"public\".\"audit_log\"",
                "--no-owner",
                "app",
            ]
        );
        assert_eq!(step.standard_output, StandardOutput::Log);
        assert_eq!(plan.dump_file, None);
        assert_eq!(
            plan.progress,
            ProgressSource::Lines {
                counted: ProgressLine::PgDumpTable,
                total: 1
            }
        );
    }

    #[test]
    fn plain_pg_dump_without_compression_has_no_compress_switch() {
        let tool = tool(ToolProgram::PgDump, Product::PostgreSql, [16, 4, 0]);
        let plan = backup_plan(
            &tool,
            None,
            &endpoint(DatabaseType::PostgreSQL, None),
            &request(BackupFormat::Plain, Vec::new()),
        )
        .expect("plan");
        assert!(
            !plan.steps[0]
                .arguments
                .iter()
                .any(|argument| argument.starts_with("--compress"))
        );
        assert_eq!(plan.progress, ProgressSource::Unknown);
    }

    #[test]
    fn tunnel_points_postgres_tools_at_the_local_port() {
        let tool = tool(ToolProgram::PgDump, Product::PostgreSql, [16, 4, 0]);
        let mut endpoint = endpoint(DatabaseType::PostgreSQL, Some(15433));
        endpoint.config.ssl_mode = Some("required".into());
        let (arguments, environment) = connection_settings(&tool, &endpoint).expect("settings");
        assert!(arguments.contains(&"--host=127.0.0.1".to_string()));
        assert!(arguments.contains(&"--port=15433".to_string()));
        assert!(environment.contains(&("PGSSLMODE".into(), "require".into())));
        assert!(!environment.iter().any(|(name, _)| name == "PGHOSTADDR"));
    }

    #[test]
    fn verify_full_through_a_tunnel_keeps_the_host_name() {
        let tool = tool(ToolProgram::PgRestore, Product::PostgreSql, [16, 4, 0]);
        let mut endpoint = endpoint(DatabaseType::PostgreSQL, Some(15433));
        endpoint.config.ssl_mode = Some("verify-full".into());
        endpoint.config.ssl_ca_cert_path = Some("/certs/root.crt".into());
        let (arguments, environment) = connection_settings(&tool, &endpoint).expect("settings");
        assert!(arguments.contains(&"--host=db.example.com".to_string()));
        assert!(arguments.contains(&"--port=15433".to_string()));
        assert!(environment.contains(&("PGHOSTADDR".into(), "127.0.0.1".into())));
        assert!(environment.contains(&("PGSSLMODE".into(), "verify-full".into())));
        assert!(environment.contains(&("PGSSLROOTCERT".into(), "/certs/root.crt".into())));
    }

    #[test]
    fn mysql_verify_full_through_a_tunnel_is_refused() {
        let tool = tool(ToolProgram::MysqlDump, Product::MySql, [8, 0, 36]);
        let mut endpoint = endpoint(DatabaseType::MySQL, Some(15433));
        endpoint.config.ssl_mode = Some("verify-full".into());
        assert!(connection_settings(&tool, &endpoint).is_err());
        endpoint.config.ssl_mode = Some("verify-ca".into());
        let (arguments, _) = connection_settings(&tool, &endpoint).expect("settings");
        assert!(arguments.contains(&"--host=127.0.0.1".to_string()));
        assert!(arguments.contains(&"--ssl-mode=VERIFY_CA".to_string()));
    }

    #[test]
    fn mysqldump_plan_dumps_structure_only_tables_in_a_second_run() {
        let tool = tool(ToolProgram::MysqlDump, Product::MySql, [8, 0, 36]);
        let server = Version {
            product: Product::MariaDb,
            numbers: [10, 11, 6],
        };
        let mut tables = vec![
            TableChoice::new(None, "users".into()),
            TableChoice::new(None, "sessions".into()),
            TableChoice::new(None, "cache".into()),
        ];
        tables[1].structure_only = true;
        tables[2].include = false;
        let mut request = request(BackupFormat::Plain, tables);
        request.compression = Compression::Default;
        let mut endpoint = endpoint(DatabaseType::MySQL, None);
        endpoint.config.port = 3306;
        endpoint.config.ssl_mode = Some("required".into());
        let plan = backup_plan(&tool, Some(&server), &endpoint, &request).expect("plan");

        assert_password_only_in_environment(&plan, "MYSQL_PWD");
        assert_eq!(plan.steps.len(), 2);
        let first = &plan.steps[0];
        assert_eq!(
            first.arguments,
            vec![
                "--host=db.example.com",
                "--port=3306",
                "--user=admin",
                "--protocol=TCP",
                "--ssl-mode=REQUIRED",
                "--verbose",
                "--column-statistics=0",
                "--single-transaction",
                "--routines",
                "--triggers",
                "--ignore-table=app.cache",
                "--ignore-table=app.sessions",
                "app",
            ]
        );
        let second = &plan.steps[1];
        assert!(second.arguments.contains(&"--no-data".to_string()));
        assert_eq!(
            second.arguments.last().map(String::as_str),
            Some("sessions")
        );
        assert!(
            plan.steps
                .iter()
                .all(|step| step.standard_output == StandardOutput::Dump)
        );
        assert_eq!(
            plan.dump_file,
            Some((PathBuf::from("/backups/app_2026-10-11.sql"), Some(6)))
        );
    }

    #[test]
    fn mariadb_tools_spell_ssl_without_ssl_mode() {
        assert_eq!(
            mysql_ssl_arguments(Product::MariaDb, Some("disabled")),
            vec!["--skip-ssl"]
        );
        assert_eq!(
            mysql_ssl_arguments(Product::MariaDb, Some("verify-full")),
            vec!["--ssl", "--ssl-verify-server-cert"]
        );
        assert_eq!(
            mysql_ssl_arguments(Product::MySql, Some("verify-full")),
            vec!["--ssl-mode=VERIFY_IDENTITY"]
        );
        assert!(mysql_ssl_arguments(Product::MySql, None).is_empty());
    }

    #[test]
    fn column_statistics_is_only_turned_off_for_mysql_8_tools_on_other_servers() {
        let mysql_8 = Version {
            product: Product::MySql,
            numbers: [8, 0, 36],
        };
        let mariadb = Version {
            product: Product::MariaDb,
            numbers: [10, 11, 6],
        };
        let mysql_57 = Version {
            product: Product::MySql,
            numbers: [5, 7, 44],
        };
        assert!(needs_column_statistics_off(&mysql_8, Some(&mariadb)));
        assert!(needs_column_statistics_off(&mysql_8, Some(&mysql_57)));
        assert!(!needs_column_statistics_off(&mysql_8, Some(&mysql_8)));
        assert!(!needs_column_statistics_off(&mariadb, Some(&mysql_57)));
    }

    #[test]
    fn a_backup_needs_at_least_one_table() {
        let tool = tool(ToolProgram::PgDump, Product::PostgreSql, [16, 4, 0]);
        let mut table = TableChoice::new(Some("public".into()), "users".into());
        table.include = false;
        assert!(
            backup_plan(
                &tool,
                None,
                &endpoint(DatabaseType::PostgreSQL, None),
                &request(BackupFormat::Plain, vec![table]),
            )
            .is_err()
        );
    }

    #[test]
    fn restore_plans_use_the_matching_tool_and_input() {
        let endpoint = endpoint(DatabaseType::PostgreSQL, None);
        let plain = DumpFile {
            path: PathBuf::from("/backups/app.sql.gz"),
            format: DumpFormat::Plain,
            gzip: true,
            size: 1200,
        };
        let psql = tool(ToolProgram::Psql, Product::PostgreSql, [16, 4, 0]);
        let plan = restore_plan(&psql, &endpoint, &plain, &[], None).expect("plan");
        assert_password_only_in_environment(&plan, "PGPASSWORD");
        assert_eq!(
            plan.input,
            Input::File {
                path: plain.path.clone(),
                gzip: true
            }
        );
        assert_eq!(plan.progress, ProgressSource::InputBytes { total: 1200 });
        assert!(
            plan.steps[0]
                .arguments
                .contains(&"--set=ON_ERROR_STOP=1".to_string())
        );

        let custom = DumpFile {
            path: PathBuf::from("/backups/app.dump"),
            format: DumpFormat::Custom,
            gzip: false,
            size: 1200,
        };
        let pg_restore = tool(ToolProgram::PgRestore, Product::PostgreSql, [16, 4, 0]);
        let plan = restore_plan(
            &pg_restore,
            &endpoint,
            &custom,
            &["--clean".to_string()],
            Some(8),
        )
        .expect("plan");
        assert_eq!(plan.input, Input::None);
        assert_eq!(
            plan.steps[0].arguments.last().map(String::as_str),
            Some("/backups/app.dump")
        );
        assert!(plan.steps[0].arguments.contains(&"--clean".to_string()));
        assert_eq!(
            plan.progress,
            ProgressSource::Lines {
                counted: ProgressLine::PgRestoreEntry,
                total: 8
            }
        );

        assert!(custom.program(ClientTools::MySql).is_err());
        assert_eq!(
            plain.program(ClientTools::MySql).expect("program"),
            ToolProgram::Mysql
        );
    }

    #[test]
    fn detects_dump_formats_through_gzip() {
        use std::io::Write as _;
        let directory = tempfile::tempdir().expect("temp dir");

        let plain = directory.path().join("plain.sql");
        std::fs::write(&plain, "CREATE TABLE t (id int);\n").expect("write");
        let detected = detect_dump(&plain).expect("detect");
        assert_eq!(detected.format, DumpFormat::Plain);
        assert!(!detected.gzip);

        let archive = directory.path().join("archive.dump.gz");
        let mut encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).expect("create"),
            flate2::Compression::default(),
        );
        encoder.write_all(b"PGDMP\x01\x0f\x00").expect("write");
        encoder.finish().expect("finish");
        let detected = detect_dump(&archive).expect("detect");
        assert_eq!(detected.format, DumpFormat::Custom);
        assert!(detected.gzip);

        let folder = directory.path().join("folder");
        std::fs::create_dir(&folder).expect("mkdir");
        assert!(detect_dump(&folder).is_err());
        std::fs::write(folder.join("toc.dat"), b"PGDMP").expect("write");
        assert_eq!(
            detect_dump(&folder).expect("detect").format,
            DumpFormat::Directory
        );
    }

    #[test]
    fn file_name_pattern_uses_sortable_dates() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 3, 7, 9, 5, 2)
            .single()
            .expect("date");
        assert_eq!(
            expand_file_name("{database}_{date}", "app", "local", &now).expect("name"),
            "app_2026-03-07"
        );
        assert_eq!(
            expand_file_name("", "app", "local", &now).expect("name"),
            "app_2026-03-07"
        );
        assert_eq!(
            expand_file_name("{connection}/{database} {date}T{time}", "a:b", "prod", &now)
                .expect("name"),
            "prod_a_b 2026-03-07T09-05-02"
        );
        assert!(expand_file_name("...", "app", "local", &now).is_err());
    }

    #[test]
    fn output_path_never_overwrites_an_earlier_backup() {
        let directory = tempfile::tempdir().expect("temp dir");
        let first = unique_output_path(directory.path(), "app_2026-03-07", ".sql");
        assert_eq!(first, directory.path().join("app_2026-03-07.sql"));
        std::fs::write(&first, "").expect("write");
        assert_eq!(
            unique_output_path(directory.path(), "app_2026-03-07", ".sql"),
            directory.path().join("app_2026-03-07_2.sql")
        );
    }

    #[test]
    fn splits_extra_arguments_like_a_shell() {
        assert_eq!(
            split_arguments("--no-owner --exclude-schema='audit log'").expect("split"),
            vec!["--no-owner", "--exclude-schema=audit log"]
        );
        assert!(split_arguments("--x='open").is_err());
    }

    #[test]
    fn counts_archive_entries_and_restore_progress_lines() {
        let listing = ";\n; Archive created at 2026-10-11\n;\n6; 2615 16448 SCHEMA - s2 blanco\n216; 1259 16438 TABLE public a blanco\n";
        assert_eq!(count_archive_entries(listing), 2);
        let entry = ProgressLine::PgRestoreEntry;
        assert!(entry.matches("pg_restore: creating TABLE \"public.a\""));
        assert!(entry.matches("pg_restore: processing data for table \"public.a\""));
        assert!(!entry.matches("pg_restore: connecting to database for restore"));
        assert!(
            ProgressLine::PgDumpTable.matches("pg_dump: dumping contents of table \"public.a\"")
        );
        assert!(
            ProgressLine::MysqlDumpTable.matches("-- Retrieving table structure for table a...")
        );
    }
}
