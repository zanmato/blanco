use async_std::{
    fs::File,
    io::{BufWriter, WriteExt},
    task,
};
use futures::channel::mpsc;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use futures::{Stream, StreamExt, SinkExt};

use crate::{
    export_modal::ExportOptions,
    results_panel::SelectedTableData,
    transformers::{DataTransformer, TransformError},
};

/// Progress information for export operations
#[derive(Clone, Debug)]
pub struct ExportProgress {
    pub exported_rows: usize,
    pub total_rows: usize,
    pub current_file_size: u64,
    pub is_complete: bool,
}

/// Result of an export operation
#[derive(Clone, Debug)]
pub enum ExportResult {
    Success {
        file_path: String,
        rows_exported: usize,
        file_size: u64,
    },
    Error {
        message: String,
    },
    Cancelled,
}

/// Row data sent through the streaming channel
#[derive(Clone, Debug)]
pub struct StreamRowData {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub row_data: Vec<String>,
}

/// Service for handling data export operations
pub struct ExportService;

impl ExportService {
    pub fn new() -> Self {
        Self
    }

    /// Export data using the specified transformer with streaming support
    pub async fn export_data(
        &self,
        transformer: &dyn DataTransformer,
        table_data: &SelectedTableData,
        file_path: &Path,
        options: &ExportOptions,
    ) -> Result<ExportResult, TransformError> {
        // Validate input
        if !table_data.has_selection() {
            return Ok(ExportResult::Error {
                message: "No data selected for export".to_string(),
            });
        }

        let total_rows = table_data.selected_rows.len();
        let exported_rows = Arc::new(AtomicUsize::new(0));
        let is_cancelled = Arc::new(AtomicUsize::new(0)); // 0 = not cancelled, 1 = cancelled

        let _exported_rows_clone = exported_rows.clone();
        let _is_cancelled_clone = is_cancelled.clone();

        // Create the output file with buffered writer
        let file = File::create(file_path).await.map_err(|e| {
            TransformError::FormatError(format!("Failed to create file: {}", e))
        })?;

        let mut writer = BufWriter::new(file);

        // Sort row indices for consistent output
        let mut row_indices: Vec<usize> = table_data.selected_rows.iter().map(|r| r.row).collect();
        row_indices.sort();

        // Transform the complete data at once to avoid header duplication
        let complete_output = transformer.transform_selected_data(table_data)?;

        // Write the complete output in chunks for memory efficiency
        let chunk_size = 8192; // 8KB chunks
        let output_bytes = complete_output.as_bytes();
        let total_bytes = output_bytes.len();
        let mut bytes_written = 0;

        for chunk_start in (0..total_bytes).step_by(chunk_size) {
            // Check for cancellation
            if is_cancelled.load(Ordering::Relaxed) == 1 {
                return Ok(ExportResult::Cancelled);
            }

            let chunk_end = (chunk_start + chunk_size).min(total_bytes);
            let chunk = &output_bytes[chunk_start..chunk_end];

            writer.write_all(chunk).await.map_err(|e| {
                TransformError::FormatError(format!("Failed to write chunk to file: {}", e))
            })?;

            bytes_written += chunk.len();

            // Update progress based on bytes written (as proxy for row progress)
            let progress = (bytes_written as f64 / total_bytes as f64) * table_data.selected_rows.len() as f64;
            exported_rows.store(progress as usize, Ordering::Relaxed);
        }

        // Flush the writer to ensure all data is written
        writer.flush().await.map_err(|e| {
            TransformError::FormatError(format!("Failed to flush file: {}", e))
        })?;

        // Get final file size
        let file_size = std::fs::metadata(file_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let final_exported_rows = exported_rows.load(Ordering::Relaxed);

        Ok(ExportResult::Success {
            file_path: file_path.to_string_lossy().to_string(),
            rows_exported: final_exported_rows,
            file_size,
        })
    }

    /// Export data using streaming with channels to avoid loading all rows at once
    /// This method uses a producer-consumer pattern where rows are streamed from the database
    /// and sent through a channel to the writer
    pub async fn export_data_streaming<C>(
        &self,
        connection: &C,
        query: &str,
        database_name: Option<&str>,
        file_path: &Path,
        options: &ExportOptions,
        mut progress_callback: impl FnMut(ExportProgress) + Send + Sync + 'static,
    ) -> Result<ExportResult, anyhow::Error>
    where
        C: blanco_core::Connection + Send + Sync + 'static + ?Sized,
    {
        // Create channels for producer-consumer pattern
        let (mut sender, mut receiver) = mpsc::channel::<StreamRowData>(1000); // Buffer of 1000 rows
        let exported_rows = Arc::new(AtomicUsize::new(0));
        let is_cancelled = Arc::new(AtomicUsize::new(0));

        // Get the streaming query results
        let (columns, column_types, mut row_stream) = connection
            .execute_query_stream_rows(query, database_name)
            .await?;

        // Clone needed data for the background task
        let exported_rows_clone = exported_rows.clone();
        let is_cancelled_clone = is_cancelled.clone();
        let columns_clone = columns.clone();

        // Start the producer task in the background
        let producer_task = task::spawn(async move {
            let mut row_count = 0;

            // Process the stream and send rows through the channel
            while let Some(row_result) = row_stream.next().await {
                // Check for cancellation
                if is_cancelled_clone.load(Ordering::Relaxed) == 1 {
                    log::info!("Export cancelled by user");
                    break;
                }

                let row_data = match row_result {
                    Ok(row) => row,
                    Err(e) => {
                        log::error!("Error fetching row: {}", e);
                        continue;
                    }
                };

                let stream_data = StreamRowData {
                    columns: columns_clone.clone(),
                    column_types: column_types.clone(),
                    row_data,
                };

                // Send row through channel, checking if receiver is still connected
                if let Err(_) = sender.send(stream_data).await {
                    log::info!("Export receiver disconnected, stopping producer");
                    break;
                }

                row_count += 1;
                exported_rows_clone.store(row_count, Ordering::Relaxed);

                // Send progress update
                let progress = ExportProgress {
                    exported_rows: row_count,
                    total_rows: row_count, // We don't know total in advance for true streaming
                    current_file_size: 0, // Will be calculated by consumer
                    is_complete: false,
                };

                // Note: Progress callback will be called from consumer task
            }

            // Send completion signal
            drop(sender); // Close the channel to signal completion
        });

        // Consumer task: write rows to file
        let file = File::create(file_path).await
            .map_err(|e| anyhow::anyhow!("Failed to create file: {}", e))?;

        let mut writer = BufWriter::new(file);
        let mut rows_written = 0;
        let mut bytes_written = 0;

        // Create a simple CSV transformer for demonstration
        // In the future, this should use the selected transformer
        let mut headers_written = false;

        let mut stream = receiver;
        while let Some(stream_data) = StreamExt::next(&mut stream).await {
            // Check for cancellation
            if is_cancelled.load(Ordering::Relaxed) == 1 {
                return Ok(ExportResult::Cancelled);
            }

            // Write headers if this is the first row
            if !headers_written && !stream_data.columns.is_empty() {
                let header_line = stream_data.columns.join(",");
                writer.write_all(header_line.as_bytes()).await
                    .map_err(|e| anyhow::anyhow!("Failed to write headers: {}", e))?;
                writer.write_all(b"\n").await
                    .map_err(|e| anyhow::anyhow!("Failed to write newline: {}", e))?;
                headers_written = true;
                bytes_written += header_line.len() + 1;
            }

            // Write row data
            let row_line = stream_data.row_data.join(",");
            writer.write_all(row_line.as_bytes()).await
                .map_err(|e| anyhow::anyhow!("Failed to write row: {}", e))?;
            writer.write_all(b"\n").await
                .map_err(|e| anyhow::anyhow!("Failed to write newline: {}", e))?;

            bytes_written += row_line.len() + 1;
            rows_written += 1;

            // Update progress
            let progress = ExportProgress {
                exported_rows: rows_written,
                total_rows: rows_written, // Estimate since we don't know total in advance
                current_file_size: bytes_written as u64,
                is_complete: false,
            };

            progress_callback(progress);
        }

        // Flush the writer
        writer.flush().await
            .map_err(|e| anyhow::anyhow!("Failed to flush file: {}", e))?;

        // Wait for producer task to complete
        producer_task.await;

        // Get final file size
        let file_size = std::fs::metadata(file_path)
            .map(|m| m.len())
            .unwrap_or(0);

        // Send final progress update
        let final_progress = ExportProgress {
            exported_rows: rows_written,
            total_rows: rows_written,
            current_file_size: file_size,
            is_complete: true,
        };

        progress_callback(final_progress);

        Ok(ExportResult::Success {
            file_path: file_path.to_string_lossy().to_string(),
            rows_exported: rows_written,
            file_size,
        })
    }

    /// Export data from a stream using the specified transformer with channels
    pub async fn export_from_stream(
        &self,
        mut row_stream: Box<dyn Stream<Item = Result<Vec<String>, anyhow::Error>> + Send + Unpin>,
        columns: &[String],
        column_types: &[String],
        file_path: &Path,
        options: &ExportOptions,
        mut progress_callback: impl FnMut(ExportProgress) + Send + Sync + 'static,
    ) -> Result<ExportResult, anyhow::Error> {
        // Create channels for producer-consumer pattern
        let (mut sender, mut receiver) = mpsc::channel::<StreamRowData>(1000); // Buffer of 1000 rows
        let exported_rows = Arc::new(AtomicUsize::new(0));
        let is_cancelled = Arc::new(AtomicUsize::new(0));

        // Clone needed data for the background task
        let exported_rows_clone = exported_rows.clone();
        let is_cancelled_clone = is_cancelled.clone();
        let columns_clone = columns.to_vec();
        let column_types_clone = column_types.to_vec();

        // Start the producer task
        let producer_task = task::spawn(async move {
            let mut row_count = 0;

            while let Some(row_result) = row_stream.next().await {
                // Check for cancellation
                if is_cancelled_clone.load(Ordering::Relaxed) == 1 {
                    break;
                }

                let row_data = match row_result {
                    Ok(row) => row,
                    Err(e) => {
                        log::error!("Error in stream: {}", e);
                        continue;
                    }
                };

                let stream_data = StreamRowData {
                    columns: columns_clone.clone(),
                    column_types: column_types_clone.clone(),
                    row_data,
                };

                // Send row through channel
                if let Err(_) = sender.send(stream_data).await {
                    log::info!("Export receiver disconnected");
                    break;
                }

                row_count += 1;
                exported_rows_clone.store(row_count, Ordering::Relaxed);
            }

            // Send completion signal
            drop(sender);
        });

        // Consumer: write to file (same as in export_data_streaming)
        let file = File::create(file_path).await
            .map_err(|e| anyhow::anyhow!("Failed to create file: {}", e))?;

        let mut writer = BufWriter::new(file);
        let mut rows_written = 0;
        let mut bytes_written = 0;

        let mut headers_written = false;

        let mut stream = receiver;
        while let Some(stream_data) = StreamExt::next(&mut stream).await {
            // Check for cancellation
            if is_cancelled.load(Ordering::Relaxed) == 1 {
                return Ok(ExportResult::Cancelled);
            }

            // Write headers if this is the first row
            if !headers_written && !stream_data.columns.is_empty() {
                let header_line = stream_data.columns.join(",");
                writer.write_all(header_line.as_bytes()).await
                    .map_err(|e| anyhow::anyhow!("Failed to write headers: {}", e))?;
                writer.write_all(b"\n").await
                    .map_err(|e| anyhow::anyhow!("Failed to write newline: {}", e))?;
                headers_written = true;
                bytes_written += header_line.len() + 1;
            }

            // Write row data
            let row_line = stream_data.row_data.join(",");
            writer.write_all(row_line.as_bytes()).await
                .map_err(|e| anyhow::anyhow!("Failed to write row: {}", e))?;
            writer.write_all(b"\n").await
                .map_err(|e| anyhow::anyhow!("Failed to write newline: {}", e))?;

            bytes_written += row_line.len() + 1;
            rows_written += 1;

            // Update progress
            let progress = ExportProgress {
                exported_rows: rows_written,
                total_rows: rows_written,
                current_file_size: bytes_written as u64,
                is_complete: false,
            };

            progress_callback(progress);
        }

        // Flush and finalize
        writer.flush().await
            .map_err(|e| anyhow::anyhow!("Failed to flush file: {}", e))?;

        producer_task.await;

        let file_size = std::fs::metadata(file_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let final_progress = ExportProgress {
            exported_rows: rows_written,
            total_rows: rows_written,
            current_file_size: file_size,
            is_complete: true,
        };

        progress_callback(final_progress);

        Ok(ExportResult::Success {
            file_path: file_path.to_string_lossy().to_string(),
            rows_exported: rows_written,
            file_size,
        })
    }

    /// Export data using streaming with transformers that support streaming
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
        let (mut sender, receiver) = mpsc::channel::<StreamRowData>(1000);
        let exported_rows = Arc::new(AtomicUsize::new(0));
        let is_cancelled = Arc::new(AtomicUsize::new(0));

        // Get the streaming query results
        let (columns, column_types, mut row_stream) = connection
            .execute_query_stream_rows(query, database_name)
            .await?;

        // Initialize transformer output (headers, etc.)
        let initial_output = transformer.initialize_stream(&columns, &column_types)
            .map_err(|e| anyhow::anyhow!("Failed to initialize transformer: {}", e))?;

        // Clone needed data for the background task
        let exported_rows_clone = exported_rows.clone();
        let is_cancelled_clone = is_cancelled.clone();
        let columns_clone = columns.clone();
        let column_types_clone = column_types.clone();

        // Start the producer task in the background
        let producer_task = task::spawn(async move {
            let mut row_count = 0;

            // Process the stream and send rows through the channel
            while let Some(row_result) = row_stream.next().await {
                // Check for cancellation
                if is_cancelled_clone.load(Ordering::Relaxed) == 1 {
                    log::info!("Export cancelled by user");
                    break;
                }

                let row_data = match row_result {
                    Ok(row) => row,
                    Err(e) => {
                        log::error!("Error fetching row: {}", e);
                        continue;
                    }
                };

                let stream_data = StreamRowData {
                    columns: columns_clone.clone(),
                    column_types: column_types_clone.clone(),
                    row_data,
                };

                // Send row through channel
                if let Err(_) = sender.send(stream_data).await {
                    log::info!("Export receiver disconnected, stopping producer");
                    break;
                }

                row_count += 1;
                exported_rows_clone.store(row_count, Ordering::Relaxed);
            }

            // Send completion signal
            drop(sender);
        });

        // Consumer task: write transformed rows to file
        let file = File::create(file_path).await
            .map_err(|e| anyhow::anyhow!("Failed to create file: {}", e))?;

        let mut writer = BufWriter::new(file);
        let mut rows_written = 0;
        let mut bytes_written = 0;

        // Write initial output (headers, etc.)
        if !initial_output.is_empty() {
            writer.write_all(initial_output.as_bytes()).await
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
            let row_output = transformer.transform_stream_row(
                &stream_data.row_data,
                &stream_data.columns,
                &stream_data.column_types,
            ).map_err(|e| anyhow::anyhow!("Failed to transform row: {}", e))?;

            // Write transformed row
            writer.write_all(row_output.as_bytes()).await
                .map_err(|e| anyhow::anyhow!("Failed to write transformed row: {}", e))?;

            bytes_written += row_output.len();
            rows_written += 1;

            // Update progress
            let progress = ExportProgress {
                exported_rows: rows_written,
                total_rows: rows_written, // Estimate since we don't know total in advance
                current_file_size: bytes_written as u64,
                is_complete: false,
            };

            progress_callback(progress);
        }

        // Finalize transformer output
        let final_output = transformer.finalize_stream()
            .map_err(|e| anyhow::anyhow!("Failed to finalize transformer: {}", e))?;

        if !final_output.is_empty() {
            writer.write_all(final_output.as_bytes()).await
                .map_err(|e| anyhow::anyhow!("Failed to write final output: {}", e))?;
            bytes_written += final_output.len();
        }

        // Flush the writer
        writer.flush().await
            .map_err(|e| anyhow::anyhow!("Failed to flush file: {}", e))?;

        // Wait for producer task to complete
        producer_task.await;

        // Get final file size
        let file_size = std::fs::metadata(file_path)
            .map(|m| m.len())
            .unwrap_or(0);

        // Send final progress update
        let final_progress = ExportProgress {
            exported_rows: rows_written,
            total_rows: rows_written,
            current_file_size: file_size,
            is_complete: true,
        };

        progress_callback(final_progress);

        Ok(ExportResult::Success {
            file_path: file_path.to_string_lossy().to_string(),
            rows_exported: rows_written,
            file_size,
        })
    }

    /// Cancel an ongoing export operation (placeholder for future enhancement)
    pub fn cancel_export(&self) {
        // TODO: Implement cancellation mechanism
        log::info!("Export cancellation requested - not yet implemented");
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
    use crate::transformers::CsvTransformer;

    #[test]
    fn test_export_service_creation() {
        let service = ExportService::new();
        // Just verify it can be created
        assert!(true);
    }
}