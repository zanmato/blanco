use futures::channel::mpsc;
use futures::io::{AsyncWriteExt, BufWriter};
use futures::{SinkExt, StreamExt};
use smol::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::modal::ExportOptions;
use blanco_core::ColumnType;
use transformers::{DataTransformer, SelectedTableData};

/// Progress information for export operations
#[derive(Clone, Debug)]
pub struct ExportProgress {
    pub exported_rows: usize,
    pub _current_file_size: u64,
    pub _is_complete: bool,
}

/// Result of an export operation
#[derive(Clone, Debug)]
pub enum ExportResult {
    Success {
        file_path: String,
        rows_exported: usize,
        _file_size: u64,
    },
    Cancelled,
}

/// Row data sent through the streaming channel
#[derive(Clone, Debug)]
pub struct StreamRowData {
    pub columns: Vec<String>,
    pub column_types: Vec<ColumnType>,
    pub row_data: Vec<Option<String>>,
}

/// Service for handling data export operations
pub struct ExportService;

impl ExportService {
    pub fn new() -> Self {
        Self
    }

    /// Export data using streaming with transformers that support streaming
    #[allow(clippy::too_many_arguments)]
    pub async fn export_data_streaming_with_transformer<C>(
        &self,
        connection: &C,
        query: &str,
        database_name: Option<&str>,
        transformer: &dyn DataTransformer,
        file_path: &Path,
        options: &ExportOptions,
        mut progress_callback: impl FnMut(ExportProgress) + Send + Sync + 'static,
    ) -> Result<ExportResult, anyhow::Error>
    where
        C: blanco_core::Connection + Send + Sync + 'static + ?Sized,
    {
        // Check if transformer supports streaming
        if !transformer.supports_streaming() {
            return Err(anyhow::anyhow!(
                "Transformer '{}' does not support streaming",
                transformer.format_name()
            ));
        }

        // Create channels for producer-consumer pattern
        let (mut sender, receiver) = mpsc::channel::<anyhow::Result<StreamRowData>>(1000);
        let is_cancelled = Arc::clone(&options.cancel);

        // Get the streaming query results
        let (columns, column_types, mut row_stream) = connection
            .execute_query_stream_rows(query, database_name)
            .await?;

        // Initialize transformer output (headers, etc.)
        let initial_output = transformer
            .initialize_stream(&columns, &column_types)
            .map_err(|e| anyhow::anyhow!("Failed to initialize transformer: {}", e))?;

        // Clone needed data for the background task
        let is_cancelled_clone = is_cancelled.clone();
        let columns_clone = columns.clone();
        let column_types_clone = column_types.clone();

        // Start the producer task in the background
        smol::spawn(async move {
            // Process the stream and send rows through the channel
            while let Some(row_result) = row_stream.next().await {
                // Check for cancellation
                if is_cancelled_clone.load(Ordering::Relaxed) {
                    tracing::info!("Export cancelled by user");
                    break;
                }

                // A row that fails to arrive ends the export with its error.
                // Skipping it would write a file that looks complete.
                let failed = row_result.is_err();
                let stream_data = row_result
                    .map(|row_data| StreamRowData {
                        columns: columns_clone.clone(),
                        column_types: column_types_clone.clone(),
                        row_data,
                    })
                    .map_err(|error| anyhow::anyhow!("Failed to read a row: {error}"));

                // Send row through channel
                if (sender.send(stream_data).await).is_err() {
                    tracing::info!("Export receiver disconnected, stopping producer");
                    break;
                }
                if failed {
                    break;
                }
            }

            // Send completion signal
            drop(sender);
        })
        .detach();

        // Consumer task: write transformed rows to file
        let file = File::create(file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create file: {}", e))?;

        let mut writer = BufWriter::new(file);
        let mut rows_written = 0;
        let mut bytes_written = 0;
        let encoding = options.encoding;

        writer
            .write_all(encoding.preamble())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write byte order mark: {}", e))?;

        // Write initial output (headers, etc.)
        if !initial_output.is_empty() {
            writer
                .write_all(&encoding.encode(&initial_output))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to write initial output: {}", e))?;
            bytes_written += initial_output.len();
        }

        let mut stream = receiver;
        while let Some(stream_data) = StreamExt::next(&mut stream).await {
            // Check for cancellation
            if is_cancelled.load(Ordering::Relaxed) {
                return Ok(ExportResult::Cancelled);
            }
            let stream_data = stream_data?;

            // Transform row using transformer
            let row_output = transformer
                .transform_stream_row(
                    &stream_data.row_data,
                    &stream_data.columns,
                    &stream_data.column_types,
                )
                .map_err(|e| anyhow::anyhow!("Failed to transform row: {}", e))?;

            // Write transformed row
            writer
                .write_all(&encoding.encode(&row_output))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to write transformed row: {}", e))?;

            bytes_written += row_output.len();
            rows_written += 1;

            // Update progress
            let progress = ExportProgress {
                exported_rows: rows_written,
                _current_file_size: bytes_written as u64,
                _is_complete: false,
            };

            progress_callback(progress);
        }

        // The producer stops sending when it sees the flag, which ends the
        // loop above without a row to notice it on.
        if is_cancelled.load(Ordering::Relaxed) {
            return Ok(ExportResult::Cancelled);
        }

        // Finalize transformer output
        let final_output = transformer
            .finalize_stream()
            .map_err(|e| anyhow::anyhow!("Failed to finalize transformer: {}", e))?;

        if !final_output.is_empty() {
            writer
                .write_all(&encoding.encode(&final_output))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to write final output: {}", e))?;
        }

        // Flush the writer
        writer
            .flush()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to flush file: {}", e))?;

        // Get final file size
        let file_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);

        // Send final progress update
        let final_progress = ExportProgress {
            exported_rows: rows_written,
            _current_file_size: file_size,
            _is_complete: true,
        };

        progress_callback(final_progress);

        Ok(ExportResult::Success {
            file_path: file_path.to_string_lossy().to_string(),
            rows_exported: rows_written,
            _file_size: file_size,
        })
    }

    /// Export selected table data to a file
    /// This is used for exporting selected rows from the results panel
    pub async fn export_selected_data(
        &self,
        data: &SelectedTableData,
        transformer: &dyn DataTransformer,
        file_path: &Path,
    ) -> Result<ExportResult, anyhow::Error> {
        // Transform the data
        let transformed = transformer
            .transform_selected_data(data)
            .map_err(|e| anyhow::anyhow!("Failed to transform data: {}", e))?;

        // Write to file
        let file = File::create(file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create file: {}", e))?;

        let mut writer = BufWriter::new(file);

        writer
            .write_all(transformed.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write to file: {}", e))?;

        writer
            .flush()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to flush file: {}", e))?;

        // Get file size
        let file_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);

        let rows_exported = data.selected_rows.len();

        Ok(ExportResult::Success {
            file_path: file_path.to_string_lossy().to_string(),
            rows_exported,
            _file_size: file_size,
        })
    }
}

impl Default for ExportService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::Connection as _;
    use transformers::{CsvOptions, CsvTransformer, TextEncoding};

    #[tokio::test]
    async fn streamed_export_is_written_in_the_chosen_encoding() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection_string = format!("sqlite:{}", dir.path().join("export.db").display());
        let mut connection =
            sqlite::SqliteConnection::new(connection_string.clone()).expect("sqlite");
        connection
            .connect(&connection_string)
            .await
            .expect("connect");

        let path = dir.path().join("out.csv");
        let options = ExportOptions {
            encoding: TextEncoding::Utf16Le,
            ..Default::default()
        };
        ExportService::new()
            .export_data_streaming_with_transformer(
                &connection,
                "SELECT 'å' AS name",
                None,
                &CsvTransformer::new(CsvOptions::default()),
                &path,
                &options,
                |_| {},
            )
            .await
            .expect("export");

        let bytes = std::fs::read(&path).expect("read export");
        let mut expected = vec![0xFF, 0xFE];
        expected.extend(TextEncoding::Utf16Le.encode("name\nå\n").iter());
        assert_eq!(bytes, expected);
    }

    async fn sqlite_connection(dir: &std::path::Path) -> sqlite::SqliteConnection {
        let connection_string = format!("sqlite:{}", dir.join("export.db").display());
        let mut connection =
            sqlite::SqliteConnection::new(connection_string.clone()).expect("sqlite");
        connection
            .connect(&connection_string)
            .await
            .expect("connect");
        connection
    }

    /// A table whose name needs quoting, holding values with a tab, a line
    /// break and a quote, comes back out of a TSV export unchanged.
    #[tokio::test]
    async fn tsv_export_keeps_tabs_line_breaks_and_quotes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection = sqlite_connection(dir.path()).await;
        for statement in [
            "CREATE TABLE \"Mixed Case\" (id INTEGER, note TEXT)",
            "INSERT INTO \"Mixed Case\" VALUES (1, 'a' || char(9) || 'b'), \
             (2, 'two' || char(10) || 'lines'), (3, 'say \"hi\"'), (4, NULL)",
        ] {
            connection
                .execute_query(statement, None, None)
                .await
                .expect("setup");
        }
        let query = format!(
            "SELECT * FROM {} ORDER BY id",
            blanco_core::DatabaseType::SQLite
                .dialect()
                .quote_qualified(None, "Mixed Case")
        );

        let path = dir.path().join("out.tsv");
        let result = ExportService::new()
            .export_data_streaming_with_transformer(
                &connection,
                &query,
                None,
                &transformers::TsvTransformer,
                &path,
                &ExportOptions::default(),
                |_| {},
            )
            .await
            .expect("export");
        assert!(matches!(
            result,
            ExportResult::Success {
                rows_exported: 4,
                ..
            }
        ));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read export"),
            "id\tnote\n1\t\"a\tb\"\n2\t\"two\nlines\"\n3\t\"say \"\"hi\"\"\"\n4\t\n"
        );
    }

    #[tokio::test]
    async fn a_cancelled_export_stops_and_says_so() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection = sqlite_connection(dir.path()).await;
        let options = ExportOptions::default();
        options.cancel.store(true, Ordering::Relaxed);

        let result = ExportService::new()
            .export_data_streaming_with_transformer(
                &connection,
                "SELECT 1 AS n UNION ALL SELECT 2",
                None,
                &CsvTransformer::new(CsvOptions::default()),
                &dir.path().join("out.csv"),
                &options,
                |_| {},
            )
            .await
            .expect("export");
        assert!(matches!(result, ExportResult::Cancelled));
    }
}
