use crate::import::detect::{self, ImportFormat};
use crate::import::mapping::{
    CompiledMapping, ConflictStrategy, MappingSource, build_insert_sql, cell_to_param,
};
use anyhow::{Context, Result};
use blanco_core::{Connection, WriteOperation};
use database::DatabaseType;
use encoding_rs::Encoding;
use encoding_rs_io::DecodeReaderBytesBuilder;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_BATCH_SIZE: usize = 500;

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub rows_inserted: u64,
    pub duration_ms: u128,
}

#[derive(Debug)]
pub enum ImportError {
    Cancelled,
    Failed(anyhow::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Cancelled => write!(f, "import cancelled"),
            ImportError::Failed(e) => write!(f, "{}", e),
        }
    }
}

impl From<anyhow::Error> for ImportError {
    fn from(err: anyhow::Error) -> Self {
        ImportError::Failed(err)
    }
}

pub struct ImportRequest<'a> {
    pub connection: &'a dyn Connection,
    pub database: Option<&'a str>,
    pub fq_table: &'a str,
    pub db_type: DatabaseType,
    pub mappings: &'a [CompiledMapping],
    pub conflict: &'a ConflictStrategy,
    pub file: &'a Path,
    pub format: ImportFormat,
    /// Source column names in the order the mapping indexes refer to (the
    /// detected headers). JSON rows are laid out in this order.
    pub source_headers: &'a [String],
    pub encoding: &'static Encoding,
    pub delimiter: u8,
    pub has_header: bool,
    pub batch_size: usize,
    pub cancel: Arc<AtomicBool>,
}

/// One imported record: cells in source column order, `None` for missing or
/// null values. Produced by [`RowSource`] so the insert path does not care
/// whether the file was CSV or JSON.
type SourceRow = Vec<Option<String>>;

enum RowSource {
    Csv(Box<csv::Reader<encoding_rs_io::DecodeReaderBytes<BufReader<File>, Vec<u8>>>>),
    /// JSON is parsed up front: an array needs the whole document anyway, and
    /// the union of keys (the column order) is only known once every object
    /// has been seen.
    Json(std::vec::IntoIter<SourceRow>),
}

impl RowSource {
    fn open(
        file: &Path,
        format: ImportFormat,
        encoding: &'static Encoding,
        delimiter: u8,
        has_header: bool,
        headers: &[String],
    ) -> Result<Self> {
        let file_handle =
            File::open(file).with_context(|| format!("failed to open {}", file.display()))?;
        let decoder = DecodeReaderBytesBuilder::new()
            .encoding(Some(encoding))
            .bom_sniffing(true)
            .build(BufReader::new(file_handle));
        match format {
            ImportFormat::Csv => Ok(Self::Csv(Box::new(
                csv::ReaderBuilder::new()
                    .delimiter(delimiter)
                    .has_headers(has_header)
                    .flexible(true)
                    .from_reader(decoder),
            ))),
            ImportFormat::Json => {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut { decoder }, &mut text)
                    .with_context(|| format!("failed to read {}", file.display()))?;
                let objects = detect::parse_json_objects(&text, None)?;
                let rows: Vec<SourceRow> = objects
                    .iter()
                    .map(|object| {
                        headers
                            .iter()
                            .map(|header| detect::json_cell(object.get(header)))
                            .collect()
                    })
                    .collect();
                Ok(Self::Json(rows.into_iter()))
            }
        }
    }

    fn next_row(&mut self) -> Option<Result<SourceRow>> {
        match self {
            Self::Csv(reader) => reader.records().next().map(|record| {
                record
                    .map(|record| record.iter().map(|cell| Some(cell.to_string())).collect())
                    .map_err(anyhow::Error::from)
            }),
            Self::Json(rows) => rows.next().map(Ok),
        }
    }
}

pub async fn run_import<F>(
    request: ImportRequest<'_>,
    on_progress: F,
) -> Result<ImportReport, ImportError>
where
    F: Fn(u64) + Send,
{
    let ImportRequest {
        connection,
        database,
        fq_table,
        db_type,
        mappings,
        conflict,
        file,
        format,
        source_headers,
        encoding,
        delimiter,
        has_header,
        batch_size,
        cancel,
    } = request;

    if mappings.is_empty() {
        return Err(ImportError::Failed(anyhow::anyhow!(
            "no columns mapped for import"
        )));
    }
    let batch_size = batch_size.max(1);

    let started = std::time::Instant::now();

    let column_names: Vec<String> = mappings.iter().map(|m| m.table_column.clone()).collect();
    let data_types: Vec<String> = mappings.iter().map(|m| m.data_type.clone()).collect();
    let mut reader = RowSource::open(
        file,
        format,
        encoding,
        delimiter,
        has_header,
        source_headers,
    )?;

    // Every batch becomes one multi-row INSERT; the whole import is then sent
    // as a single transactional batch so a failure anywhere leaves the table
    // untouched. Manual BEGIN/COMMIT through `execute_write` would not do
    // that: pooled connections may hand each statement to a different socket.
    let mut operations: Vec<WriteOperation> = Vec::new();
    let mut rows_seen: u64 = 0;
    let mut batch: Vec<SourceRow> = Vec::with_capacity(batch_size);

    while let Some(record) = reader.next_row() {
        if cancel.load(Ordering::Acquire) {
            return Err(ImportError::Cancelled);
        }
        let record = record.map_err(ImportError::Failed)?;
        batch.push(record);
        if batch.len() >= batch_size {
            rows_seen += batch.len() as u64;
            operations.push(batch_operation(
                fq_table,
                db_type,
                mappings,
                conflict,
                &column_names,
                &data_types,
                &mut batch,
            ));
            on_progress(rows_seen);
        }
    }
    if !batch.is_empty() {
        rows_seen += batch.len() as u64;
        operations.push(batch_operation(
            fq_table,
            db_type,
            mappings,
            conflict,
            &column_names,
            &data_types,
            &mut batch,
        ));
        on_progress(rows_seen);
    }

    if cancel.load(Ordering::Acquire) {
        return Err(ImportError::Cancelled);
    }
    connection
        .execute_operations_transactional(&operations, database)
        .await
        .map_err(|failure| {
            ImportError::Failed(failure.error.context(format!(
                "import failed, {} of {} batches were applied",
                failure.applied,
                operations.len()
            )))
        })?;

    Ok(ImportReport {
        rows_inserted: rows_seen,
        duration_ms: started.elapsed().as_millis(),
    })
}

fn batch_operation(
    fq_table: &str,
    db_type: DatabaseType,
    mappings: &[CompiledMapping],
    conflict: &ConflictStrategy,
    column_names: &[String],
    data_types: &[String],
    batch: &mut Vec<SourceRow>,
) -> WriteOperation {
    let rows = std::mem::take(batch);
    let sql = build_insert_sql(
        fq_table,
        column_names,
        data_types,
        rows.len(),
        db_type,
        conflict,
    );
    let mut params: Vec<Option<String>> = Vec::with_capacity(rows.len() * mappings.len());
    for row in &rows {
        for mapping in mappings {
            let value = match &mapping.source {
                MappingSource::Fixed(v) => {
                    cell_to_param(Some(v.as_str()), mapping.is_nullable, mapping.transform)
                }
                MappingSource::CsvColumn(idx) => cell_to_param(
                    row.get(*idx).and_then(|cell| cell.as_deref()),
                    mapping.is_nullable,
                    mapping.transform,
                ),
            };
            params.push(value);
        }
    }
    WriteOperation::new(sql, params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::mapping::{ColumnMapping, Transform, compile_mappings};
    use blanco_core::ColumnInfo;
    use std::sync::atomic::AtomicBool;

    async fn sqlite_with_people() -> (sqlite::SqliteConnection, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection_string = format!("sqlite:{}", dir.path().join("import.db").display());
        let mut connection =
            sqlite::SqliteConnection::new(connection_string.clone()).expect("sqlite");
        connection
            .connect(&connection_string)
            .await
            .expect("connect");
        connection
            .execute_write(
                "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT)",
                None,
                &[],
            )
            .await
            .expect("create table");
        (connection, dir)
    }

    fn people_columns() -> Vec<ColumnInfo> {
        ["id", "name", "email"]
            .into_iter()
            .map(|name| ColumnInfo {
                name: name.to_string(),
                data_type: if name == "id" { "INTEGER" } else { "TEXT" }.to_string(),
                is_nullable: name == "email",
                is_primary_key: name == "id",
                default_value: None,
                character_maximum_length: None,
                foreign_key: None,
            })
            .collect()
    }

    async fn import_file(
        connection: &sqlite::SqliteConnection,
        file: &Path,
        format: ImportFormat,
        source_headers: &[String],
    ) -> ImportReport {
        let columns = people_columns();
        let mappings: Vec<ColumnMapping> = columns
            .iter()
            .map(|column| {
                let index = source_headers
                    .iter()
                    .position(|header| header == &column.name)
                    .expect("header present");
                ColumnMapping::CsvColumn(index)
            })
            .collect();
        let transforms = vec![Transform::None; columns.len()];
        let compiled = compile_mappings(&columns, &mappings, &transforms);
        let request = ImportRequest {
            connection,
            database: None,
            fq_table: "\"people\"",
            db_type: DatabaseType::SQLite,
            mappings: &compiled,
            conflict: &ConflictStrategy::Fail,
            file,
            format,
            source_headers,
            encoding: encoding_rs::UTF_8,
            delimiter: b',',
            has_header: true,
            batch_size: 2,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        run_import(request, |_| {}).await.expect("import")
    }

    async fn names(connection: &sqlite::SqliteConnection) -> Vec<(String, Option<String>)> {
        let result = connection
            .execute_query("SELECT name, email FROM people ORDER BY id", None, None)
            .await
            .expect("select");
        result
            .rows
            .iter()
            .map(|row| (row[0].clone().unwrap_or_default(), row[1].clone()))
            .collect()
    }

    #[tokio::test]
    async fn imports_csv_rows() {
        let (connection, dir) = sqlite_with_people().await;
        let file = dir.path().join("people.csv");
        std::fs::write(
            &file,
            "id,name,email\n1,Ada,ada@example.com\n2,Bob,\n3,Cy,cy@x\n",
        )
        .expect("write");
        let headers: Vec<String> = ["id", "name", "email"].map(String::from).to_vec();
        let report = import_file(&connection, &file, ImportFormat::Csv, &headers).await;
        assert_eq!(report.rows_inserted, 3);
        let rows = names(&connection).await;
        assert_eq!(rows[0], ("Ada".into(), Some("ada@example.com".into())));
        assert_eq!(
            rows[1],
            ("Bob".into(), None),
            "empty nullable cell becomes NULL"
        );
    }

    #[tokio::test]
    async fn imports_json_array_with_sparse_keys() {
        let (connection, dir) = sqlite_with_people().await;
        let file = dir.path().join("people.json");
        std::fs::write(
            &file,
            r#"[{"id": 1, "name": "Ada", "email": "ada@example.com"}, {"id": 2, "name": "Bob"}, {"id": 3, "name": "Cy", "email": null}]"#,
        )
        .expect("write");
        // Headers as the sample detection would order them.
        let headers: Vec<String> = ["id", "name", "email"].map(String::from).to_vec();
        let report = import_file(&connection, &file, ImportFormat::Json, &headers).await;
        assert_eq!(report.rows_inserted, 3);
        let rows = names(&connection).await;
        assert_eq!(rows[0].1.as_deref(), Some("ada@example.com"));
        assert_eq!(rows[1], ("Bob".into(), None), "missing key imports as NULL");
        assert_eq!(
            rows[2],
            ("Cy".into(), None),
            "explicit null imports as NULL"
        );
    }

    #[tokio::test]
    async fn imports_ndjson() {
        let (connection, dir) = sqlite_with_people().await;
        let file = dir.path().join("people.ndjson");
        std::fs::write(
            &file,
            "{\"id\":1,\"name\":\"Ada\"}\n{\"id\":2,\"name\":\"Bob\"}\n",
        )
        .expect("write");
        let headers: Vec<String> = ["id", "name", "email"].map(String::from).to_vec();
        let report = import_file(&connection, &file, ImportFormat::Json, &headers).await;
        assert_eq!(report.rows_inserted, 2);
    }
}
