//! Back up a database with the real client tools and restore it into a new
//! one, against the docker-compose servers. Skipped when a server is
//! unreachable (unless `BLANCO_RUN_DB_TESTS=1`) or a tool is not installed.
#![allow(clippy::print_stderr)]

use anyhow::{Context as _, Result, anyhow};
use blanco_core::ClientTools;
use database::{ConnectionConfig, DatabaseService, DatabaseType};
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use super::command::{
    BackupFormat, BackupRequest, Compression, DumpFormat, TableChoice, backup_plan,
    count_archive_entries, detect_dump, restore_plan,
};
use super::runner::{RunOutcome, capture_output, run_plan};
use super::tools::{LocatedTool, Version, locate};
use crate::settings::ClientToolSettings;

fn strict() -> bool {
    env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1")
}

fn env_or(prefix: &str, field: &str, default: &str) -> String {
    env::var(format!("BLANCO_{prefix}_{field}")).unwrap_or_else(|_| default.to_string())
}

struct Server {
    db_type: DatabaseType,
    tools: ClientTools,
    config: ConnectionConfig,
    /// Database the test connects to while it creates and drops its own.
    admin_database: String,
}

impl Server {
    fn postgres() -> Self {
        let mut config = ConnectionConfig::new(
            1,
            "backup-postgres".to_string(),
            DatabaseType::PostgreSQL,
            env_or("POSTGRES", "HOST", "localhost"),
            env_or("POSTGRES", "PORT", "5488").parse().unwrap_or(5488),
            env_or("POSTGRES", "DB", "blanco"),
            env_or("POSTGRES", "USER", "blanco"),
            Some(env_or("POSTGRES", "PASSWORD", "blanco")),
        );
        config.ssl_mode = Some("disabled".to_string());
        Self {
            db_type: DatabaseType::PostgreSQL,
            tools: ClientTools::PostgreSql,
            admin_database: config.database.clone(),
            config,
        }
    }

    fn mysql() -> Self {
        let config = ConnectionConfig::new(
            2,
            "backup-mysql".to_string(),
            DatabaseType::MySQL,
            env_or("MYSQL", "HOST", "localhost"),
            env_or("MYSQL", "PORT", "3306").parse().unwrap_or(3306),
            env_or("MYSQL", "DB", "blanco"),
            env_or("MYSQL", "USER", "blanco"),
            Some(env_or("MYSQL", "PASSWORD", "blanco")),
        );
        Self {
            db_type: DatabaseType::MySQL,
            tools: ClientTools::MySql,
            admin_database: config.database.clone(),
            config,
        }
    }
}

struct Fixture {
    service: DatabaseService,
    server: Server,
    version: Version,
    databases: Vec<String>,
    // Dropped last: the service's connections run on it.
    _runtime: tokio::runtime::Runtime,
}

impl Fixture {
    /// `None` when the server cannot be reached and the test is not strict.
    async fn connect(server: Server, runtime: tokio::runtime::Runtime) -> Result<Option<Self>> {
        let service = DatabaseService::new(runtime.handle().clone());
        service.add_connection_config(server.config.clone()).await;
        // Postgres connects lazily, so only a ping shows whether the server
        // is there.
        let reached = async {
            let connection = service
                .get_or_create_connection(server.config.id, Some(&server.admin_database))
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        };
        let connection = match reached.await {
            Ok(connection) => connection,
            Err(error) if !strict() => {
                eprintln!(
                    "skip {} backup round trip: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require.",
                    server.db_type
                );
                return Ok(None);
            }
            Err(error) => return Err(error.context("server unreachable")),
        };
        let result = connection
            .execute_query(
                server.tools.server_version_query(),
                Some(&server.admin_database),
                None,
            )
            .await?;
        let text = result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.clone())
            .ok_or_else(|| anyhow!("no server version"))?;
        let version = Version::parse_server(server.tools, &text)
            .ok_or_else(|| anyhow!("unexpected server version {text}"))?;
        Ok(Some(Self {
            service,
            server,
            version,
            databases: Vec::new(),
            _runtime: runtime,
        }))
    }

    async fn admin(&self, sql: &str) -> Result<()> {
        let connection = self
            .service
            .get_or_create_connection(self.server.config.id, Some(&self.server.admin_database))
            .await?;
        connection
            .execute_query(sql, Some(&self.server.admin_database), None)
            .await
            .with_context(|| sql.to_string())?;
        Ok(())
    }

    async fn create_database(&mut self, name: &str) -> Result<()> {
        self.admin(&format!("CREATE DATABASE {name}")).await?;
        self.databases.push(name.to_string());
        Ok(())
    }

    async fn execute(&self, database: &str, sql: &str) -> Result<()> {
        let connection = self
            .service
            .get_or_create_connection(self.server.config.id, Some(database))
            .await?;
        connection
            .execute_script(sql, Some(database))
            .await
            .with_context(|| sql.to_string())?;
        Ok(())
    }

    async fn single_value(&self, database: &str, sql: &str) -> Result<Option<String>> {
        let connection = self
            .service
            .get_or_create_connection(self.server.config.id, Some(database))
            .await?;
        let result = connection
            .execute_query(sql, Some(database), None)
            .await
            .with_context(|| sql.to_string())?;
        Ok(result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.clone()))
    }

    async fn locate(&self, program: super::tools::ToolProgram) -> Option<LocatedTool> {
        match locate(program, &ClientToolSettings::default(), Some(&self.version)).await {
            Ok(tool) => Some(tool),
            Err(error) => {
                eprintln!("skip backup round trip: {error:#}");
                None
            }
        }
    }

    async fn back_up(
        &self,
        tool: &LocatedTool,
        database: &str,
        format: BackupFormat,
        compression: Compression,
        tables: Vec<TableChoice>,
        output: &Path,
    ) -> Result<()> {
        let endpoint = self
            .service
            .client_tool_endpoint(self.server.config.id, database)
            .await?;
        let plan = backup_plan(
            tool,
            Some(&self.version),
            &endpoint,
            &BackupRequest {
                format,
                compression,
                tables,
                extra_arguments: Vec::new(),
                output: output.to_path_buf(),
            },
        )?;
        run(&plan).await
    }

    async fn restore(&self, tool: &LocatedTool, database: &str, dump: &Path) -> Result<()> {
        let dump = detect_dump(dump)?;
        let entries = if dump.format == DumpFormat::Plain || dump.gzip {
            None
        } else {
            Some(count_archive_entries(
                &capture_output(
                    &tool.path,
                    &["--list".to_string(), dump.path.display().to_string()],
                )
                .await?,
            ))
        };
        let endpoint = self
            .service
            .client_tool_endpoint(self.server.config.id, database)
            .await?;
        let plan = restore_plan(tool, &endpoint, &dump, &[], entries)?;
        run(&plan).await
    }

    async fn drop_databases(&self) {
        for database in &self.databases {
            if let Err(error) = self
                .service
                .disconnect(self.server.config.id, Some(database))
                .await
            {
                eprintln!("could not disconnect {database}: {error:#}");
            }
            let sql = match self.server.db_type {
                DatabaseType::PostgreSQL => {
                    format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)")
                }
                _ => format!("DROP DATABASE IF EXISTS {database}"),
            };
            if let Err(error) = self.admin(&sql).await {
                eprintln!("could not drop {database}: {error:#}");
            }
        }
    }
}

async fn run(plan: &super::command::ToolPlan) -> Result<()> {
    let (events, received) = smol::channel::unbounded();
    let (_stop_sender, stop) = smol::channel::bounded(1);
    let outcome = run_plan(plan, &events, &stop, &Arc::new(AtomicU64::new(0))).await;
    drop(events);
    let output: Vec<String> = std::iter::from_fn(|| received.try_recv().ok())
        .map(|event| format!("{event:?}"))
        .collect();
    match outcome {
        Ok(RunOutcome::Finished) => Ok(()),
        Ok(RunOutcome::Stopped) => Err(anyhow!("stopped")),
        Err(error) => Err(error.context(output.join("\n"))),
    }
}

fn unique_name(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or_default();
    format!("{prefix}_{}_{nanos}", std::process::id())
}

fn tables(schema: Option<&str>) -> Vec<TableChoice> {
    let table = |name: &str| TableChoice::new(schema.map(str::to_string), name.to_string());
    let mut audit = table("audit_log");
    audit.structure_only = true;
    let mut scratch = table("scratch");
    scratch.include = false;
    vec![table("items"), audit, scratch]
}

const SOURCE_SQL: &str = "
CREATE TABLE items (id INT PRIMARY KEY, name VARCHAR(40));
INSERT INTO items VALUES (1, 'one'), (2, 'two'), (3, 'three');
CREATE TABLE audit_log (id INT);
INSERT INTO audit_log VALUES (1), (2);
CREATE TABLE scratch (id INT);
";

async fn assert_restored(fixture: &Fixture, database: &str, schema_filter: &str) -> Result<()> {
    assert_eq!(
        fixture
            .single_value(database, "SELECT COUNT(*) FROM items")
            .await?
            .as_deref(),
        Some("3")
    );
    assert_eq!(
        fixture
            .single_value(database, "SELECT name FROM items WHERE id = 2")
            .await?
            .as_deref(),
        Some("two")
    );
    assert_eq!(
        fixture
            .single_value(database, "SELECT COUNT(*) FROM audit_log")
            .await?
            .as_deref(),
        Some("0"),
        "structure only tables come back empty"
    );
    assert_eq!(
        fixture
            .single_value(
                database,
                &format!(
                    "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'scratch' AND {schema_filter}"
                ),
            )
            .await?
            .as_deref(),
        Some("0"),
        "left out tables are not restored"
    );
    Ok(())
}

#[test]
fn postgres_backup_and_restore_round_trip() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    smol::block_on(async {
        let Some(mut fixture) = Fixture::connect(Server::postgres(), runtime).await? else {
            return Ok(());
        };
        let (Some(pg_dump), Some(pg_restore), Some(psql)) = (
            fixture.locate(super::tools::ToolProgram::PgDump).await,
            fixture.locate(super::tools::ToolProgram::PgRestore).await,
            fixture.locate(super::tools::ToolProgram::Psql).await,
        ) else {
            return Ok(());
        };
        let source = unique_name("blanco_backup_source");
        let custom_target = unique_name("blanco_restore_custom");
        let plain_target = unique_name("blanco_restore_plain");
        let result = async {
            for database in [&source, &custom_target, &plain_target] {
                fixture.create_database(database).await?;
            }
            fixture.execute(&source, SOURCE_SQL).await?;
            let directory = tempfile::tempdir()?;

            let archive = directory.path().join("source.dump");
            fixture
                .back_up(
                    &pg_dump,
                    &source,
                    BackupFormat::Custom,
                    Compression::Default,
                    tables(Some("public")),
                    &archive,
                )
                .await?;
            assert_eq!(detect_dump(&archive)?.format, DumpFormat::Custom);
            fixture
                .restore(&pg_restore, &custom_target, &archive)
                .await?;
            assert_restored(&fixture, &custom_target, "table_schema = 'public'").await?;

            let script = directory.path().join("source.sql.gz");
            fixture
                .back_up(
                    &pg_dump,
                    &source,
                    BackupFormat::Plain,
                    Compression::Fast,
                    tables(Some("public")),
                    &script,
                )
                .await?;
            let detected = detect_dump(&script)?;
            assert_eq!(detected.format, DumpFormat::Plain);
            assert!(detected.gzip);
            fixture.restore(&psql, &plain_target, &script).await?;
            assert_restored(&fixture, &plain_target, "table_schema = 'public'").await
        }
        .await;
        fixture.drop_databases().await;
        result
    })
}

#[test]
fn mysql_backup_and_restore_round_trip() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    smol::block_on(async {
        let Some(mut fixture) = Fixture::connect(Server::mysql(), runtime).await? else {
            return Ok(());
        };
        let (Some(mysqldump), Some(mysql)) = (
            fixture.locate(super::tools::ToolProgram::MysqlDump).await,
            fixture.locate(super::tools::ToolProgram::Mysql).await,
        ) else {
            return Ok(());
        };
        let source = unique_name("blanco_backup_source");
        let target = unique_name("blanco_restore_plain");
        let result = async {
            for database in [&source, &target] {
                fixture.create_database(database).await?;
            }
            fixture.execute(&source, SOURCE_SQL).await?;
            let directory = tempfile::tempdir()?;
            let script = directory.path().join("source.sql.gz");
            fixture
                .back_up(
                    &mysqldump,
                    &source,
                    BackupFormat::Plain,
                    Compression::Default,
                    tables(None),
                    &script,
                )
                .await?;
            assert!(detect_dump(&script)?.gzip);
            fixture.restore(&mysql, &target, &script).await?;
            assert_restored(&fixture, &target, &format!("table_schema = '{target}'")).await
        }
        .await;
        fixture.drop_databases().await;
        result
    })
}
