use futures::channel::mpsc;
use futures::io::{AsyncWriteExt, BufWriter};
use futures::{SinkExt, StreamExt};
use smol::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{results_panel::SelectedTableData, transformers::DataTransformer};
use super::modal::ExportOptions;
use blanco_core::connection_trait::ColumnType;

/// Progress information for export operations
#[derive(Clone, Debug)]
pub struct ExportProgress {
    pub exported_rows: usize,
    pub total_rows: usize,
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
        _options: &ExportOptions,
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
        let (mut sender, receiver) = mpsc::channel::<StreamRowData>(1000);
        let exported_rows = Arc::new(AtomicUsize::new(0));
        let is_cancelled = Arc::new(AtomicUsize::new(0));

        // Get the streaming query results
        let (columns, column_types, mut row_stream) = connection
            .execute_query_stream_rows(query, database_name)
            .await?;

        // Initialize transformer output (headers, etc.)
        let initial_output = transformer
            .initialize_stream(&columns, &column_types)
            .map_err(|e| anyhow::anyhow!("Failed to initialize transformer: {}", e))?;

        // Clone needed data for the background task
        let exported_rows_clone = exported_rows.clone();
        let is_cancelled_clone = is_cancelled.clone();
        let columns_clone = columns.clone();
        let column_types_clone = column_types.clone();

        // Start the producer task in the background
        smol::spawn(async move {
            let mut row_count = 0;

            // Process the stream and send rows through the channel
            while let Some(row_result) = row_stream.next().await {
                // Check for cancellation
                if is_cancelled_clone.load(Ordering::Relaxed) == 1 {
                    tracing::info!("Export cancelled by user");
                    break;
                }

                let row_data = match row_result {
                    Ok(row) => row,
                    Err(e) => {
                        tracing::error!("Error fetching row: {}", e);
                        continue;
                    }
                };

                let stream_data = StreamRowData {
                    columns: columns_clone.clone(),
                    column_types: column_types_clone.clone(),
                    row_data,
                };

                // Send row through channel
                if (sender.send(stream_data).await).is_err() {
                    tracing::info!("Export receiver disconnected, stopping producer");
                    break;
                }

                row_count += 1;
                exported_rows_clone.store(row_count, Ordering::Relaxed);
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

        // Write initial output (headers, etc.)
        if !initial_output.is_empty() {
            writer
                .write_all(initial_output.as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to write initial output: {}", e))?;
            bytes_written += initial_output.len();
        }

        let mut stream = receiver;
        while let Some(stream_data) = StreamExt::next(&mut stream).await {
            // Check for cancellation
            if is_cancelled.load(Ordering::Relaxed) == 1 {
                return Ok(ExportResult::Cancelled);
            }

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
                .write_all(row_output.as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to write transformed row: {}", e))?;

            bytes_written += row_output.len();
            rows_written += 1;

            // Update progress
            let progress = ExportProgress {
                exported_rows: rows_written,
                total_rows: rows_written, // Estimate since we don't know total in advance
                _current_file_size: bytes_written as u64,
                _is_complete: false,
            };

            progress_callback(progress);
        }

        // Finalize transformer output
        let final_output = transformer
            .finalize_stream()
            .map_err(|e| anyhow::anyhow!("Failed to finalize transformer: {}", e))?;

        if !final_output.is_empty() {
            writer
                .write_all(final_output.as_bytes())
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
            total_rows: rows_written,
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
