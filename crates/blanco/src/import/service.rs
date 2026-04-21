use crate::import::mapping::{
    CompiledMapping, ConflictStrategy, MappingSource, build_insert_sql, cell_to_param,
};
use anyhow::{Context, Result};
use blanco_core::Connection;
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
    pub encoding: &'static Encoding,
    pub delimiter: u8,
    pub has_header: bool,
    pub batch_size: usize,
    pub cancel: Arc<AtomicBool>,
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

    let file_handle =
        File::open(file).with_context(|| format!("failed to open {}", file.display()))?;
    let decoder = DecodeReaderBytesBuilder::new()
        .encoding(Some(encoding))
        .bom_sniffing(true)
        .build(BufReader::new(file_handle));
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .flexible(true)
        .from_reader(decoder);

    let column_names: Vec<String> = mappings.iter().map(|m| m.table_column.clone()).collect();
    let data_types: Vec<String> = mappings.iter().map(|m| m.data_type.clone()).collect();

    connection
        .execute_write("BEGIN", database, &[])
        .await
        .context("failed to open import transaction")?;

    let mut rows_inserted: u64 = 0;
    let mut batch: Vec<csv::StringRecord> = Vec::with_capacity(batch_size);

    for record in reader.records() {
        if cancel.load(Ordering::Acquire) {
            rollback(connection, database).await;
            return Err(ImportError::Cancelled);
        }
        let record = match record {
            Ok(r) => r,
            Err(err) => {
                rollback(connection, database).await;
                return Err(ImportError::Failed(anyhow::Error::from(err)));
            }
        };
        batch.push(record);
        if batch.len() >= batch_size {
            match flush_batch(
                connection,
                database,
                fq_table,
                db_type,
                mappings,
                conflict,
                &column_names,
                &data_types,
                &mut batch,
            )
            .await
            {
                Ok(n) => {
                    rows_inserted += n;
                    on_progress(rows_inserted);
                }
                Err(err) => {
                    rollback(connection, database).await;
                    return Err(ImportError::Failed(err));
                }
            }
        }
    }

    if !batch.is_empty() {
        match flush_batch(
            connection,
            database,
            fq_table,
            db_type,
            mappings,
            conflict,
            &column_names,
            &data_types,
            &mut batch,
        )
        .await
        {
            Ok(n) => {
                rows_inserted += n;
                on_progress(rows_inserted);
            }
            Err(err) => {
                rollback(connection, database).await;
                return Err(ImportError::Failed(err));
            }
        }
    }

    connection
        .execute_write("COMMIT", database, &[])
        .await
        .context("failed to commit import transaction")?;

    Ok(ImportReport {
        rows_inserted,
        duration_ms: started.elapsed().as_millis(),
    })
}

async fn flush_batch(
    connection: &dyn Connection,
    database: Option<&str>,
    fq_table: &str,
    db_type: DatabaseType,
    mappings: &[CompiledMapping],
    conflict: &ConflictStrategy,
    column_names: &[String],
    data_types: &[String],
    batch: &mut Vec<csv::StringRecord>,
) -> Result<u64> {
    if batch.is_empty() {
        return Ok(0);
    }
    let rows = std::mem::take(batch);
    let sql = build_insert_sql(fq_table, column_names, data_types, rows.len(), db_type, conflict);
    let mut params: Vec<Option<String>> = Vec::with_capacity(rows.len() * mappings.len());
    for row in &rows {
        for mapping in mappings {
            let value = match &mapping.source {
                MappingSource::Fixed(v) => cell_to_param(Some(v.as_str()), mapping.is_nullable, mapping.transform),
                MappingSource::CsvColumn(idx) => cell_to_param(row.get(*idx), mapping.is_nullable, mapping.transform),
            };
            params.push(value);
        }
    }
    connection.execute_write(&sql, database, &params).await?;
    Ok(rows.len() as u64)
}

async fn rollback(connection: &dyn Connection, database: Option<&str>) {
    if let Err(err) = connection.execute_write("ROLLBACK", database, &[]).await {
        tracing::error!("failed to rollback import transaction: {}", err);
    }
}
