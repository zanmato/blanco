use std::collections::{HashMap, HashSet};
use std::ops::Range;

use serde_json::Value;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, Styled, Subscription,
    Window, div, px,
};
use gpui_component::{
    ActiveTheme, Icon,
    input::{Input, InputEvent, InputState},
    menu::PopupMenu,
    table::{Column, ColumnSort, Table, TableDelegate, TableState},
    v_flex,
};

use crate::app::{AddRow, DuplicateRow};
use crate::app_events::AppEvent;
use crate::transformers::CopyHandler;
use database::DatabaseService;

// Response structure for table operations
#[derive(Debug, Clone)]
pub struct TableOperationResponse {
    pub table_name: String,
    pub connection_id: i64,
    pub success: bool,
    pub rows_affected: Option<u64>,
    pub error_message: Option<String>,
    pub operations_executed: usize,
}
use blanco_core::QueryResult;
mod table_operations;
use blanco_ui::IconName;
use table_operations::{ColumnChange, OperationType, RowIdentifier, TableChangeOperation};

// Data structures for copy functionality
#[derive(Clone, Debug)]
pub struct SelectedCell {
    #[allow(dead_code)]
    pub row: usize,
    pub col: usize,
    pub value: String,
    pub column_name: Option<String>,
    pub column_type: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SelectedRow {
    pub row: usize,
    pub cells: Vec<SelectedCell>,
    #[allow(dead_code)]
    pub primary_key_value: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SelectedTableData {
    pub table_name: Option<String>,
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub selected_rows: Vec<SelectedRow>,
    #[allow(dead_code)]
    pub primary_key_column: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TableChange {
    pub change_type: ChangeType,
    pub table_name: String,
    pub row_index: usize,
    pub column_index: Option<usize>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub primary_key_value: Option<String>,
    pub primary_key_column: Option<String>,
    // New fields for prepared statements
    pub sql_template: Option<String>,
    pub parameters: Vec<String>,
    // For INSERT operations, store multiple column values
    pub insert_values: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChangeType {
    UpdateCell,
    InsertRow,
}

/// Format a value for display in table cells, replacing whitespace with visual indicators
/// to maintain table layout while showing multi-line content.
/// - \n (newline) becomes ⏎
/// - \r (carriage return) becomes ␍
/// - \t (tab) becomes ⇥
fn format_value_for_display(value: &str) -> String {
    value
        .replace('\r', "␍")
        .replace('\n', "⏎")
        .replace('\t', "⇥")
}

#[derive(Clone, Debug, Default)]
pub struct CellEditState {
    pub editing_cell: Option<(usize, usize)>,  // (row, col)
    pub expanded_cell: Option<(usize, usize)>, // (row, col) - cell in expanded multi-line mode
    pub original_values: HashMap<(usize, usize), String>,
    pub edited_values: HashMap<(usize, usize), String>,
    pub pending_new_rows: Vec<usize>, // Track rows that are newly added
    pub editing_input: Option<Entity<InputState>>, // Store input state per delegate
    pub changes: Vec<TableChange>,    // Track all changes for SQL generation
    pub selected_rows: HashSet<usize>, // Track selected rows
    pub current_column: usize,        // Track current column for selection
}

impl CellEditState {
    pub fn is_editing(&self, row: usize, col: usize) -> bool {
        self.editing_cell == Some((row, col))
    }

    pub fn is_edited(&self, row: usize, col: usize) -> bool {
        self.edited_values.contains_key(&(row, col))
    }

    pub fn is_expanded(&self, row: usize, col: usize) -> bool {
        self.expanded_cell == Some((row, col))
    }

    pub fn toggle_expanded(&mut self, row: usize, col: usize) {
        if self.expanded_cell == Some((row, col)) {
            tracing::info!("Collapsing cell at ({}, {})", row, col);
            self.expanded_cell = None;
        } else {
            tracing::info!("Expanding cell at ({}, {})", row, col);
            self.expanded_cell = Some((row, col));
        }
    }

    pub fn get_edited_value(&self, row: usize, col: usize) -> Option<&String> {
        self.edited_values.get(&(row, col))
    }

    #[allow(dead_code)]
    pub fn get_original_value(&self, row: usize, col: usize) -> Option<&String> {
        self.original_values.get(&(row, col))
    }

    pub fn start_editing(&mut self, row: usize, col: usize, input: Entity<InputState>) {
        self.editing_cell = Some((row, col));
        self.editing_input = Some(input);
    }

    pub fn stop_editing(&mut self) {
        self.editing_cell = None;
        self.editing_input = None;
        self.expanded_cell = None;
    }

    pub fn get_editing_input(&self) -> Option<Entity<InputState>> {
        self.editing_input.clone()
    }

    pub fn has_unsaved_changes(&self) -> bool {
        !self.edited_values.is_empty()
            || !self.pending_new_rows.is_empty()
            || !self.changes.is_empty()
    }

    pub fn clear_edits(&mut self) {
        self.edited_values.clear();
        self.original_values.clear();
        self.pending_new_rows.clear();
        self.editing_cell = None;
        self.editing_input = None;
        self.expanded_cell = None;
    }

    pub fn clear_all(&mut self) {
        self.editing_cell = None;
        self.original_values.clear();
        self.edited_values.clear();
        self.pending_new_rows.clear();
        self.editing_input = None;
        self.expanded_cell = None;
        self.changes.clear();
        self.selected_rows.clear();
        self.current_column = 1; // Start with first data column
    }

    pub fn add_change(&mut self, change: TableChange) {
        // For UpdateCell changes, replace any existing change for the same cell
        // This prevents duplicate SET clauses for the same column
        if change.change_type == ChangeType::UpdateCell
            && let Some(col_idx) = change.column_index
        {
            // Remove any existing UpdateCell change for this same (row, column) combination
            self.changes.retain(|existing_change| {
                existing_change.change_type != ChangeType::UpdateCell
                    || existing_change.row_index != change.row_index
                    || existing_change.column_index != Some(col_idx)
            });
        }
        self.changes.push(change);
    }

    pub fn clear_changes(&mut self) {
        self.changes.clear();
        self.edited_values.clear();
        self.original_values.clear();
        self.pending_new_rows.clear();
    }

    pub fn is_new_row(&self, row_index: usize) -> bool {
        self.pending_new_rows.contains(&row_index)
    }

    pub fn clear_selection(&mut self) {
        self.selected_rows.clear();
    }
}

/// Builder for creating TableChange instances
pub struct TableChangeBuilder {
    change_type: ChangeType,
    table_name: String,
    row_index: usize,
    column_index: Option<usize>,
    old_value: Option<String>,
    new_value: Option<String>,
    primary_key_value: Option<String>,
    primary_key_column: Option<String>,
    insert_values: Option<Vec<String>>,
}

impl TableChangeBuilder {
    pub fn new(change_type: ChangeType, table_name: String, row_index: usize) -> Self {
        Self {
            change_type,
            table_name,
            row_index,
            column_index: None,
            old_value: None,
            new_value: None,
            primary_key_value: None,
            primary_key_column: None,
            insert_values: None,
        }
    }

    pub fn column_index(mut self, column_index: Option<usize>) -> Self {
        self.column_index = column_index;
        self
    }

    pub fn old_value(mut self, old_value: Option<String>) -> Self {
        self.old_value = old_value;
        self
    }

    pub fn new_value(mut self, new_value: Option<String>) -> Self {
        self.new_value = new_value;
        self
    }

    pub fn primary_key_value(mut self, primary_key_value: Option<String>) -> Self {
        self.primary_key_value = primary_key_value;
        self
    }

    pub fn primary_key_column(mut self, primary_key_column: Option<String>) -> Self {
        self.primary_key_column = primary_key_column;
        self
    }

    pub fn insert_values(mut self, values: Option<Vec<String>>) -> Self {
        self.insert_values = values;
        self
    }

    pub fn build(self) -> TableChange {
        let mut change = TableChange {
            change_type: self.change_type,
            table_name: self.table_name,
            row_index: self.row_index,
            column_index: self.column_index,
            old_value: self.old_value,
            new_value: self.new_value,
            primary_key_value: self.primary_key_value,
            primary_key_column: self.primary_key_column,
            sql_template: None,
            parameters: Vec::new(),
            insert_values: self.insert_values,
        };

        // Generate prepared statement immediately
        let _ = change.generate_prepared_statement();
        change
    }
}

impl TableChange {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        change_type: ChangeType,
        table_name: String,
        row_index: usize,
        column_index: Option<usize>,
        old_value: Option<String>,
        new_value: Option<String>,
        primary_key_value: Option<String>,
        primary_key_column: Option<String>,
        insert_values: Option<Vec<String>>,
    ) -> Self {
        TableChangeBuilder::new(change_type, table_name, row_index)
            .column_index(column_index)
            .old_value(old_value)
            .new_value(new_value)
            .primary_key_value(primary_key_value)
            .primary_key_column(primary_key_column)
            .insert_values(insert_values)
            .build()
    }

    fn generate_prepared_statement(&mut self) -> Result<(), String> {
        match self.change_type {
            ChangeType::UpdateCell => {
                // For now, generate a template without column name
                // The actual column name will be filled in when we have access to columns
                let pk_column = self
                    .primary_key_column
                    .clone()
                    .or_else(|| Some("id".to_string())) // Default fallback
                    .ok_or_else(|| "No primary key column".to_string())?;

                // UPDATE table_name SET column_name = $1 WHERE pk_column = $2
                // Note: column_name will be substituted when we have column info
                self.sql_template = Some(format!(
                    "UPDATE {} SET {} = $1 WHERE {} = $2",
                    self.table_name, "COLUMN_PLACEHOLDER", pk_column
                ));

                // Parameters: $1 = new_value, $2 = pk_value
                self.parameters.clear();
                if let Some(new_val) = &self.new_value {
                    self.parameters.push(new_val.clone());
                } else {
                    self.parameters.push("NULL".to_string());
                }

                if let Some(pk_val) = &self.primary_key_value {
                    self.parameters.push(pk_val.clone());
                } else {
                    return Err("No primary key value available".to_string());
                }
            }
            ChangeType::InsertRow => {
                // For INSERT, use the insert_values if available, otherwise fall back to new_value
                if let Some(values) = &self.insert_values {
                    // Create parameter placeholders ($1, $2, $3, ...)
                    let placeholders: Vec<String> =
                        (1..=values.len()).map(|i| format!("${}", i)).collect();

                    self.sql_template = Some(format!(
                        "INSERT INTO {} VALUES ({})",
                        self.table_name,
                        placeholders.join(", ")
                    ));

                    // Use the individual values as parameters
                    self.parameters.clear();
                    self.parameters.extend(values.clone());
                } else if let Some(new_val) = &self.new_value {
                    // Fallback for backward compatibility
                    self.sql_template =
                        Some(format!("INSERT INTO {} VALUES ($1)", self.table_name));
                    self.parameters.clear();
                    self.parameters.push(new_val.clone());
                } else {
                    self.sql_template =
                        Some(format!("INSERT INTO {} VALUES ($1)", self.table_name));
                    self.parameters.clear();
                    self.parameters.push("NULL".to_string());
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct ResultsTableDelegate {
    columns: Vec<Column>,
    column_types: Vec<String>,
    rows: Vec<Vec<String>>,
    edit_state: CellEditState,
    table_name: Option<String>,
    primary_key_column: Option<String>,
    pending_edit_cell: Option<(usize, usize)>,
    connection_id: i64,
    database_name: String,
    original_query: Option<String>,
}

impl ResultsTableDelegate {
    /// Remove a row at the specified index
    pub fn remove_row(&mut self, row_index: usize) {
        if row_index < self.rows.len() {
            self.rows.remove(row_index);
        }
    }

    /// Find the column index of the primary key column (excluding row number column)
    pub fn get_primary_key_column_index(&self) -> Option<usize> {
        if let Some(pk_column) = &self.primary_key_column {
            // Skip row number column (index 0) and find the primary key in data columns
            self.columns
                .iter()
                .skip(1)
                .position(|col| col.name.as_str() == pk_column)
        } else {
            None
        }
    }

    /// Get column names for INSERT operations, excluding row number and primary key (for new rows)
    pub fn get_insert_column_names(&self, exclude_primary_key: bool) -> Vec<String> {
        let pk_index = if exclude_primary_key {
            self.get_primary_key_column_index()
        } else {
            None
        };

        self.columns
            .iter()
            .skip(1) // Skip row number column
            .enumerate()
            .filter_map(|(data_index, col)| {
                // Convert data_index back to full column index
                let _full_index = data_index + 1;
                if exclude_primary_key
                    && let Some(pk_data_index) = pk_index
                    && data_index == pk_data_index
                {
                    return None; // Skip primary key column
                }
                Some(col.name.to_string())
            })
            .collect()
    }

    /// Get column values for INSERT operations, excluding row number and primary key (for new rows)
    pub fn get_insert_values(&self, row_index: usize, exclude_primary_key: bool) -> Vec<String> {
        let pk_index = if exclude_primary_key {
            self.get_primary_key_column_index()
        } else {
            None
        };

        if let Some(row) = self.rows.get(row_index) {
            row.iter()
                .skip(1) // Skip row number column
                .enumerate()
                .filter_map(|(data_index, val)| {
                    if exclude_primary_key
                        && let Some(pk_data_index) = pk_index
                        && data_index == pk_data_index
                    {
                        return None; // Skip primary key column
                    }

                    // Check if there's an edited value for this cell
                    let display_col = data_index + 1; // +1 for row number column
                    if let Some(edited_value) =
                        self.edit_state.edited_values.get(&(row_index, display_col))
                    {
                        return Some(edited_value.clone());
                    }

                    Some(val.clone())
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get a mutable reference to a cell
    pub fn get_cell_mut(&mut self, row: usize, col: usize) -> Option<&mut String> {
        self.rows
            .get_mut(row)
            .and_then(|row_data| row_data.get_mut(col))
    }

    /// Set the connection ID for database operations
    pub fn set_connection_id(&mut self, connection_id: i64, database_name: &str) {
        self.connection_id = connection_id;
        self.database_name = database_name.to_owned();
    }

    /// Set the original SQL query for alias resolution
    pub fn set_original_query(&mut self, query: String) {
        self.original_query = Some(query);
    }

    /// Convert table changes to database-agnostic TableChangeOperations
    /// This method consolidates multiple changes to the same row into single operations.
    pub fn create_change_operations(&self) -> Vec<table_operations::TableChangeOperation> {
        use std::collections::HashMap;

        // Map to consolidate changes by (table_name, pk_column, pk_value)
        let mut update_operations: HashMap<(String, String, String), Vec<ColumnChange>> =
            HashMap::new();
        let mut insert_operations: Vec<table_operations::TableChangeOperation> = Vec::new();

        for change in &self.edit_state.changes {
            match change.change_type {
                ChangeType::UpdateCell => {
                    // Get primary key information
                    let (pk_column, pk_value) = if let (Some(pk_col), Some(pk_val)) =
                        (&change.primary_key_column, &change.primary_key_value)
                    {
                        (pk_col.clone(), pk_val.clone())
                    } else {
                        // Fallback: try to get primary key from delegate
                        if let Some(ref pk_column) = self.primary_key_column {
                            if let Some(pk_value) =
                                self.rows.get(change.row_index).and_then(|row| row.first())
                            {
                                (pk_column.clone(), pk_value.clone())
                            } else {
                                continue; // Skip this change if we can't determine PK
                            }
                        } else {
                            continue; // Skip this change if we can't determine PK
                        }
                    };

                    // Get column name from index
                    let column_name = self
                        .columns
                        .get(change.column_index.unwrap_or(0))
                        .map(|col| col.name.to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    // Create column change
                    let column_change = ColumnChange {
                        column_name,
                        old_value: change.old_value.clone(),
                        new_value: change.new_value.clone(),
                    };

                    // Add to the consolidated operation
                    let key = (change.table_name.clone(), pk_column, pk_value);
                    update_operations
                        .entry(key)
                        .or_default()
                        .push(column_change);
                }
                ChangeType::InsertRow => {
                    // For INSERT operations, get the current values from the actual row data
                    // This ensures we use the most up-to-date values instead of stored ones

                    // Check if primary key should be excluded (only if it's NULL/auto-generated)
                    let pk_col_index = self.get_primary_key_column_index();
                    let exclude_primary_key = pk_col_index.is_some_and(|idx| {
                        // Get the current value of the primary key column
                        let display_col = idx + 1; // +1 for row number column

                        // Check edited values first, then fall back to row data
                        let pk_value = self
                            .edit_state
                            .edited_values
                            .get(&(change.row_index, display_col))
                            .or_else(|| {
                                self.rows
                                    .get(change.row_index)
                                    .and_then(|row| row.get(display_col))
                            });

                        // Only exclude if PK is NULL or empty
                        pk_value.is_none_or(|v| v.is_empty() || v == "NULL")
                    });

                    let column_names = self.get_insert_column_names(exclude_primary_key);
                    let row_values = self.get_insert_values(change.row_index, exclude_primary_key);

                    let column_changes: Vec<ColumnChange> = column_names
                        .into_iter()
                        .zip(row_values.iter())
                        .map(|(column_name, value)| {
                            // Pass raw values - to_sql_query() will handle SQL formatting
                            let formatted_value = if value.is_empty() || value == "NULL" {
                                "NULL".to_string()
                            } else {
                                value.to_string() // Pass raw value without SQL formatting
                            };
                            ColumnChange {
                                column_name,
                                old_value: None,
                                new_value: Some(formatted_value),
                            }
                        })
                        .collect();

                    insert_operations.push(TableChangeOperation::insert_row(
                        change.table_name.clone(),
                        column_changes,
                    ));
                }
            }
        }

        // Convert consolidated update operations to TableChangeOperations
        let mut operations = Vec::new();
        for ((table_name, pk_column, pk_value), column_changes) in update_operations {
            let operation = TableChangeOperation {
                operation_type: OperationType::Update,
                table_name,
                row_identifier: RowIdentifier::PrimaryKey {
                    column: pk_column,
                    value: pk_value,
                },
                changes: column_changes,
            };
            operations.push(operation);
        }

        // Add insert operations
        operations.extend(insert_operations);

        operations
    }

    pub fn set_query_result(&mut self, result: QueryResult) {
        // Clear previous edit state
        self.edit_state.clear_all();
        self.pending_edit_cell = None;

        // Store column types
        self.column_types = result.column_types.clone();

        // TODO: calculate actual width
        // let width = window
        //         .text_system()
        //         .shape_line(
        //             longest_line.clone(),
        //             text_size,
        //             &[TextRun {
        //                 len: longest_line.len(),
        //                 font: style.font(),
        //                 color: gpui::black(),
        //                 background_color: None,
        //                 underline: None,
        //                 strikethrough: None,
        //             }],
        //             wrap_width,
        //         )
        //         .width;

        // Calculate column widths based on content and type
        let mut column_widths: Vec<f32> = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col_name)| {
                let mut max_width = col_name.len() as f32 * 9.0; // Smaller font size = smaller multiplier

                // Check some sample rows to determine content width
                for row in result.rows.iter().take(20) {
                    // Sample more rows for better accuracy
                    if let Some(cell_value) = row.get(i) {
                        let content_width = cell_value.len() as f32 * 9.0; // Adjusted for smaller font
                        max_width = max_width.max(content_width);
                    }
                }

                // Add padding for cell content
                max_width += 16.0; // Account for px_2 padding on each side

                // Account for cell borders and extra spacing
                max_width += 2.0;

                // Apply minimum and maximum bounds (adjusted for smaller font)
                max_width.clamp(60.0, 400.0)
            })
            .collect();

        // Add row number column width at the beginning
        let row_num_width = 50.0; // Fixed width for row numbers
        column_widths.insert(0, row_num_width);

        // Build columns from result with calculated widths, starting with row number column
        let mut columns = vec![
            Column::new("row_number".to_string(), "#".to_string())
                .width(row_num_width)
                .resizable(false),
        ];

        columns.extend(result.columns.iter().enumerate().map(|(i, name)| {
            Column::new(format!("col_{}", i + 1), name)
                .width(column_widths.get(i + 1).copied().unwrap_or(150.0))
                .resizable(true)
                .sortable()
        }));

        self.columns = columns;

        // Add row numbers to the beginning of each row
        self.rows = result
            .rows
            .into_iter()
            .enumerate()
            .map(|(row_index, mut row)| {
                let mut new_row = vec![(row_index + 1).to_string()];
                new_row.append(&mut row);
                new_row
            })
            .collect();

        // Use table metadata from QueryResult if available
        let old_table_name = self.table_name.clone();
        self.table_name = result.table_name.clone();

        // Debug logging for table name extraction
        match (&old_table_name, &self.table_name) {
            (Some(old), Some(new)) => {
                if old != new {
                    tracing::info!("Table name changed from '{}' to '{}'", old, new);
                } else {
                    tracing::debug!("Table name unchanged: '{}'", new);
                }
            }
            (None, Some(new)) => {
                tracing::info!("Table name extracted from metadata: '{}'", new);
            }
            (Some(old), None) => {
                tracing::warn!(
                    "Table name lost: '{}' (was extracted before, now None)",
                    old
                );
            }
            (None, None) => {
                if result.query_text.is_some() {
                    tracing::warn!("Failed to extract table name from query metadata");
                }
            }
        }

        // Use primary key from QueryResult if available, otherwise fall back to heuristic
        self.primary_key_column = result.primary_key_column.clone().or_else(|| {
            self.table_name
                .as_ref()
                .and_then(|table_name| self.detect_primary_key_simple(table_name))
        });
    }

    /// Simple heuristic method to detect primary key column (fallback)
    fn detect_primary_key_simple(&self, _table_name: &str) -> Option<String> {
        // Simple heuristic: look for common primary key column names
        for column in self.columns.iter() {
            let column_name_lower = column.name.to_lowercase();
            if column_name_lower.contains("id")
                || column_name_lower == "uuid"
                || column_name_lower.ends_with("_id")
                || column_name_lower == "pk"
                || column_name_lower.ends_with("_pk")
            {
                return Some(column.name.to_string());
            }
        }

        // Fallback: return the first column name
        self.columns.first().map(|col| col.name.to_string())
    }
    pub fn start_editing_cell(&mut self, row: usize, col: usize) {
        if let Some(cell_value) = self.rows.get(row).and_then(|r| r.get(col)) {
            // Store the original value if not already stored
            self.edit_state
                .original_values
                .entry((row, col))
                .or_insert_with(|| cell_value.clone());
            self.edit_state.editing_cell = Some((row, col));
        }
    }

    pub fn set_pending_edit_cell(&mut self, row: usize, col: usize) {
        self.pending_edit_cell = Some((row, col));
    }

    pub fn update_cell_value(&mut self, row: usize, col: usize, new_value: String) {
        self.edit_state.edited_values.insert((row, col), new_value);
    }

    pub fn commit_cell_edit(&mut self, row: usize, col: usize) -> Option<String> {
        tracing::info!("delegate.commit_cell_edit called for ({}, {})", row, col);
        tracing::info!(
            "edited_values contains: {:?}",
            self.edit_state.edited_values
        );

        if let Some(new_value) = self.edit_state.edited_values.get(&(row, col)).cloned() {
            tracing::info!("Found edited value: '{}' for ({}, {})", new_value, row, col);

            // Get the original value
            let original_value = self.edit_state.original_values.get(&(row, col)).cloned();

            // Update the actual row data
            if let Some(row_data) = self.rows.get_mut(row) {
                if let Some(cell) = row_data.get_mut(col) {
                    tracing::info!("Updating cell from '{}' to '{}'", cell, new_value);
                    *cell = new_value.clone();
                    tracing::info!("Cell updated successfully");
                } else {
                    tracing::info!("No cell found at column {}", col);
                }
            } else {
                tracing::info!("No row data found at row {}", row);
            }

            // Track the change for SQL generation (but not for new rows)
            if let (Some(original), Some(table_name)) = (&original_value, &self.table_name) {
                // Check if this is a new row - if so, don't create UPDATE changes
                // New rows should be handled by INSERT operations only
                if !self.edit_state.is_new_row(row) {
                    // Get primary key value - if updating the PK column itself, use the original value
                    let primary_key_value = if let Some(pk_column) = &self.primary_key_column {
                        // Find the index of the primary key column
                        if let Some(pk_index) = self
                            .columns
                            .iter()
                            .position(|col| col.name.as_str() == pk_column)
                        {
                            // If we're updating the primary key column itself, get the original value
                            if pk_index == col {
                                self.edit_state.original_values.get(&(row, col)).cloned()
                            } else {
                                // Otherwise get the current value from the row
                                self.rows.get(row).and_then(|r| r.get(pk_index)).cloned()
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Validate change before creating
                    let _validation_status = if primary_key_value.is_none() {
                        "INVALID: No primary key value"
                    } else if self.primary_key_column.is_none() {
                        "WARNING: No primary key column detected"
                    } else {
                        "VALID"
                    };

                    let change = TableChange::new(
                        ChangeType::UpdateCell,
                        table_name.clone(),
                        row,
                        Some(col),
                        Some(original.clone()),
                        Some(new_value.clone()),
                        primary_key_value,
                        self.primary_key_column.clone(),
                        None, // No insert_values for UpdateCell operations
                    );

                    self.edit_state.add_change(change);
                }
            }

            // Clear only the editing state, keep edited_values for visual indicator
            self.edit_state.editing_cell = None;
            // Note: Keep in edited_values to maintain yellow border until committed to database

            Some(new_value)
        } else {
            // No changes to commit, just clear editing state
            self.edit_state.editing_cell = None;
            None
        }
    }

    pub fn cancel_cell_edit(&mut self, row: usize, col: usize) {
        // Clear editing state and any edited values for this cell
        self.edit_state.editing_cell = None;
        self.edit_state.edited_values.remove(&(row, col));
    }

    pub fn get_table_name(&self) -> Option<&str> {
        self.table_name.as_deref()
    }

    pub fn is_numeric_column(&self, col_index: usize) -> bool {
        if let Some(column_type) = self.column_types.get(col_index) {
            let type_lower = column_type.to_lowercase();
            // SQL standard names
            type_lower.contains("int")
                || type_lower.contains("float")
                || type_lower.contains("double")
                || type_lower.contains("numeric")
                || type_lower.contains("decimal")
                || type_lower.contains("real")
                || type_lower.contains("smallint")
                || type_lower.contains("bigint")
                || type_lower.contains("serial")
                || type_lower.contains("money")
                // PostgreSQL internal type names
                || type_lower == "int2"    // smallint
                || type_lower == "int4"    // integer
                || type_lower == "int8"    // bigint
                || type_lower == "float4"  // real
                || type_lower == "float8"  // double precision
                || type_lower == "numeric" // numeric
                || type_lower == "money" // money
        } else {
            false
        }
    }

    /// Check if a column contains UUID data
    pub fn is_uuid_column(&self, col_index: usize) -> bool {
        if let Some(column_type) = self.column_types.get(col_index) {
            let type_lower = column_type.to_lowercase();
            type_lower.contains("uuid")
        } else {
            false
        }
    }

    /// Check if a column contains timestamp data (timestamp or timestamptz)
    pub fn is_timestamp_column(&self, col_index: usize) -> bool {
        if let Some(column_type) = self.column_types.get(col_index) {
            let type_lower = column_type.to_lowercase();
            type_lower.contains("timestamp")
                || type_lower.contains("timestamptz")
                || type_lower.contains("datetime")
        } else {
            false
        }
    }

    /// Check if a column contains JSON or JSONB data
    pub fn is_json_column(&self, col_index: usize) -> bool {
        if let Some(column_type) = self.column_types.get(col_index) {
            let type_lower = column_type.to_lowercase();
            type_lower.contains("json")
        } else {
            false
        }
    }

    /// Check if a column contains array data (PostgreSQL returns "ARRAY" or "type[]")
    pub fn is_array_column(&self, col_index: usize) -> bool {
        if let Some(column_type) = self.column_types.get(col_index) {
            let type_lower = column_type.to_lowercase();

            type_lower == "array" || type_lower.ends_with("[]")
        } else {
            tracing::debug!("Array detection: No column type for index {}", col_index);
            false
        }
    }

    pub fn is_editable(&self) -> bool {
        self.table_name.is_some() && self.primary_key_column.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.edit_state.clear_selection();
    }
}

impl TableDelegate for ResultsTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_row_number_col = col_ix == 0;
        let col = &self.columns[col_ix];
        div()
            .font_family(cx.theme().mono_font_family.clone())
            .text_sm()
            .pt(px(1.))
            .child(col.name.to_string())
            .when(is_row_number_col, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |table, event: &gpui::MouseDownEvent, _window, cx| {
                        if event.click_count == 1 {
                            table.select_all_rows(cx);
                            table.refresh(cx);
                            cx.notify();
                        }
                    }),
                )
            })
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_row_number_col = col_ix == 0;
        let is_editing = self.edit_state.is_editing(row_ix, col_ix) && !is_row_number_col;
        let is_edited = self.edit_state.is_edited(row_ix, col_ix) && !is_row_number_col;
        let is_editable = self.is_editable() && !is_row_number_col;

        let current_value = if is_edited {
            self.edit_state.get_edited_value(row_ix, col_ix).cloned()
        } else {
            self.rows
                .get(row_ix)
                .and_then(|row| row.get(col_ix))
                .cloned()
        }
        .unwrap_or_else(|| "--".to_string());

        // Check if the value is NULL
        let is_null = current_value.eq_ignore_ascii_case("null") || current_value == "--";
        let display_text = if is_null {
            "NULL".to_string()
        } else {
            // Format multi-line values for display (replace newlines with visual indicators)
            format_value_for_display(&current_value)
        };

        if is_editing {
            // Embed Input directly in the cell (not for row number column)
            if let Some(input) = self.edit_state.get_editing_input() {
                let is_expanded = self.edit_state.is_expanded(row_ix, col_ix);
                let is_json = self.is_json_column(col_ix - 1);
                let input = input.clone();

                if is_expanded {
                    // Expanded mode: absolute positioned input with larger size
                    div()
                        .bg(cx.theme().background)
                        .border_2()
                        .border_color(cx.theme().yellow)
                        .p_0()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(
                            gpui::deferred(
                                div()
                                    .absolute()
                                    .right(px(0.))
                                    .top(px(0.))
                                    .w(px(600.))
                                    .h(px(200.))
                                    .bg(cx.theme().background)
                                    .shadow_lg()
                                    .child(
                                        Input::new(&input).size_full().font_family(cx.theme().mono_font_family.clone()).text_size(px(12.)).suffix(
                                            div()
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        move |table, _event, window, cx| {
                                                            // Get current text before recreating input
                                                            let current_text = table
                                                                .delegate_mut()
                                                                .edit_state
                                                                .editing_input
                                                                .as_ref().map(|input| input
                                                                            .read(cx)
                                                                            .text()
                                                                            .to_string())
                                                                .unwrap_or_default();

                                                            // Recreate InputState with single-line mode and subscribe to events
                                                            let new_input = cx.new(|cx| {
                                                                InputState::new(window, cx)
                                                                    .default_value(current_text)
                                                            });
                                                            table
                                                                .delegate_mut()
                                                                .edit_state
                                                                .editing_input =
                                                                Some(new_input.clone());

                                                            // Re-subscribe to input events (blur/change)
                                                            ResultsPanel::subscribe_to_input_events(
                                                                table, &new_input, row_ix, col_ix,
                                                                cx,
                                                            );

                                                            // Re-focus the input after recreation
                                                            new_input
                                                                .focus_handle(cx)
                                                                .focus(window, cx);

                                                            // Toggle expanded state
                                                            table
                                                                .delegate_mut()
                                                                .edit_state
                                                                .toggle_expanded(row_ix, col_ix);

                                                            table.refresh(cx);
                                                            cx.notify();
                                                        },
                                                    ),
                                                )
                                                .child(Icon::new(IconName::Minimize).text_xs()),
                                        ),
                                    ),
                            )
                            .with_priority(99),
                        )
                } else {
                    // Normal inline edit with expand icon as suffix
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .size_full()
                        .flex()
                        .items_center()
                        .p_0()
                        .when(self.is_numeric_column(col_ix - 1), |this| {
                            this.justify_end()
                        })
                        .when(self.is_uuid_column(col_ix - 1), |this| {
                            this.text_color(cx.theme().blue)
                        })
                        .when(self.is_timestamp_column(col_ix - 1), |this| {
                            this.text_color(cx.theme().green)
                        })
                        .when(self.is_json_column(col_ix - 1), |this| {
                            this.text_color(cx.theme().yellow)
                        })
                        .when(self.is_array_column(col_ix - 1), |this| {
                            this.text_color(cx.theme().blue)
                        })
                        .child(
                            Input::new(&input)
                                .flex_1()
                                .text_size(px(12.))
                                .border_0()
                                .suffix(
                                    div()
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |table, _event, window, cx| {
                                                // Get current text before recreating input
                                                let current_text = table
                                                    .delegate_mut()
                                                    .edit_state
                                                    .editing_input
                                                    .as_ref()
                                                    .map(|input| input.read(cx).text().to_string())
                                                    .unwrap_or_default();

                                                // Toggle expanded state
                                                table
                                                    .delegate_mut()
                                                    .edit_state
                                                    .toggle_expanded(row_ix, col_ix);

                                                // Recreate InputState with multi-line mode
                                                let new_input = cx.new(|cx| {
                                                    let editor = InputState::new(window, cx)
                                                        .multi_line(true)
                                                        .soft_wrap(true)
                                                        .show_context_menu(false);

                                                    if is_json {
                                                        // Prettify JSON if valid
                                                        let prettified_text = if let Ok(value) =
                                                            serde_json::from_str::<Value>(
                                                                &current_text,
                                                            ) {
                                                            serde_json::to_string_pretty(&value)
                                                                .unwrap_or(current_text)
                                                        } else {
                                                            current_text
                                                        };
                                                        editor
                                                            .code_editor("json")
                                                            .default_value(prettified_text)
                                                    } else {
                                                        editor.default_value(current_text)
                                                    }
                                                });
                                                table.delegate_mut().edit_state.editing_input =
                                                    Some(new_input.clone());

                                                // Re-subscribe to input events (blur/change)
                                                ResultsPanel::subscribe_to_input_events(
                                                    table, &new_input, row_ix, col_ix, cx,
                                                );

                                                // Re-focus the input after recreation
                                                new_input.focus_handle(cx).focus(window, cx);

                                                table.refresh(cx);
                                                cx.notify();
                                            }),
                                        )
                                        .child(Icon::new(IconName::Maximize).text_xs()),
                                ),
                        )
                }
            } else {
                div().child("")
            }
        } else {
            // Check if this is a numeric column for right-alignment (adjust for row number column)
            let is_numeric = !is_row_number_col && self.is_numeric_column(col_ix - 1);

            // Render static cell with appropriate handlers
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(12.))
                .size_full() // Fill the entire cell container
                .flex() // Enable flexbox layout
                .items_center() // Center vertically
                .when(is_row_number_col, |this| {
                    this.font_weight(FontWeight::BOLD) // Bold row numbers
                        .text_color(cx.theme().muted_foreground) // Muted color for row numbers
                        .cursor_pointer() // Pointer cursor for row selection
                        .when(self.edit_state.is_new_row(row_ix), |this| {
                            this.border_l_3().border_color(cx.theme().yellow)
                        })
                })
                .when(is_numeric && !is_row_number_col, |this| {
                    this.justify_end() // Right-align numeric columns
                        .text_color(cx.theme().foreground) // Ensure numeric text is visible
                })
                .when(
                    !is_row_number_col && self.is_uuid_column(col_ix - 1),
                    |this| {
                        this.text_color(cx.theme().blue) // Blue color for UUIDs
                    },
                )
                .when(
                    !is_row_number_col && self.is_timestamp_column(col_ix - 1),
                    |this| {
                        this.text_color(cx.theme().green) // Green color for timestamps
                    },
                )
                .when(
                    !is_row_number_col && self.is_json_column(col_ix - 1),
                    |this| {
                        this.text_color(cx.theme().yellow) // Yellow color for JSON
                    },
                )
                .when(is_edited, |this| {
                    this.bg(cx.theme().yellow.opacity(0.3))
                        .border_l_2()
                        .border_color(cx.theme().yellow)
                })
                .when(is_null, |this| {
                    this.text_color(cx.theme().muted_foreground).italic()
                })
                // Only show visual feedback for editable cells when hovering
                .when(!is_row_number_col && !is_null && is_editable, |this| {
                    this.cursor_pointer()
                })
                // Apply muted grey color only to non-editable tables for cells WITHOUT type-based highlighting
                // Type-based highlighting (UUID=blue, timestamp=green, JSON=yellow, array=blue) should work regardless
                .when(
                    !is_editable
                        && !is_row_number_col
                        && !is_null
                        && !self.is_uuid_column(col_ix - 1)
                        && !self.is_timestamp_column(col_ix - 1)
                        && !self.is_json_column(col_ix - 1)
                        && !self.is_array_column(col_ix - 1),
                    |this| this.text_color(cx.theme().muted_foreground.opacity(0.6)),
                )
                // All data cells (non-row-number) should be selectable for copying
                .when(!is_row_number_col, |this| {
                    this.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |table, event: &gpui::MouseDownEvent, _window, cx| {
                            if event.click_count == 2 {
                                let delegate = table.delegate_mut();
                                if delegate.is_editable() {
                                    delegate.clear_selection();
                                    delegate.set_pending_edit_cell(row_ix, col_ix);
                                    table.refresh(cx);
                                    cx.notify();
                                }
                            }
                        }),
                    )
                })
                .px_2()
                .py_1()
                .child(display_text)
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        // Don't sort by row number column
        if col_ix == 0 {
            return;
        }

        // Sort rows by the specified column (excluding row number column)
        self.rows.sort_by(|a, b| {
            let a_val = a.get(col_ix).map(|s| s.as_str()).unwrap_or("");
            let b_val = b.get(col_ix).map(|s| s.as_str()).unwrap_or("");

            match sort {
                ColumnSort::Descending => b_val.cmp(a_val),
                _ => a_val.cmp(b_val),
            }
        });

        // Update row numbers after sorting
        for (index, row) in self.rows.iter_mut().enumerate() {
            if let Some(row_num_cell) = row.get_mut(0) {
                *row_num_cell = (index + 1).to_string();
            }
        }
    }

    fn visible_rows_changed(
        &mut self,
        _visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
    }

    fn visible_columns_changed(
        &mut self,
        _visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
    }

    fn render_last_empty_col(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Add extra space to ensure all columns are scrollable
        // This compensates for any viewport calculation issues
        div().w(px(30.0)).h_full().flex_shrink_0()
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        // Basic copy operations

        menu.menu_with_icon(
            "Copy as CSV",
            Icon::new(IconName::Sheet),
            Box::new(crate::app::CopyAsCSV),
        )
        .menu_with_icon(
            "Copy as JSON",
            Icon::new(IconName::Braces),
            Box::new(crate::app::CopyAsJSON),
        )
        .menu_with_icon(
            "Copy as SQL",
            Icon::new(IconName::Database),
            Box::new(crate::app::CopyAsSQL),
        )
        .menu_with_icon(
            "Copy as Markdown",
            Icon::new(IconName::Markdown),
            Box::new(crate::app::CopyAsMarkdown),
        )
        .separator()
        .menu_with_icon("Add Row", Icon::new(IconName::Plus), Box::new(AddRow))
        .menu_with_icon(
            "Duplicate Row",
            Icon::new(IconName::Copy),
            Box::new(DuplicateRow { row: row_ix }),
        )
    }
}

pub struct ResultsPanel {
    focus_handle: FocusHandle,
    table_state: Entity<TableState<ResultsTableDelegate>>,
    current_result: Option<QueryResult>,
    editing_input: Option<Entity<InputState>>,
    editing_cell: Option<(usize, usize)>,
    copy_handler: CopyHandler,
    _subscriptions: Vec<Subscription>, // Store subscriptions to prevent them from being dropped
}

impl ResultsPanel {
    pub fn new(
        connection_id: i64,
        database_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut delegate = ResultsTableDelegate::default();

        // Set connection ID on delegate if provided
        delegate.set_connection_id(connection_id, database_name);

        let table_state = cx.new(|cx| TableState::new(delegate, window, cx).col_selectable(false));

        // Set up event subscriptions
        let subscriptions = Vec::new();

        Self {
            table_state,
            focus_handle: cx.focus_handle(),
            current_result: None,
            editing_input: None,
            editing_cell: None,
            copy_handler: CopyHandler::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_query_result(
        &mut self,
        result: QueryResult,
        _connection_id: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |state, cx| {
            // Set the original query for alias resolution
            if let Some(ref query) = result.query_text {
                state.delegate_mut().set_original_query(query.clone());
            }
            state.delegate_mut().set_query_result(result.clone());
            state.refresh(cx);
        });
        self.current_result = Some(result.clone());

        // Clear any panel-level editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
    }

    pub fn cancel_current_edit(&mut self, cx: &mut Context<Self>) {
        // Use the delegate's editing state instead of the panel's
        let editing_cell = self.table_state.read(cx).delegate().edit_state.editing_cell;

        if let Some((row, col)) = editing_cell {
            self.cancel_cell_edit(row, col, cx);
        }
    }

    pub fn start_cell_edit(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Get the current cell value
        let current_value = self
            .table_state
            .read(cx)
            .delegate()
            .rows
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_else(|| "".to_string());

        // Check if this is a new row (pending insert) or existing row
        let is_new_row = self
            .table_state
            .read(cx)
            .delegate()
            .edit_state
            .pending_new_rows
            .contains(&row);

        // Create input state for editing with the current cell value
        let input = cx.new(|cx| InputState::new(window, cx).default_value(&current_value));

        // Start editing in the delegate with the input
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            delegate.start_editing_cell(row, col);
            delegate.edit_state.start_editing(row, col, input.clone());

            // Store the original value for existing rows
            if !is_new_row {
                delegate
                    .edit_state
                    .original_values
                    .insert((row, col), current_value.clone());
            }

            // Subscribe to input changes to update edited_values
            Self::subscribe_to_input_events(state, &input, row, col, cx);
        });

        // Focus the input automatically when editing starts
        input.focus_handle(cx).focus(window, cx);

        // Store the editing state in the panel for commit/cancel operations
        self.editing_input = Some(input.clone());
        self.editing_cell = Some((row, col));
    }

    /// Subscribe to input events (blur/change) for a given cell
    fn subscribe_to_input_events(
        _state: &mut TableState<ResultsTableDelegate>,
        input: &Entity<InputState>,
        row: usize,
        col: usize,
        cx: &mut Context<TableState<ResultsTableDelegate>>,
    ) {
        let row_clone = row;
        let col_clone = col;
        cx.subscribe(input, move |table, input, event, cx| {
            if let InputEvent::Change = event {
                let new_text = input.read(cx).text().to_string();
                tracing::info!(
                    "Input change: '{}' at ({}, {})",
                    new_text,
                    row_clone,
                    col_clone
                );
                table
                    .delegate_mut()
                    .edit_state
                    .edited_values
                    .insert((row_clone, col_clone), new_text.clone());

                // Debug: Input change handled in edited_values for commit_cell_edit
                // Note: Can't refresh here due to borrowing issues
            } else if let InputEvent::Blur = event {
                // Handle blur - save current edit to edited_values when input loses focus
                // Get the current editing cell and value
                let editing_cell = table.delegate_mut().edit_state.editing_cell;
                tracing::info!("Blur event triggered for editing_cell: {:?}", editing_cell);

                if let Some((row, col)) = editing_cell {
                    // Ignore blur if the cell is in expanded mode
                    if table.delegate_mut().edit_state.is_expanded(row, col) {
                        tracing::info!(
                            "Blur: ignoring blur for expanded cell at ({}, {})",
                            row,
                            col
                        );
                        return;
                    }

                    let new_value = input.read(cx).text().to_string();
                    tracing::info!("Blur: saving value '{}' at ({}, {})", new_value, row, col);

                    // Commit the cell edit to create a TableChange entry
                    tracing::info!("Blur: committing cell edit at ({}, {})", row, col);
                    table.delegate_mut().commit_cell_edit(row, col);
                    table.refresh(cx);
                    tracing::info!("Blur: cell edit committed and table refreshed");
                } else {
                    tracing::info!("Blur: no editing cell found");
                }
            }
        })
        .detach();
    }

    pub fn commit_cell_edit(
        &mut self,
        row: usize,
        col: usize,
        new_value: String,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let mut committed_value = None;
        let mut old_value = None;
        let mut table_name = None;
        let mut primary_key_value = None;

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();

            // Get the original value before updating
            old_value = delegate.rows.get(row).and_then(|r| r.get(col)).cloned();
            table_name = delegate.table_name.clone();

            // Get primary key value if we have a primary key column
            if let Some(pk_column) = &delegate.primary_key_column {
                // Find the index of the primary key column
                if let Some(pk_index) = delegate
                    .columns
                    .iter()
                    .position(|col| col.name.as_str() == pk_column)
                {
                    primary_key_value = delegate
                        .rows
                        .get(row)
                        .and_then(|r| r.get(pk_index))
                        .cloned();
                }
            }

            // Update the cell value
            delegate.update_cell_value(row, col, new_value.clone());
            committed_value = delegate.commit_cell_edit(row, col);

            // Track the change for SQL generation (but not for new rows)
            if let (Some(old_val), Some(tbl_name)) = (&old_value, &table_name)
                && old_val != &new_value
            {
                // Check if this is a new row - if so, don't create UPDATE changes
                // New rows should be handled by INSERT operations only
                if !delegate.edit_state.is_new_row(row) {
                    // Get primary key value - if updating the PK column itself, use the original value
                    let primary_key_value = if let Some(pk_column) = &delegate.primary_key_column {
                        // Find the index of the primary key column
                        if let Some(pk_index) = delegate
                            .columns
                            .iter()
                            .position(|col| col.name.as_str() == pk_column)
                        {
                            // If we're updating the primary key column itself, get the original value
                            if pk_index == col {
                                delegate
                                    .edit_state
                                    .original_values
                                    .get(&(row, col))
                                    .cloned()
                            } else {
                                // Otherwise get the current value from the row
                                delegate
                                    .rows
                                    .get(row)
                                    .and_then(|r| r.get(pk_index))
                                    .cloned()
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let primary_key_column = delegate.primary_key_column.clone();

                    // Validate change data before creating
                    let _validation_msg = if primary_key_value.is_none() {
                        "Warning: No primary key value found - change may not be executable"
                    } else if primary_key_column.is_none() {
                        "Warning: No primary key column detected - using first column"
                    } else {
                        "Change validation passed"
                    };

                    let change = TableChange::new(
                        ChangeType::UpdateCell,
                        tbl_name.clone(),
                        row,
                        Some(col),
                        Some(old_val.clone()),
                        Some(new_value.clone()),
                        primary_key_value,
                        primary_key_column,
                        None, // No insert_values for UpdateCell operations
                    );
                    delegate.edit_state.add_change(change);

                    // Log the change tracking (this will be visible when user commits)
                    // Note: We defer detailed logging to commit time to avoid cluttering the log
                }
            }

            // Stop editing and clear input
            delegate.edit_state.stop_editing();
            state.refresh(cx);
        });

        // Clear panel editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
        committed_value
    }

    pub fn cancel_cell_edit(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().cancel_cell_edit(row, col);
            // Stop editing and clear input
            state.delegate_mut().edit_state.stop_editing();
            state.refresh(cx);
        });

        // Clear panel editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
    }

    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        self.table_state
            .read(cx)
            .delegate()
            .edit_state
            .has_unsaved_changes()
    }

    pub fn get_table_name(&self, cx: &App) -> Option<String> {
        self.table_state
            .read(cx)
            .delegate()
            .get_table_name()
            .map(|s| s.to_string())
    }

    pub fn get_changes(&self, cx: &App) -> Vec<TableChange> {
        let table_read = self.table_state.read(cx);
        let _delegate = table_read.delegate();
        // Get changes directly from edited values in delegate
        let mut changes = Vec::new();
        let table_read = self.table_state.read(cx);
        let delegate = table_read.delegate();

        for ((row, col), new_value) in &delegate.edit_state.edited_values {
            if let Some(original_value) = delegate.edit_state.original_values.get(&(*row, *col)) {
                changes.push(TableChange::new(
                    ChangeType::UpdateCell,
                    delegate.table_name.clone().unwrap_or_default(),
                    *row,
                    Some(*col),
                    Some(original_value.clone()),
                    Some(new_value.clone()),
                    None, // primary_key_value
                    None, // primary_key_column
                    None, // No insert_values for UpdateCell operations
                ));
            }
        }

        tracing::info!(
            "Commit Changes: Got {} changes from edited_values",
            changes.len()
        );
        changes
    }

    pub fn clear_changes(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().edit_state.clear_changes();
            state.refresh(cx);
        });
        cx.notify();
    }

    pub fn commit_all_edits(&mut self, cx: &mut Context<Self>) -> Vec<(usize, usize, String)> {
        let mut committed_changes = Vec::new();

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            let edited_cells: Vec<(usize, usize)> =
                delegate.edit_state.edited_values.keys().cloned().collect();

            for (row, col) in edited_cells {
                if let Some(value) = delegate.commit_cell_edit(row, col) {
                    committed_changes.push((row, col, value));
                }
            }

            // Clear any current editing cell
            if let Some((row, col)) = delegate.edit_state.editing_cell {
                delegate.cancel_cell_edit(row, col);
            }

            state.refresh(cx);
        });

        cx.notify();
        committed_changes
    }

    pub fn cancel_all_edits(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();

            // Cancel current editing cell
            if let Some((row, col)) = delegate.edit_state.editing_cell {
                delegate.cancel_cell_edit(row, col);
            }

            // Clear all edited values
            delegate.edit_state.edited_values.clear();
            delegate.edit_state.original_values.clear();

            state.refresh(cx);
        });

        cx.notify();
    }

    pub fn get_current_editing_cell(&self, cx: &App) -> Option<(usize, usize)> {
        self.table_state.read(cx).delegate().edit_state.editing_cell
    }

    pub fn update_editing_cell_value(
        &mut self,
        row: usize,
        col: usize,
        new_value: String,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().update_cell_value(row, col, new_value);
            state.refresh(cx);
        });
        cx.notify();
    }

    /// Commit all pending changes to the database
    pub fn commit_changes_with_sql_log(
        &mut self,
        _window: &mut Window,
        sql_log: &Entity<blanco_ui::SqlLog>,
        cx: &mut Context<Self>,
    ) {
        self.commit_changes_internal(_window, Some(sql_log), cx)
    }

    #[allow(dead_code)]
    pub fn commit_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.commit_changes_internal(_window, None, cx)
    }

    fn commit_changes_internal(
        &mut self,
        _window: &mut Window,
        sql_log: Option<&Entity<blanco_ui::SqlLog>>,
        cx: &mut Context<Self>,
    ) {
        // First, commit any currently editing cell
        if let Some((row, col)) = self.get_current_editing_cell(cx) {
            // Get the current value from the input
            if let Some(input) = &self.editing_input {
                let current_value = input.read(cx).text().to_string();
                // Update the cell value before committing
                self.update_editing_cell_value(row, col, current_value.clone(), cx);

                // Commit the cell edit
                self.commit_cell_edit(row, col, current_value, cx);
            } else {
                self.cancel_current_edit(cx);
            }
        }

        // Get changes and convert to database-agnostic operations
        let change_operations = self
            .table_state
            .read(cx)
            .delegate()
            .create_change_operations();

        if change_operations.is_empty() {
            tracing::info!("Commit Changes: No changes to commit");
            return;
        }

        tracing::info!(
            "Commit Changes: Sending {} operations to async pipeline",
            change_operations.len()
        );

        let delegate = self.table_state.read(cx).delegate();

        // Get table name for logging
        let table_name = delegate
            .table_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Create clones for different uses
        let change_operations_for_pipeline = change_operations.clone();
        let change_operations_for_logging = change_operations.clone();
        let table_name_for_logging = table_name.clone();
        let _table_name_for_event = table_name.clone();
        let connection_id_for_pipeline = delegate.connection_id;
        let connection_id_for_event = delegate.connection_id;
        let database_name = delegate.database_name.clone();

        // Create response channel for table operations
        let (response_tx, response_rx) = smol::channel::bounded(1);

        // Emit a query execution started event
        cx.emit(AppEvent::QueryExecutionStarted {
            connection_id: Some(connection_id_for_event),
            query: format!(
                "Table operations on {} ({} operations)",
                table_name_for_logging,
                change_operations_for_logging.len()
            ),
        });

        tracing::info!("Commit Changes: Starting table operations execution");

        // Log the operations to SQL log if available
        if let Some(sql_log) = sql_log {
            for operation in &change_operations_for_logging {
                let sql_query =
                    (operation as &table_operations::TableChangeOperation).to_sql_query();
                sql_log.update(cx, |log, cx| {
                    log.append_text(&blanco_ui::SqlLogMessage::SqlStatement(sql_query), cx);
                    log.append_text(
                        &blanco_ui::SqlLogMessage::Comment("Executing table operation".to_string()),
                        cx,
                    );
                });
            }
        }

        // Spawn background task to execute table operations
        let db_service = DatabaseService::global(cx).clone();
        let _table_entity = self.table_state.clone();
        let _sql_log_entity: Option<Entity<blanco_ui::SqlLog>> = sql_log.cloned();

        cx.background_spawn(async move {
            let start_time = std::time::Instant::now();

            // Execute table operations using DatabaseService
            let result = match db_service
                .get_or_create_connection(connection_id_for_pipeline, Some(&database_name))
                .await
            {
                Ok(connection) => {
                    tracing::info!("Got connection for table operations");

                    // Convert table operations to SQL and execute them
                    let mut total_rows_affected = 0u64;
                    let mut operations_executed = 0;
                    let mut error_message = None;
                    let mut success = true;

                    for operation in &change_operations_for_pipeline {
                        tracing::debug!("Got operation {:?}", operation);
                        let sql_query =
                            (operation as &table_operations::TableChangeOperation).to_sql_query();
                        match connection
                            .execute_query(&sql_query, Some(&database_name), None)
                            .await
                        {
                            Ok(query_result) => {
                                total_rows_affected += query_result.rows_affected;
                                operations_executed += 1;
                                tracing::debug!("Successfully executed operation: {}", sql_query);
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to execute operation '{}': {}",
                                    sql_query,
                                    e
                                );
                                success = false;
                                error_message = Some(e.to_string());
                                break;
                            }
                        }
                    }

                    TableOperationResponse {
                        table_name: table_name_for_logging.clone(),
                        connection_id: connection_id_for_pipeline,
                        success,
                        rows_affected: Some(total_rows_affected),
                        error_message,
                        operations_executed,
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to get connection for table operations: {}", e);
                    TableOperationResponse {
                        table_name: table_name_for_logging.clone(),
                        connection_id: connection_id_for_pipeline,
                        success: false,
                        rows_affected: None,
                        error_message: Some(format!("Connection error: {}", e)),
                        operations_executed: 0,
                    }
                }
            };

            tracing::info!(
                "Table operations completed in {:?}, success: {}",
                start_time.elapsed(),
                result.success
            );

            // Send response back through the channel
            if let Err(e) = response_tx.send(result.clone()).await {
                tracing::error!("Failed to send table operation response: {}", e);
            }

            result
        })
        .detach();

        // Spawn async task to handle the response
        let sql_log_response_entity: Option<Entity<blanco_ui::SqlLog>> = sql_log.cloned();
        cx.spawn(async move |entity, cx| {
            match response_rx.recv().await {
                Ok(response) => {
                    tracing::info!("Received table operation response: success={}, rows_affected={:?}",
                        response.success, response.rows_affected);

                    // Handle successful operations
                    if response.success {
                        // Clear edits and refresh the table
                        let _ = entity.update(cx, |panel, cx| {
                            panel.table_state.update(cx, |state, cx| {
                                state.delegate_mut().edit_state.clear_edits();
                                state.refresh(cx);
                            });

                            // Update SQL log with success message
                            if let Some(sql_log) = sql_log_response_entity {
                                sql_log.update(cx, |log, cx| {
                                    let success_msg = format!(
                                        "✓ Table operations completed successfully\n-- {} operations executed, {} rows affected",
                                        response.operations_executed,
                                        response.rows_affected.unwrap_or(0)
                                    );
                                    log.append_text(&blanco_ui::SqlLogMessage::Comment(success_msg), cx);
                                });
                            }
                        });

                        // Emit table operation completed event
                        let _ = entity.update(cx, |_, cx| {
                            cx.emit(AppEvent::TableOperationCompleted {
                                table_name: response.table_name,
                                connection_id: response.connection_id,
                                success: true,
                                rows_affected: response.rows_affected,
                                error_message: None,
                                operations_executed: response.operations_executed,
                            });
                        });
                    } else {
                        // Handle failed operations - show error but keep edits for retry
                        if let Some(sql_log) = sql_log_response_entity {
                            let error_message_clone = response.error_message.clone();
                            let _ = sql_log.update(cx, |log, cx| {
                                let error_msg = format!(
                                    "✗ Table operations failed: {}",
                                    error_message_clone.unwrap_or_else(|| "Unknown error".to_string())
                                );
                                log.append_text(&blanco_ui::SqlLogMessage::Comment(error_msg), cx);
                            });
                        }

                        // Emit table operation completed event with failure
                        let _ = entity.update(cx, |_, cx| {
                            cx.emit(AppEvent::TableOperationCompleted {
                                table_name: response.table_name,
                                connection_id: response.connection_id,
                                success: false,
                                rows_affected: None,
                                error_message: response.error_message,
                                operations_executed: response.operations_executed,
                            });
                        });
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to receive table operation response: {}", e);

                    // Update SQL log with error
                    if let Some(sql_log) = sql_log_response_entity {
                        let _ = sql_log.update(cx, |log, cx| {
                            let error_msg = format!("✗ Failed to get operation response: {}", e);
                            log.append_text(&blanco_ui::SqlLogMessage::Comment(error_msg), cx);
                        });
                    }
                }
            }
        }).detach();
    }

    /// Rollback all pending changes
    pub fn rollback_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let changes = self.get_changes(cx);

        if changes.is_empty() {
            return;
        }

        // Get table name and connection for events
        let table_name = self
            .table_state
            .read(cx)
            .delegate()
            .table_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let connection_id = self.table_state.read(cx).delegate().connection_id;

        // TODO: error here instead of fallback to sqlite::memory

        tracing::info!(
            "Rollback Changes: Rolling back {} changes on table {}",
            changes.len(),
            table_name
        );

        // Restore all original values from changes
        for change in &changes {
            match change.change_type {
                ChangeType::UpdateCell => {
                    if let Some(col) = change.column_index {
                        let row = change.row_index;
                        if let Some(old_value) = &change.old_value {
                            self.table_state.update(cx, |state, _cx| {
                                if let Some(cell) = state.delegate_mut().get_cell_mut(row, col) {
                                    *cell = old_value.clone();
                                }
                            });
                        }
                    }
                }
                ChangeType::InsertRow => {
                    // Remove inserted rows (reverse order to maintain indices)
                    let row = change.row_index;
                    self.table_state.update(cx, |state, _cx| {
                        state.delegate_mut().remove_row(row);
                    });
                }
            }
        }

        // Clear all changes
        self.clear_changes(cx);

        // Emit rollback event
        let changes_count = changes.len();
        cx.emit(AppEvent::TableChangesRollback {
            table_name: table_name.clone(),
            connection_id,
            changes_count,
        });

        tracing::info!(
            "Rollback Changes: Successfully rolled back {} changes",
            changes_count
        );
        cx.notify();
    }

    pub fn add_new_row(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            let column_count = delegate.columns.len();

            // Create a new row with empty values
            let new_row: Vec<String> = (0..column_count).map(|_| "".to_string()).collect();
            delegate.rows.push(new_row);

            // Mark this as a pending new row
            let new_row_index = delegate.rows.len() - 1;
            delegate.edit_state.pending_new_rows.push(new_row_index);

            // Track the INSERT change with NULL values (excluding row number and primary key columns)
            if let Some(table_name) = &delegate.table_name {
                // For new rows, exclude primary key to avoid UPDATE/INSERT confusion
                let column_names = delegate.get_insert_column_names(true); // exclude_primary_key = true
                let values_vec = column_names
                    .iter()
                    .map(|_| "NULL".to_string())
                    .collect::<Vec<_>>();

                let change = TableChange::new(
                    ChangeType::InsertRow,
                    table_name.clone(),
                    new_row_index,
                    None,
                    None,
                    None, // No single new_value for insert operations
                    None, // No primary key value for new rows
                    delegate.primary_key_column.clone(),
                    Some(values_vec), // Use insert_values parameter instead
                );
                delegate.edit_state.add_change(change);
            }

            state.refresh(cx);
        });
        cx.notify();
    }

    pub fn duplicate_row(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            let selected_rows = state.selected_rows().clone();

            // Check if the row exists
            for row_ix in selected_rows {
                let delegate = state.delegate_mut();
                if let Some(row_to_duplicate) = delegate.rows.get(row_ix).cloned() {
                    // Add the duplicated row
                    delegate.rows.push(row_to_duplicate.clone());

                    // Mark this as a pending new row
                    let new_row_index = delegate.rows.len() - 1;
                    delegate.edit_state.pending_new_rows.push(new_row_index);

                    // Track the INSERT change with proper column values (excluding row number and primary key columns)
                    if let Some(table_name) = &delegate.table_name {
                        // For new rows (duplicated rows), exclude primary key to avoid UPDATE/INSERT confusion
                        let _column_names = delegate.get_insert_column_names(true); // exclude_primary_key = true
                        let values = delegate.get_insert_values(new_row_index, true); // exclude_primary_key = true
                        let values_vec = values
                            .iter()
                            .map(|val| {
                                if val.is_empty() || val == "NULL" {
                                    "NULL".to_string()
                                } else {
                                    val.to_string() // Keep raw value without SQL formatting
                                }
                            })
                            .collect::<Vec<_>>();

                        let change = TableChange::new(
                            ChangeType::InsertRow,
                            table_name.clone(),
                            new_row_index,
                            None,
                            None,
                            None, // No single new_value for insert operations
                            None, // No primary key value for new rows
                            delegate.primary_key_column.clone(),
                            Some(values_vec.clone()), // Use insert_values parameter instead
                        );
                        delegate.edit_state.add_change(change);
                    }

                    state.refresh(cx);
                }
            }
        });
        cx.notify();
    }

    /// Handle table operation completion event
    #[allow(dead_code)]
    pub fn handle_table_operation_completed(
        &mut self,
        _table_name: &str,
        success: bool,
        rows_affected: Option<u64>,
        error_message: Option<String>,
        operations_executed: usize,
        cx: &mut Context<Self>,
    ) {
        if success {
            tracing::info!(
                "Table operation completed successfully: {} operations, {} rows affected",
                operations_executed,
                rows_affected.unwrap_or(0)
            );

            // Clear edit state after successful commit
            self.table_state.update(cx, |state, cx| {
                state.delegate_mut().edit_state.clear_edits();
                state.refresh(cx);
            });

            // Optionally refresh the data or show a success message
            cx.notify();
        } else {
            tracing::error!(
                "Table operation failed: {}",
                error_message.unwrap_or_else(|| "Unknown error".to_string())
            );

            // Keep the edit state so user can retry or fix issues
            // Don't refresh the table to preserve user's changes
        }
    }

    // Copy and selection action handlers

    fn on_copy_as_csv(
        &mut self,
        _action: &crate::app::CopyAsCSV,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as CSV: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        if let Err(e) = self.copy_handler.copy_as_format(&selected_data, "csv", cx) {
            tracing::error!("Failed to copy as CSV: {}", e);
        }
    }

    fn on_copy_as_json(
        &mut self,
        _action: &crate::app::CopyAsJSON,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as JSON: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        if let Err(e) = self.copy_handler.copy_as_format(&selected_data, "json", cx) {
            tracing::error!("Failed to copy as JSON: {}", e);
        }
    }

    fn on_copy_as_sql(
        &mut self,
        _action: &crate::app::CopyAsSQL,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as SQL: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        if let Err(e) = self.copy_handler.copy_as_format(&selected_data, "sql", cx) {
            tracing::error!("Failed to copy as SQL: {}", e);
        }
    }

    fn on_copy_as_markdown(
        &mut self,
        _action: &crate::app::CopyAsMarkdown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as Markdown: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        if let Err(e) = self
            .copy_handler
            .copy_as_format(&selected_data, "markdown", cx)
        {
            tracing::error!("Failed to copy as Markdown: {}", e);
        }
    }

    fn on_add_row(&mut self, _action: &AddRow, _window: &mut Window, cx: &mut Context<Self>) {
        self.add_new_row(cx);
    }

    fn on_duplicate_row(
        &mut self,
        _action: &DuplicateRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.duplicate_row(cx);
    }

    pub fn get_selected_data_for_rows(
        &self,
        selected_rows: &HashSet<usize>,
        delegate: &ResultsTableDelegate,
    ) -> SelectedTableData {
        let mut selected_rows_data = Vec::new();

        // Collect selected row data
        for &row in selected_rows {
            if let Some(row_data) = delegate.rows.get(row) {
                let cells: Vec<SelectedCell> = row_data
                    .iter()
                    .enumerate()
                    .filter(|&(col, _)| col > 0) // Skip row number column
                    .map(|(col, value)| SelectedCell {
                        row,
                        col: col - 1, // Adjust for row number column
                        value: value.clone(),
                        column_name: delegate.columns.get(col).map(|c| c.name.to_string()),
                        column_type: delegate.column_types.get(col - 1).cloned(), // Adjust for row number column
                    })
                    .collect();

                selected_rows_data.push(SelectedRow {
                    row,
                    cells,
                    primary_key_value: None, // TODO: Extract primary key if needed
                });
            }
        }

        SelectedTableData {
            table_name: delegate.table_name.clone(),
            columns: delegate
                .columns
                .iter()
                .skip(1)
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>(),
            column_types: delegate.column_types.clone(),
            selected_rows: selected_rows_data,
            primary_key_column: delegate.primary_key_column.clone(),
        }
    }
}

impl EventEmitter<AppEvent> for ResultsPanel {}
impl Focusable for ResultsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ResultsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check for pending inline edits
        if let Some((row, col)) = self.table_state.read(cx).delegate().pending_edit_cell {
            // Clear the pending edit and start editing
            self.table_state.update(cx, |state, _cx| {
                state.delegate_mut().pending_edit_cell = None;
            });

            self.start_cell_edit(row, col, window, cx);
        }

        let _row_count = self.table_state.read(cx).delegate().rows_count(cx);
        let _has_unsaved_changes = self.has_unsaved_changes(cx);
        let _table_name = self.get_table_name(cx);
        let _is_editable = self.table_state.read(cx).delegate().is_editable();

        v_flex()
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            // Handle copy and selection actions
            .on_action(cx.listener(Self::on_copy_as_csv))
            .on_action(cx.listener(Self::on_copy_as_json))
            .on_action(cx.listener(Self::on_copy_as_sql))
            .on_action(cx.listener(Self::on_copy_as_markdown))
            .on_action(cx.listener(Self::on_add_row))
            .on_action(cx.listener(Self::on_duplicate_row))
            // The table component (table should have built-in scrolling)
            .child(
                div()
                    .id("results-table")
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .flex_1() // Allow table to fill available space
                    .overflow_hidden()
                    .min_h(px(200.0)) // Minimum height for table
                    .child(Table::new(&self.table_state).bordered(false)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_edit_state() {
        let mut edit_state = CellEditState {
            editing_cell: Some((0, 0)),
            ..Default::default()
        };

        // Test initial state
        assert!(edit_state.is_editing(0, 0));
        assert!(!edit_state.is_edited(0, 0));
        assert!(!edit_state.has_unsaved_changes());

        // Test starting editing with string value
        edit_state
            .original_values
            .insert((0, 0), "original".to_string());

        assert!(edit_state.is_editing(0, 0));
        assert!(!edit_state.is_edited(0, 0));

        // Test updating value
        edit_state
            .edited_values
            .insert((0, 0), "modified".to_string());
        assert!(edit_state.is_edited(0, 0));
        assert_eq!(
            edit_state.get_edited_value(0, 0),
            Some(&"modified".to_string())
        );
        assert_eq!(
            edit_state.get_original_value(0, 0),
            Some(&"original".to_string())
        );

        // Test clearing editing state
        edit_state.editing_cell = None;
        assert!(!edit_state.is_editing(0, 0));
        assert!(edit_state.has_unsaved_changes());
    }

    #[test]
    fn test_results_table_delegate_default() {
        let delegate = ResultsTableDelegate::default();

        // Test basic properties without requiring App context
        assert!(delegate.table_name.is_none());
        assert!(delegate.primary_key_column.is_none());
        assert!(delegate.columns.is_empty());
        assert!(delegate.rows.is_empty());
    }

    #[test]
    fn test_cell_edit_state_clear_all() {
        let mut edit_state = CellEditState {
            editing_cell: Some((0, 0)),
            ..Default::default()
        };

        // Add some data
        edit_state
            .original_values
            .insert((0, 0), "original".to_string());
        edit_state
            .edited_values
            .insert((0, 0), "modified".to_string());
        edit_state.pending_new_rows.push(0);

        // Clear all
        edit_state.clear_all();

        assert!(!edit_state.is_editing(0, 0));
        assert!(!edit_state.is_edited(0, 0));
        assert!(!edit_state.has_unsaved_changes());
        assert!(edit_state.pending_new_rows.is_empty());
    }

    #[test]
    fn test_primary_key_update_preserves_original_value() {
        let mut delegate = ResultsTableDelegate::default();

        // Set up test data with primary key as first column
        delegate.table_name = Some("test_table".to_string());
        delegate.primary_key_column = Some("id".to_string());

        // Create columns: row_number, id, name
        delegate.columns = vec![
            Column::new("row_number".to_string(), "#".to_string()),
            Column::new("id".to_string(), "id".to_string()),
            Column::new("name".to_string(), "name".to_string()),
        ];

        // Add a row with id=2
        delegate.rows = vec![vec!["1".to_string(), "2".to_string(), "test".to_string()]];

        // Simulate editing the primary key column (id) from 2 to 4
        let row = 0;
        let col = 1; // id column

        // Store original value
        delegate
            .edit_state
            .original_values
            .insert((row, col), "2".to_string());
        delegate
            .edit_state
            .edited_values
            .insert((row, col), "4".to_string());

        // Commit the edit
        delegate.commit_cell_edit(row, col);

        // Check that the change was created with the correct primary key value
        assert_eq!(delegate.edit_state.changes.len(), 1);
        let change = &delegate.edit_state.changes[0];

        // The primary key value should be the original value (2), not the new value (4)
        assert_eq!(change.primary_key_value, Some("2".to_string()));
        assert_eq!(change.old_value, Some("2".to_string()));
        assert_eq!(change.new_value, Some("4".to_string()));

        // Verify the SQL generation would use the correct WHERE clause
        let operations = delegate.create_change_operations();
        assert_eq!(operations.len(), 1);

        if let OperationType::Update = &operations[0].operation_type {
            if let RowIdentifier::PrimaryKey { value, .. } = &operations[0].row_identifier {
                assert_eq!(value, "2"); // Should use original ID in WHERE clause
            } else {
                panic!("Expected PrimaryKey row identifier");
            }
        } else {
            panic!("Expected Update operation");
        }
    }
}
