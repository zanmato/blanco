use std::collections::{HashMap, HashSet};
use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, Styled, Window,
};
use gpui_component::{
    input::{Input, InputEvent, InputState},
    legacytable::{Column, ColumnSort, Table, TableDelegate},
    menu::PopupMenu,
    v_flex, ActiveTheme, Icon, IconName,
};

use crate::app::{ClearSelection, CopyCell, SelectRow}; // Import the action types
use crate::app_events::AppEvent;
use crate::db_service::DbService;
use crate::transformers::CopyHandler;

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
use blanco_core::table_operations::TableChangeOperation;
use blanco_core::ColumnChange;
use blanco_core::QueryResult;

// Data structures for copy functionality
#[derive(Clone, Debug)]
pub struct SelectedCell {
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
    pub selected_cells: Vec<SelectedCell>,
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChangeType {
    UpdateCell,
    InsertRow,
}

#[derive(Clone, Debug, Default)]
pub struct CellEditState {
    pub editing_cell: Option<(usize, usize)>, // (row, col)
    pub original_values: HashMap<(usize, usize), String>,
    pub edited_values: HashMap<(usize, usize), String>,
    pub pending_new_rows: Vec<usize>, // Track rows that are newly added
    pub editing_input: Option<Entity<InputState>>, // Store input state per delegate
    pub changes: Vec<TableChange>,    // Track all changes for SQL generation
    pub selected_cells: HashSet<(usize, usize)>, // Track selected cells
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
    }

    pub fn clear_all(&mut self) {
        self.editing_cell = None;
        self.original_values.clear();
        self.edited_values.clear();
        self.pending_new_rows.clear();
        self.editing_input = None;
        self.changes.clear();
        self.selected_cells.clear();
        self.selected_rows.clear();
        self.current_column = 1; // Start with first data column
    }

    pub fn add_change(&mut self, change: TableChange) {
        self.changes.push(change);
    }

    pub fn clear_changes(&mut self) {
        self.changes.clear();
        self.edited_values.clear();
        self.original_values.clear();
        self.pending_new_rows.clear();
    }

    pub fn select_cell(&mut self, row: usize, col: usize) -> bool {
        // Toggle cell selection (col 0 is row number column)
        if col == 0 {
            return false;
        }
        if self.selected_cells.contains(&(row, col)) {
            self.selected_cells.remove(&(row, col));
            false
        } else {
            self.selected_cells.insert((row, col));
            true
        }
    }

    pub fn select_row(&mut self, row: usize) -> bool {
        // Toggle row selection
        if self.selected_rows.contains(&row) {
            self.selected_rows.remove(&row);
            false
        } else {
            self.selected_rows.insert(row);
            true
        }
    }

    pub fn is_cell_selected(&self, row: usize, col: usize) -> bool {
        self.selected_cells.contains(&(row, col)) || self.selected_rows.contains(&row)
    }

    pub fn clear_selection(&mut self) {
        self.selected_cells.clear();
        self.selected_rows.clear();
    }

    pub fn has_selection(&self) -> bool {
        !self.selected_cells.is_empty() || !self.selected_rows.is_empty()
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
    ) -> Self {
        TableChangeBuilder::new(change_type, table_name, row_index)
            .column_index(column_index)
            .old_value(old_value)
            .new_value(new_value)
            .primary_key_value(primary_key_value)
            .primary_key_column(primary_key_column)
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
                // For INSERT, we need column names from the table structure
                // This is a placeholder - actual implementation will need column info
                self.sql_template = Some(format!("INSERT INTO {} VALUES ($1)", self.table_name));

                if let Some(new_val) = &self.new_value {
                    self.parameters.push(new_val.clone());
                } else {
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
    connection_id: Option<i64>,
}

impl ResultsTableDelegate {
    /// Remove a row at the specified index
    pub fn remove_row(&mut self, row_index: usize) {
        if row_index < self.rows.len() {
            self.rows.remove(row_index);
        }
    }

    /// Get a mutable reference to a cell
    pub fn get_cell_mut(&mut self, row: usize, col: usize) -> Option<&mut String> {
        self.rows
            .get_mut(row)
            .and_then(|row_data| row_data.get_mut(col))
    }

    /// Set the connection ID for database operations
    pub fn set_connection_id(&mut self, connection_id: i64) {
        self.connection_id = Some(connection_id);
    }

    /// Convert table changes to database-agnostic TableChangeOperations
    pub fn create_change_operations(
        &self,
    ) -> Vec<blanco_core::table_operations::TableChangeOperation> {
        let mut operations = Vec::new();

        for change in &self.edit_state.changes {
            let operation = match change.change_type {
                ChangeType::UpdateCell => {
                    // Get column name from index
                    let column_name = self
                        .columns
                        .get(change.column_index.unwrap_or(0))
                        .map(|col| col.name.to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    // Use the primary key information stored in the change
                    if let (Some(pk_column), Some(pk_value)) =
                        (&change.primary_key_column, &change.primary_key_value)
                    {
                        TableChangeOperation::update_cell(
                            change.table_name.clone(),
                            pk_column.clone(),
                            pk_value.clone(),
                            column_name,
                            change.old_value.clone(),
                            change.new_value.clone(),
                        )
                    } else {
                        // Fallback: try to get primary key from delegate
                        if let Some(ref pk_column) = self.primary_key_column {
                            if let Some(pk_value) =
                                self.rows.get(change.row_index).and_then(|row| row.first())
                            {
                                // Assume PK is first column as fallback
                                TableChangeOperation::update_cell(
                                    change.table_name.clone(),
                                    pk_column.clone(),
                                    pk_value.clone(),
                                    column_name,
                                    change.old_value.clone(),
                                    change.new_value.clone(),
                                )
                            } else {
                                continue; // Skip this change if we can't determine PK
                            }
                        } else {
                            continue; // Skip this change if we can't determine PK
                        }
                    }
                }
                ChangeType::InsertRow => {
                    // Convert the new_value (comma-separated) into column changes
                    if let Some(ref values_str) = change.new_value {
                        let values: Vec<String> =
                            values_str.split(", ").map(|s| s.to_string()).collect();

                        let column_changes: Vec<ColumnChange> = self
                            .columns
                            .iter()
                            .zip(values.iter())
                            .map(|(col, value)| ColumnChange {
                                column_name: col.name.to_string(),
                                old_value: None,
                                new_value: Some(value.clone()),
                            })
                            .collect();

                        TableChangeOperation::insert_row(change.table_name.clone(), column_changes)
                    } else {
                        continue; // Skip if no values
                    }
                }
            };

            operations.push(operation);
        }

        operations
    }

    pub fn set_query_result(&mut self, result: QueryResult) {
        // Clear previous edit state
        self.edit_state.clear_all();
        self.pending_edit_cell = None;

        // Store column types
        self.column_types = result.column_types.clone();

        // Calculate column widths based on content and type
        let mut column_widths: Vec<f32> = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col_name)| {
                let mut max_width = col_name.len() as f32 * 7.0; // Smaller font size = smaller multiplier

                // Check some sample rows to determine content width
                for row in result.rows.iter().take(20) {
                    // Sample more rows for better accuracy
                    if let Some(cell_value) = row.get(i) {
                        let content_width = cell_value.len() as f32 * 7.0; // Adjusted for smaller font
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
        let mut columns = vec![Column::new("row_number".to_string(), "#".to_string())
            .width(row_num_width)
            .resizable(false)];

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
                    log::info!("Table name changed from '{}' to '{}'", old, new);
                } else {
                    log::debug!("Table name unchanged: '{}'", new);
                }
            }
            (None, Some(new)) => {
                log::info!("Table name extracted from metadata: '{}'", new);
            }
            (Some(old), None) => {
                log::warn!(
                    "Table name lost: '{}' (was extracted before, now None)",
                    old
                );
            }
            (None, None) => {
                if result.query_text.is_some() {
                    log::warn!("Failed to extract table name from query metadata");
                }
            }
        }

        // Use primary key from QueryResult if available, otherwise fall back to heuristic
        let old_pk = self.primary_key_column.clone();
        self.primary_key_column = result.primary_key_column.clone().or_else(|| {
            self.table_name
                .as_ref()
                .and_then(|table_name| self.detect_primary_key_simple(table_name))
        });

        // Debug: Log table setup details
        if let Some(table_name) = &self.table_name {
            log::info!(
                "Table setup complete - name: '{}', pk_column: {:?}, columns: {}, rows: {}",
                table_name,
                self.primary_key_column,
                self.columns.len(),
                self.rows.len()
            );

            // Log primary key detection results
            match (&old_pk, &self.primary_key_column) {
                (Some(old), Some(new)) => {
                    if old != new {
                        log::info!("Primary key changed from '{}' to '{}'", old, new);
                    } else {
                        log::debug!("Primary key unchanged: '{}'", new);
                    }
                }
                (None, Some(new)) => {
                    if result.primary_key_column.is_some() {
                        log::info!("Primary key from metadata: '{}'", new);
                    } else {
                        log::info!("Primary key detected by heuristic: '{}'", new);
                    }
                }
                (Some(old), None) => {
                    log::info!("Primary key cleared: '{}' (was Some before)", old);
                }
                (None, None) => {
                    log::debug!("Primary key remains None");
                }
            }
        }

        // Additional debug: Log column names for primary key detection
        if let Some(table_name) = &self.table_name {
            let column_names: Vec<String> = self
                .columns
                .iter()
                .map(|col| col.name.to_string())
                .collect();
            log::debug!(
                "Available columns for table '{}': {:?}",
                table_name,
                column_names
            );
        } else {
            log::warn!("No table name could be extracted - table will not be editable");
        }
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

    pub fn update_cell_value(&mut self, row: usize, col: usize, new_value: String) {
        self.edit_state.edited_values.insert((row, col), new_value);
    }

    pub fn commit_cell_edit(&mut self, row: usize, col: usize) -> Option<String> {
        log::info!("delegate.commit_cell_edit called for ({}, {})", row, col);
        log::info!(
            "edited_values contains: {:?}",
            self.edit_state.edited_values
        );

        if let Some(new_value) = self.edit_state.edited_values.get(&(row, col)).cloned() {
            log::info!("Found edited value: '{}' for ({}, {})", new_value, row, col);

            // Get the original value
            let original_value = self.edit_state.original_values.get(&(row, col)).cloned();

            // Update the actual row data
            if let Some(row_data) = self.rows.get_mut(row) {
                if let Some(cell) = row_data.get_mut(col) {
                    log::info!("Updating cell from '{}' to '{}'", cell, new_value);
                    *cell = new_value.clone();
                    log::info!("Cell updated successfully");
                } else {
                    log::info!("No cell found at column {}", col);
                }
            } else {
                log::info!("No row data found at row {}", row);
            }

            // Track the change for SQL generation
            if let (Some(original), Some(table_name)) = (&original_value, &self.table_name) {
                // Get primary key value using the detected primary key column
                let primary_key_value = self.get_primary_key_value(row);

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
                );

                self.edit_state.add_change(change);
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

    pub fn get_primary_key_value(&self, row: usize) -> Option<String> {
        // Get the primary key value using the detected primary key column
        if let Some(pk_column_name) = &self.primary_key_column {
            // Find the index of the primary key column
            if let Some(pk_index) = self
                .columns
                .iter()
                .position(|col| col.name.as_str() == pk_column_name)
            {
                return self.rows.get(row).and_then(|r| r.get(pk_index).cloned());
            }
        }
        // Fallback to first column if no primary key column is detected
        self.rows.get(row).and_then(|r| r.first().cloned())
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
            let is_array = type_lower == "array" || type_lower.ends_with("[]");
            log::debug!(
                "Array detection: type='{}', lower='{}', is_array={}",
                column_type,
                type_lower,
                is_array
            );
            is_array
        } else {
            log::debug!("Array detection: No column type for index {}", col_index);
            false
        }
    }

    pub fn get_cell_value(&self, row: usize, col: usize) -> Option<String> {
        self.rows.get(row).and_then(|r| r.get(col)).cloned()
    }

    pub fn is_editable(&self) -> bool {
        self.table_name.is_some() && self.primary_key_column.is_some()
    }

    pub fn get_selected_data(&self) -> SelectedTableData {
        let mut selected_cells = Vec::new();
        let mut selected_rows = Vec::new();

        // Collect selected cell data
        for &(row, col) in &self.edit_state.selected_cells {
            if let Some(cell_value) = self.get_cell_value(row, col) {
                selected_cells.push(SelectedCell {
                    row,
                    col: col - 1, // Adjust for row number column
                    value: cell_value,
                    column_name: self.columns.get(col).map(|c| c.name.to_string()),
                    column_type: self.column_types.get(col - 1).cloned(), // Adjust for row number column
                });
            }
        }

        // Collect selected row data
        for &row in &self.edit_state.selected_rows {
            if let Some(row_data) = self.rows.get(row) {
                let cells: Vec<SelectedCell> = row_data
                    .iter()
                    .enumerate()
                    .filter(|&(col, _)| col > 0) // Skip row number column
                    .map(|(col, value)| SelectedCell {
                        row,
                        col: col - 1, // Adjust for row number column
                        value: value.clone(),
                        column_name: self.columns.get(col).map(|c| c.name.to_string()),
                        column_type: self.column_types.get(col - 1).cloned(), // Adjust for row number column
                    })
                    .collect();

                selected_rows.push(SelectedRow {
                    row,
                    cells,
                    primary_key_value: None, // TODO: Extract primary key if needed
                });
            }
        }

        SelectedTableData {
            table_name: self.table_name.clone(),
            columns: self
                .columns
                .iter()
                .skip(1)
                .map(|c| c.name.to_string())
                .collect(), // Skip row number column
            column_types: self.column_types.clone(),
            selected_cells,
            selected_rows,
            primary_key_column: None, // TODO: Extract primary key if needed
        }
    }

    // Wrapper methods for selection functionality
    pub fn select_row(&mut self, row: usize) {
        self.edit_state.select_row(row);
    }

    pub fn select_cell(&mut self, row: usize, col: usize) {
        self.edit_state.select_cell(row, col);
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

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_th(
        &self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        let col = &self.columns[col_ix];
        div()
            .font_family("Fira Code")
            .text_sm()
            .child(col.name.to_string())
    }

    fn render_td(
        &self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        let is_row_number_col = col_ix == 0;
        let is_editing = self.edit_state.is_editing(row_ix, col_ix) && !is_row_number_col;
        let is_edited = self.edit_state.is_edited(row_ix, col_ix) && !is_row_number_col;
        let is_editable = self.is_editable() && !is_row_number_col;
        let is_selected = self.edit_state.is_cell_selected(row_ix, col_ix);

        let current_value = if is_edited {
            let edited_val = self.edit_state.get_edited_value(row_ix, col_ix).cloned();
            edited_val
        } else {
            let original_val = self
                .rows
                .get(row_ix)
                .and_then(|row| row.get(col_ix))
                .cloned();
            original_val
        }
        .unwrap_or_else(|| "--".to_string());

        // Check if the value is NULL
        let is_null = current_value.eq_ignore_ascii_case("null") || current_value == "--";
        let display_text = if is_null {
            "NULL".to_string()
        } else {
            current_value.clone()
        };

        if is_editing {
            // Embed Input directly in the cell (not for row number column)
            if let Some(input) = self.edit_state.get_editing_input() {
                div()
                    .font_family("Fira Code")
                    .text_xs()
                    .size_full()
                    .flex() // Enable flexbox layout
                    .items_center() // Center vertically
                    .p_0() // No padding since the cell already has padding
                    .when(self.is_numeric_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.justify_end() // Right-align numeric columns
                    })
                    .when(self.is_uuid_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for UUIDs
                            .text_color(cx.theme().blue) // Blue color for UUIDs
                    })
                    .when(self.is_timestamp_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for timestamps
                            .text_color(cx.theme().green) // Green color for timestamps
                    })
                    .when(self.is_json_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for JSON
                            .text_color(cx.theme().yellow) // Yellow color for JSON
                    })
                    .when(self.is_array_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for arrays
                            .text_color(cx.theme().blue) // Blue color for arrays
                    })
                    .child(
                        Input::new(&input)
                            .size_full()
                            .text_size(px(12.))
                            .border_0() // No border on the input
                            .px_0() // No horizontal padding
                            .py_0(), // No vertical padding
                    )
            } else {
                // Fallback if input is not available
                div()
                    .font_family("Fira Code")
                    .text_size(px(12.))
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().blue)
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .size_full()
                    .flex() // Enable flexbox layout
                    .items_center() // Center vertically
                    .when(self.is_numeric_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.justify_end() // Right-align numeric columns
                    })
                    .when(self.is_uuid_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for UUIDs
                            .text_color(cx.theme().blue) // Blue color for UUIDs
                    })
                    .when(self.is_timestamp_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for timestamps
                            .text_color(cx.theme().green) // Green color for timestamps
                    })
                    .when(self.is_json_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for JSON
                            .text_color(cx.theme().yellow) // Yellow color for JSON
                    })
                    .when(self.is_array_column(col_ix - 1), |this| {
                        // Adjust for row number column
                        this.font_family("Fira Code") // Monospace font for arrays
                            .text_color(cx.theme().blue) // Blue color for arrays
                    })
                    .child(display_text)
            }
        } else {
            // Check if this is a numeric column for right-alignment (adjust for row number column)
            let is_numeric = !is_row_number_col && self.is_numeric_column(col_ix - 1);

            // Render static cell with appropriate handlers
            div()
                .font_family("Fira Code")
                .text_size(px(12.))
                .size_full() // Fill the entire cell container
                .flex() // Enable flexbox layout
                .items_center() // Center vertically
                .when(is_row_number_col, |this| {
                    this.font_weight(FontWeight::BOLD) // Bold row numbers
                        .text_color(cx.theme().muted_foreground) // Muted color for row numbers
                        .cursor_pointer() // Pointer cursor for row selection
                })
                .when(is_numeric && !is_row_number_col, |this| {
                    this.justify_end() // Right-align numeric columns
                        .text_color(cx.theme().foreground) // Ensure numeric text is visible
                })
                .when(
                    !is_row_number_col && self.is_uuid_column(col_ix - 1),
                    |this| {
                        this.font_family("Fira Code") // Monospace font for UUIDs
                            .text_color(cx.theme().blue) // Blue color for UUIDs
                    },
                )
                .when(
                    !is_row_number_col && self.is_timestamp_column(col_ix - 1),
                    |this| {
                        this.font_family("Fira Code") // Monospace font for timestamps
                            .text_color(cx.theme().green) // Green color for timestamps
                    },
                )
                .when(
                    !is_row_number_col && self.is_json_column(col_ix - 1),
                    |this| {
                        this.font_family("Fira Code") // Monospace font for JSON
                            .text_color(cx.theme().yellow) // Yellow color for JSON
                    },
                )
                .when(is_edited, |this| {
                    this.bg(cx.theme().yellow.opacity(0.1))
                        .border_l_2()
                        .border_color(cx.theme().yellow)
                })
                .when(is_selected, |this| {
                    this.bg(cx.theme().blue.opacity(0.2))
                        .border_1()
                        .border_color(cx.theme().blue)
                })
                .when(is_null, |this| {
                    this.text_color(cx.theme().muted_foreground).italic()
                })
                // All data cells (non-row-number) should be selectable for copying
                .when(!is_row_number_col, |this| {
                    this.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |table, event: &gpui::MouseDownEvent, _window, cx| {
                            let delegate = table.delegate_mut();

                            if event.click_count == 1 {
                                // Single click - select cell
                                log::info!(
                                    "Single click on cell {}:{} - selecting cell",
                                    row_ix,
                                    col_ix
                                );
                                delegate.clear_selection();
                                delegate.select_cell(row_ix, col_ix);
                                table.refresh(cx);
                                cx.notify();
                            } else if event.click_count == 2 {
                                // Double click - start editing (only if editable)
                                log::info!("Double click on cell {}:{}", row_ix, col_ix);

                                if delegate.is_editable() {
                                    // Only set pending edit if not already editing this cell
                                    if delegate.edit_state.editing_cell != Some((row_ix, col_ix)) {
                                        delegate.pending_edit_cell = Some((row_ix, col_ix));
                                        table.refresh(cx);
                                        cx.notify();
                                    }
                                }
                            }
                        }),
                    )
                })
                // Only show visual feedback for editable cells when hovering
                .when(!is_row_number_col && !is_null && is_editable, |this| {
                    this.cursor_pointer()
                })
                .when(!is_editable && !is_row_number_col, |this| {
                    this.text_color(cx.theme().muted_foreground.opacity(0.6))
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
        _: &mut Context<Table<Self>>,
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
        _: &mut Context<Table<Self>>,
    ) {
    }

    fn visible_columns_changed(
        &mut self,
        _visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) {
    }

    fn render_last_empty_col(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        // Add extra space to ensure all columns are scrollable
        // This compensates for any viewport calculation issues
        div().w(px(30.0)).h_full().flex_shrink_0()
    }

    fn context_menu(
        &self,
        row_ix: usize,
        menu: PopupMenu,
        _window: &Window,
        _cx: &App,
    ) -> PopupMenu {
        let has_selection = self.edit_state.has_selection();
        let _selected_data = self.get_selected_data();
        let row_is_selected = self.edit_state.selected_rows.contains(&row_ix);

        // Basic copy operations - use simple menu items (no submenus for now)
        let menu = if has_selection {
            menu.menu_with_icon(
                "Copy Selection as CSV",
                Icon::new(IconName::Copy),
                Box::new(crate::app::CopyAsCSV),
            )
            .menu("Copy Selection as JSON", Box::new(crate::app::CopyAsJSON))
            .menu("Copy Selection as SQL", Box::new(crate::app::CopyAsSQL))
            .menu(
                "Copy Selection as Markdown",
                Box::new(crate::app::CopyAsMarkdown),
            )
        } else {
            menu.menu(
                "Copy Cell",
                Box::new(CopyCell {
                    row: row_ix,
                    col: 0,
                }),
            )
            .menu("Copy Cell as CSV", Box::new(crate::app::CopyAsCSV))
            .menu("Copy Cell as JSON", Box::new(crate::app::CopyAsJSON))
            .menu("Copy Cell as SQL", Box::new(crate::app::CopyAsSQL))
            .menu(
                "Copy Cell as Markdown",
                Box::new(crate::app::CopyAsMarkdown),
            )
        };

        // Row selection options
        let menu = if row_is_selected {
            menu.menu_with_check("Select Row", true, Box::new(SelectRow { row: row_ix }))
        } else {
            menu.menu("Select Row", Box::new(SelectRow { row: row_ix }))
        };

        // Clear selection if we have any
        if has_selection {
            menu.separator()
                .menu("Clear Selection", Box::new(ClearSelection))
        } else {
            menu
        }
    }
}

pub struct ResultsPanel {
    focus_handle: FocusHandle,
    table: Entity<Table<ResultsTableDelegate>>,
    current_result: Option<QueryResult>,
    editing_input: Option<Entity<InputState>>,
    editing_cell: Option<(usize, usize)>,
    copy_handler: CopyHandler,
    current_selected_col: usize, // Track current column for selection/editing
}

impl ResultsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_connection_id(None, window, cx)
    }

    pub fn with_connection_id(
        connection_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut delegate = ResultsTableDelegate::default();

        // Set connection ID on delegate if provided
        if let Some(conn_id) = connection_id {
            delegate.set_connection_id(conn_id);
        }

        let table = cx.new(|cx| {
            Table::new(delegate, window, cx)
                .row_selectable(true)
                .col_selectable(false)
        });

        // The cell-level mouse events will handle selection and editing directly

        Self {
            table,
            focus_handle: cx.focus_handle(),
            current_result: None,
            editing_input: None,
            editing_cell: None,
            copy_handler: CopyHandler::new(),
            current_selected_col: 1, // Start with first data column (column 1, after row number)
        }
    }

    pub fn set_query_result(
        &mut self,
        result: QueryResult,
        connection_id: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        // Set connection ID on the delegate for table extraction
        if let Some(conn_id) = connection_id {
            self.table.update(cx, |table, _cx| {
                table.delegate_mut().set_connection_id(conn_id);
            });
        }

        self.table.update(cx, |table, cx| {
            table.delegate_mut().set_query_result(result.clone());
            table.refresh(cx);
        });
        self.current_result = Some(result.clone());

        // Clear any panel-level editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
    }

    pub fn commit_current_edit(&mut self, cx: &mut Context<Self>) {
        // Use the delegate's editing state instead of the panel's
        let editing_cell = self.table.read(cx).delegate().edit_state.editing_cell;

        if let Some((row, col)) = editing_cell {
            if let Some(input) = &self.table.read(cx).delegate().edit_state.editing_input {
                let new_value = input.read(cx).text().to_string();

                // Check if this is a new row
                let is_new_row = self
                    .table
                    .read(cx)
                    .delegate()
                    .edit_state
                    .pending_new_rows
                    .contains(&row);

                if is_new_row {
                    // For new rows, update the cell value and track as INSERT
                    self.table.update(cx, |table, cx| {
                        let delegate = table.delegate_mut();

                        // Update the actual cell value
                        if let Some(row_data) = delegate.rows.get_mut(row) {
                            if let Some(cell) = row_data.get_mut(col) {
                                *cell = new_value.clone();
                            }
                        }

                        // Update the INSERT change with the new value
                        if let Some(table_name) = &delegate.table_name {
                            let column_names: Vec<String> = delegate
                                .columns
                                .iter()
                                .map(|col| col.name.to_string())
                                .collect();
                            let values: Vec<String> = delegate
                                .rows
                                .get(row)
                                .unwrap_or(&vec!["".to_string(); column_names.len()])
                                .clone();
                            let values_str = values
                                .iter()
                                .map(|val| {
                                    if val.is_empty() || val == "NULL" {
                                        "NULL".to_string()
                                    } else {
                                        format!("'{}'", val.replace("'", "''"))
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ");

                            // Remove the old INSERT change if it exists
                            delegate.edit_state.changes.retain(|change| {
                                !(change.change_type == ChangeType::InsertRow
                                    && change.row_index == row)
                            });

                            // Add the updated INSERT change
                            let change = TableChange::new(
                                ChangeType::InsertRow,
                                table_name.clone(),
                                row,
                                None,
                                None,
                                Some(values_str),
                                None,
                                delegate.primary_key_column.clone(),
                            );
                            delegate.edit_state.add_change(change);
                        }

                        delegate.edit_state.stop_editing();
                        table.refresh(cx);
                    });
                } else {
                    // For existing rows, update the edited_values and commit as UPDATE
                    self.table.update(cx, |table, _cx| {
                        table
                            .delegate_mut()
                            .update_cell_value(row, col, new_value.clone());
                    });

                    // Then commit the edit
                    self.commit_cell_edit(row, col, new_value, cx);
                }

                // Clear panel editing state
                self.editing_input = None;
                self.editing_cell = None;
            }
        }
    }

    pub fn cancel_current_edit(&mut self, cx: &mut Context<Self>) {
        // Use the delegate's editing state instead of the panel's
        let editing_cell = self.table.read(cx).delegate().edit_state.editing_cell;

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
            .table
            .read(cx)
            .delegate()
            .rows
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_else(|| "".to_string());

        // Check if this is a new row (pending insert) or existing row
        let is_new_row = self
            .table
            .read(cx)
            .delegate()
            .edit_state
            .pending_new_rows
            .contains(&row);

        // Create input state for editing with the current cell value
        let input = cx.new(|cx| InputState::new(window, cx).default_value(&current_value));

        // Start editing in the delegate with the input
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
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
            let row_clone = row;
            let col_clone = col;
            cx.subscribe(&input, move |table, input, event, cx| {
                if let InputEvent::Change = event {
                    let new_text = input.read(cx).text().to_string();
                    log::info!(
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
                    table.refresh(cx);
                } else if let InputEvent::Blur = event {
                    // Handle blur - save current edit to edited_values when input loses focus
                    // Get the current editing cell and value
                    let editing_cell = table.delegate_mut().edit_state.editing_cell;
                    log::info!("Blur event triggered for editing_cell: {:?}", editing_cell);

                    if let Some((row, col)) = editing_cell {
                        let new_value = input.read(cx).text().to_string();
                        log::info!("Blur: saving value '{}' at ({}, {})", new_value, row, col);

                        // Commit the cell edit to create a TableChange entry
                        log::info!("Blur: committing cell edit at ({}, {})", row, col);
                        table.delegate_mut().commit_cell_edit(row, col);
                        table.refresh(cx);
                        log::info!("Blur: cell edit committed and table refreshed");
                    } else {
                        log::info!("Blur: no editing cell found");
                    }
                }
            })
            .detach();

            table.refresh(cx);
        });

        // Focus the input automatically when editing starts
        input.focus_handle(cx).focus(window);

        // Store the editing state in the panel for commit/cancel operations
        self.editing_input = Some(input.clone());
        self.editing_cell = Some((row, col));
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

        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

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

            // Track the change for SQL generation
            if let (Some(old_val), Some(tbl_name)) = (&old_value, &table_name) {
                if old_val != &new_value {
                    // Get primary key value (assuming first column is primary key)
                    let primary_key_value = delegate.get_primary_key_value(row);
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
                    );
                    delegate.edit_state.add_change(change);

                    // Log the change tracking (this will be visible when user commits)
                    // Note: We defer detailed logging to commit time to avoid cluttering the log
                }
            }

            // Stop editing and clear input
            delegate.edit_state.stop_editing();
            table.refresh(cx);
        });

        // Clear panel editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
        committed_value
    }

    pub fn cancel_cell_edit(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().cancel_cell_edit(row, col);
            // Stop editing and clear input
            table.delegate_mut().edit_state.stop_editing();
            table.refresh(cx);
        });

        // Clear panel editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
    }

    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        self.table
            .read(cx)
            .delegate()
            .edit_state
            .has_unsaved_changes()
    }

    pub fn get_table_name(&self, cx: &App) -> Option<String> {
        self.table
            .read(cx)
            .delegate()
            .get_table_name()
            .map(|s| s.to_string())
    }

    pub fn is_editing(&self, cx: &App) -> bool {
        self.table
            .read(cx)
            .delegate()
            .edit_state
            .editing_cell
            .is_some()
    }

    pub fn get_changes(&self, cx: &App) -> Vec<TableChange> {
        let table_read = self.table.read(cx);
        let _delegate = table_read.delegate();
        // Get changes directly from edited values in delegate
        let mut changes = Vec::new();
        let table_read = self.table.read(cx);
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
                ));
            }
        }

        log::info!(
            "Commit Changes: Got {} changes from edited_values",
            changes.len()
        );
        changes
    }

    pub fn clear_changes(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().edit_state.clear_changes();
            table.refresh(cx);
        });
        cx.notify();
    }

    pub fn commit_all_edits(&mut self, cx: &mut Context<Self>) -> Vec<(usize, usize, String)> {
        let mut committed_changes = Vec::new();

        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
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

            table.refresh(cx);
        });

        cx.notify();
        committed_changes
    }

    pub fn cancel_all_edits(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

            // Cancel current editing cell
            if let Some((row, col)) = delegate.edit_state.editing_cell {
                delegate.cancel_cell_edit(row, col);
            }

            // Clear all edited values
            delegate.edit_state.edited_values.clear();
            delegate.edit_state.original_values.clear();

            table.refresh(cx);
        });

        cx.notify();
    }

    pub fn get_current_editing_cell(&self, cx: &App) -> Option<(usize, usize)> {
        self.table.read(cx).delegate().edit_state.editing_cell
    }

    pub fn update_editing_cell_value(
        &mut self,
        row: usize,
        col: usize,
        new_value: String,
        cx: &mut Context<Self>,
    ) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().update_cell_value(row, col, new_value);
            table.refresh(cx);
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
        let change_operations = self.table.read(cx).delegate().create_change_operations();

        if change_operations.is_empty() {
            log::info!("Commit Changes: No changes to commit");
            return;
        }

        log::info!(
            "Commit Changes: Sending {} operations to async pipeline",
            change_operations.len()
        );

        // Get connection id from delegate (use fallback if not available)
        let connection_id = self.table.read(cx).delegate().connection_id.unwrap_or(0);
        // TODO: error here instead of fallback to connection_id 0

        // Get table name for logging
        let table_name = self
            .table
            .read(cx)
            .delegate()
            .table_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Create clones for different uses
        let change_operations_for_pipeline = change_operations.clone();
        let change_operations_for_logging = change_operations.clone();
        let table_name_for_logging = table_name.clone();
        let _table_name_for_event = table_name.clone();
        let connection_id_for_pipeline = connection_id;
        let connection_id_for_event = connection_id;

        // Create response channel for table operations
        let (response_tx, response_rx) = async_std::channel::bounded(1);

        // Emit a query execution started event
        cx.emit(AppEvent::QueryExecutionStarted {
            connection_id: Some(connection_id_for_event),
            query: format!(
                "Table operations on {} ({} operations)",
                table_name_for_logging,
                change_operations_for_logging.len()
            ),
        });

        log::info!("Commit Changes: Starting table operations execution");

        // Log the operations to SQL log if available
        if let Some(sql_log) = sql_log {
            for operation in &change_operations_for_logging {
                let sql_query = operation.to_sql_query();
                sql_log.update(cx, |log, cx| {
                    log.append_text(&blanco_ui::SqlLogMessage::SqlStatement(sql_query), cx);
                    log.append_text(
                        &blanco_ui::SqlLogMessage::Comment("Executing table operation".to_string()),
                        cx,
                    );
                });
            }

            // Log summary
            let summary = format!(
                "Executing {} table operations",
                change_operations_for_logging.len()
            );
            sql_log.update(cx, |log, cx| {
                log.append_text(&blanco_ui::SqlLogMessage::Comment(summary), cx);
            });
        }

        // Spawn background task to execute table operations
        let db_service = cx.global::<DbService>().clone();
        let table_entity = self.table.clone();
        let sql_log_entity: Option<Entity<blanco_ui::SqlLog>> = sql_log.cloned();

        cx.background_spawn(async move {
            let start_time = std::time::Instant::now();

            // Execute table operations using DbService
            let result = match db_service
                .get_or_create_connection(connection_id_for_pipeline)
                .await
            {
                Ok(connection) => {
                    log::info!("Got connection for table operations");

                    // Convert table operations to SQL and execute them
                    let mut total_rows_affected = 0u64;
                    let mut operations_executed = 0;
                    let mut error_message = None;
                    let mut success = true;

                    for operation in &change_operations_for_pipeline {
                        let sql_query = operation.to_sql_query();
                        match connection.execute_query(&sql_query, None).await {
                            Ok(query_result) => {
                                total_rows_affected += query_result.rows_affected;
                                operations_executed += 1;
                                log::debug!("Successfully executed operation: {}", sql_query);
                            }
                            Err(e) => {
                                log::error!("Failed to execute operation '{}': {}", sql_query, e);
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
                    log::error!("Failed to get connection for table operations: {}", e);
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

            log::info!(
                "Table operations completed in {:?}, success: {}",
                start_time.elapsed(),
                result.success
            );

            // Send response back through the channel
            if let Err(e) = response_tx.send(result.clone()).await {
                log::error!("Failed to send table operation response: {}", e);
            }

            result
        })
        .detach();

        // Spawn async task to handle the response
        let sql_log_response_entity: Option<Entity<blanco_ui::SqlLog>> = sql_log.cloned();
        cx.spawn(async move |entity, cx| {
            match response_rx.recv().await {
                Ok(response) => {
                    log::info!("Received table operation response: success={}, rows_affected={:?}",
                        response.success, response.rows_affected);

                    // Handle successful operations
                    if response.success {
                        // Clear edits and refresh the table
                        let _ = entity.update(cx, |panel, cx| {
                            panel.table.update(cx, |table, cx| {
                                table.delegate_mut().edit_state.clear_edits();
                                table.refresh(cx);
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
                                connection_id: Some(response.connection_id),
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
                                connection_id: Some(response.connection_id),
                                success: false,
                                rows_affected: None,
                                error_message: response.error_message,
                                operations_executed: response.operations_executed,
                            });
                        });
                    }
                }
                Err(e) => {
                    log::error!("Failed to receive table operation response: {}", e);

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
            .table
            .read(cx)
            .delegate()
            .table_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let connection_id = self.table.read(cx).delegate().connection_id;

        // TODO: error here instead of fallback to sqlite::memory

        log::info!(
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
                            self.table.update(cx, |table, _cx| {
                                if let Some(cell) = table.delegate_mut().get_cell_mut(row, col) {
                                    *cell = old_value.clone();
                                }
                            });
                        }
                    }
                }
                ChangeType::InsertRow => {
                    // Remove inserted rows (reverse order to maintain indices)
                    let row = change.row_index;
                    self.table.update(cx, |table, _cx| {
                        table.delegate_mut().remove_row(row);
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

        log::info!(
            "Rollback Changes: Successfully rolled back {} changes",
            changes_count
        );
        cx.notify();
    }

    pub fn add_new_row(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let column_count = delegate.columns.len();

            // Create a new row with empty values
            let new_row: Vec<String> = (0..column_count).map(|_| "".to_string()).collect();
            delegate.rows.push(new_row);

            // Mark this as a pending new row
            let new_row_index = delegate.rows.len() - 1;
            delegate.edit_state.pending_new_rows.push(new_row_index);

            // Track the INSERT change with NULL values
            if let Some(table_name) = &delegate.table_name {
                let _column_names: Vec<String> = delegate
                    .columns
                    .iter()
                    .map(|col| col.name.to_string())
                    .collect();
                let values_str = (0..column_count)
                    .map(|_| "NULL".to_string())
                    .collect::<Vec<_>>()
                    .join(", ");

                let change = TableChange::new(
                    ChangeType::InsertRow,
                    table_name.clone(),
                    new_row_index,
                    None,
                    None,
                    Some(values_str.clone()),
                    None,
                    delegate.primary_key_column.clone(),
                );
                delegate.edit_state.add_change(change);
            }

            table.refresh(cx);
        });
        cx.notify();
    }

    pub fn duplicate_row(&mut self, row_index: usize, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

            // Check if the row exists
            if let Some(row_to_duplicate) = delegate.rows.get(row_index).cloned() {
                // Add the duplicated row
                delegate.rows.push(row_to_duplicate.clone());

                // Mark this as a pending new row
                let new_row_index = delegate.rows.len() - 1;
                delegate.edit_state.pending_new_rows.push(new_row_index);

                // Track the INSERT change with proper column values
                if let Some(table_name) = &delegate.table_name {
                    // Create a proper representation of the row data for SQL
                    let _column_names: Vec<String> = delegate
                        .columns
                        .iter()
                        .map(|col| col.name.to_string())
                        .collect();
                    let values_str = row_to_duplicate
                        .iter()
                        .map(|val| {
                            if val.is_empty() || val == "NULL" {
                                "NULL".to_string()
                            } else {
                                format!("'{}'", val.replace("'", "''"))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");

                    let change = TableChange::new(
                        ChangeType::InsertRow,
                        table_name.clone(),
                        new_row_index,
                        None,
                        None,
                        Some(values_str.clone()),
                        None,
                        delegate.primary_key_column.clone(),
                    );
                    delegate.edit_state.add_change(change);
                }

                table.refresh(cx);
            }
        });
        cx.notify();
    }

    /// Set a cell value to NULL
    pub fn set_cell_to_null(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

            // Get current value for change tracking
            let old_value = delegate.rows.get(row).and_then(|r| r.get(col)).cloned();

            // Set the cell value to NULL
            if let Some(row_data) = delegate.rows.get_mut(row) {
                if let Some(cell) = row_data.get_mut(col) {
                    *cell = "NULL".to_string();
                }
            }

            // Track the change
            if let Some(table_name) = &delegate.table_name {
                let primary_key_value = delegate.rows.get(row).and_then(|r| r.first()).cloned();

                let change = TableChange::new(
                    ChangeType::UpdateCell,
                    table_name.clone(),
                    row,
                    Some(col),
                    old_value,
                    Some("NULL".to_string()),
                    primary_key_value.clone(),
                    delegate.primary_key_column.clone(),
                );
                delegate.edit_state.add_change(change);
            }

            table.refresh(cx);
        });
        cx.notify();
    }

    /// Clear a cell value (set to empty string)
    pub fn clear_cell_value(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

            // Get current value for change tracking
            let old_value = delegate.rows.get(row).and_then(|r| r.get(col)).cloned();

            // Set the cell value to empty string
            if let Some(row_data) = delegate.rows.get_mut(row) {
                if let Some(cell) = row_data.get_mut(col) {
                    *cell = "".to_string();
                }
            }

            // Track the change
            if let Some(table_name) = &delegate.table_name {
                let primary_key_value = delegate.rows.get(row).and_then(|r| r.first()).cloned();

                let change = TableChange::new(
                    ChangeType::UpdateCell,
                    table_name.clone(),
                    row,
                    Some(col),
                    old_value,
                    Some("".to_string()),
                    primary_key_value.clone(),
                    delegate.primary_key_column.clone(),
                );
                delegate.edit_state.add_change(change);
            }

            table.refresh(cx);
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
            log::info!(
                "Table operation completed successfully: {} operations, {} rows affected",
                operations_executed,
                rows_affected.unwrap_or(0)
            );

            // Clear edit state after successful commit
            self.table.update(cx, |table, cx| {
                table.delegate_mut().edit_state.clear_edits();
                table.refresh(cx);
            });

            // Optionally refresh the data or show a success message
            cx.notify();
        } else {
            log::error!(
                "Table operation failed: {}",
                error_message.unwrap_or_else(|| "Unknown error".to_string())
            );

            // Keep the edit state so user can retry or fix issues
            // Don't refresh the table to preserve user's changes
        }
    }

    // Copy and selection action handlers
    fn on_copy_cell(&mut self, action: &CopyCell, _window: &mut Window, cx: &mut Context<Self>) {
        let cell_value = self
            .table
            .read(cx)
            .delegate()
            .get_cell_value(action.row, action.col);

        // Get column type if available
        let column_types = self.table.read(cx).delegate().column_types.clone();
        let column_type = column_types
            .get(action.col.saturating_sub(1)) // Adjust for row number column
            .map(|s| s.as_str())
            .unwrap_or("text");

        if let Some(cell_value) = cell_value {
            if let Err(e) = self
                .copy_handler
                .copy_single_cell(&cell_value, column_type, "csv", cx)
            {
                log::error!("Failed to copy cell: {}", e);
            }
        }
    }

    fn on_copy_as_csv(
        &mut self,
        _action: &crate::app::CopyAsCSV,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_data = self.table.read(cx).delegate().get_selected_data();

        if let Err(e) = self.copy_handler.copy_as_format(&selected_data, "csv", cx) {
            log::error!("Failed to copy as CSV: {}", e);
        }
    }

    fn on_copy_as_json(
        &mut self,
        _action: &crate::app::CopyAsJSON,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_data = self.table.read(cx).delegate().get_selected_data();

        if let Err(e) = self.copy_handler.copy_as_format(&selected_data, "json", cx) {
            log::error!("Failed to copy as JSON: {}", e);
        }
    }

    fn on_copy_as_sql(
        &mut self,
        _action: &crate::app::CopyAsSQL,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_data = self.table.read(cx).delegate().get_selected_data();

        if let Err(e) = self.copy_handler.copy_as_format(&selected_data, "sql", cx) {
            log::error!("Failed to copy as SQL: {}", e);
        }
    }

    fn on_copy_as_markdown(
        &mut self,
        _action: &crate::app::CopyAsMarkdown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_data = self.table.read(cx).delegate().get_selected_data();

        if let Err(e) = self
            .copy_handler
            .copy_as_format(&selected_data, "markdown", cx)
        {
            log::error!("Failed to copy as Markdown: {}", e);
        }
    }

    fn on_select_row(&mut self, action: &SelectRow, _window: &mut Window, cx: &mut Context<Self>) {
        self.table.update(cx, |table, _cx| {
            let delegate = table.delegate_mut();
            delegate.select_row(action.row);
            table.refresh(_cx);
        });
        cx.notify();
    }

    fn on_clear_selection(
        &mut self,
        _action: &crate::app::ClearSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table.update(cx, |table, _cx| {
            let delegate = table.delegate_mut();
            delegate.edit_state.clear_selection();
            table.refresh(_cx);
        });
        cx.notify();
    }

    fn on_select_row_action(
        &mut self,
        action: &crate::app::SelectRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Handle row selection for the current column
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

            // Select the current column in the clicked row
            delegate.edit_state.clear_selection();
            delegate
                .edit_state
                .selected_cells
                .insert((action.row, self.current_selected_col));
            table.refresh(cx);
        });
        cx.notify();
    }

    /// Navigate cells in the table with keyboard
    fn navigate_cells(&mut self, cx: &mut Context<Self>, row_delta: isize, col_delta: isize) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

            // Start from current tracked position
            let mut current_row = 1;
            let mut current_col = self.current_selected_col;

            // Find first selected cell, or use current position
            if let Some(&(row, col)) = delegate.edit_state.selected_cells.iter().next() {
                current_row = row;
                current_col = col;
            }

            // Calculate new position
            let new_row = (current_row as isize + row_delta).max(1) as usize;
            let new_col = (current_col as isize + col_delta).max(1) as usize;

            // Keep within bounds
            let max_row = delegate.rows.len();
            let max_col = delegate.columns.len().saturating_sub(1);

            let final_row = new_row.min(max_row);
            let final_col = new_col.min(max_col);

            // Update both tracked columns
            self.current_selected_col = final_col;
            delegate.edit_state.current_column = final_col;

            // Clear current selection and select new cell
            delegate.edit_state.clear_selection();
            delegate
                .edit_state
                .selected_cells
                .insert((final_row, final_col));

            table.refresh(cx);
        });
        cx.notify();
    }

    /// Select the current cell based on keyboard focus
    fn select_current_cell(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();

            // If there are selected cells, toggle the first one
            if let Some(&(row, col)) = delegate.edit_state.selected_cells.iter().next() {
                if delegate.edit_state.selected_cells.contains(&(row, col)) {
                    // If already selected, clear all selections
                    delegate.edit_state.clear_selection();
                }
            } else {
                // If no selected cells, select the first cell
                if !delegate.rows.is_empty() && delegate.columns.len() > 1 {
                    delegate.edit_state.selected_cells.insert((1, 1));
                }
            }

            table.refresh(cx);
        });
        cx.notify();
    }

    /// Sync the ResultsPanel's current_selected_col with the delegate's current_column
    #[allow(dead_code)]
    fn sync_current_column(&mut self, cx: &mut Context<Self>) {
        self.current_selected_col = self.table.read(cx).delegate().edit_state.current_column;
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
        // Check for pending edits
        if let Some((row, col)) = self.table.read(cx).delegate().pending_edit_cell {
            // Clear the pending edit and start editing
            self.table.update(cx, |table, _cx| {
                table.delegate_mut().pending_edit_cell = None;
            });
            self.start_cell_edit(row, col, window, cx);
        }

        let _row_count = self.table.read(cx).delegate().rows_count(cx);
        let _has_unsaved_changes = self.has_unsaved_changes(cx);
        let _table_name = self.get_table_name(cx);
        let _is_editable = self.table.read(cx).delegate().is_editable();

        v_flex()
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            // Handle keyboard events for commit/cancel
            .on_key_down(
                cx.listener(|this, event: &gpui::KeyDownEvent, _window, cx| {
                    match event.keystroke.key.as_str() {
                        "enter" => {
                            if this.is_editing(cx) {
                                this.commit_current_edit(cx);
                            }
                        }
                        "escape" => {
                            if this.is_editing(cx) {
                                this.cancel_current_edit(cx);
                            } else {
                                // Clear all selections when not editing
                                this.table.update(cx, |table, cx| {
                                    table.delegate_mut().clear_selection();
                                    table.refresh(cx);
                                });
                                cx.notify();
                            }
                        }
                        "up" => {
                            // Navigate up in the table
                            if this.is_editing(cx) {
                                // If editing, cancel editing first
                                this.cancel_current_edit(cx);
                            } else {
                                // Navigate to previous row
                                this.navigate_cells(cx, -1, 0);
                            }
                        }
                        "down" => {
                            // Navigate down in the table
                            if this.is_editing(cx) {
                                // If editing, cancel editing first
                                this.cancel_current_edit(cx);
                            } else {
                                // Navigate to next row
                                this.navigate_cells(cx, 1, 0);
                            }
                        }
                        "left" => {
                            // Navigate left in the table
                            if this.is_editing(cx) {
                                // If editing, cancel editing first
                                this.cancel_current_edit(cx);
                            } else {
                                // Navigate to previous column
                                this.navigate_cells(cx, 0, -1);
                            }
                        }
                        "right" => {
                            // Navigate right in the table
                            if this.is_editing(cx) {
                                // If editing, cancel editing first
                                this.cancel_current_edit(cx);
                            } else {
                                // Navigate to next column
                                this.navigate_cells(cx, 0, 1);
                            }
                        }
                        "space" => {
                            // Select current cell
                            if !this.is_editing(cx) {
                                this.select_current_cell(cx);
                            }
                        }
                        "ctrl-shift-n" => {
                            // Ctrl + Shift + N: Set current cell to NULL
                            if let Some((row, col)) = this.get_current_editing_cell(cx) {
                                this.set_cell_to_null(row, col, cx);
                            }
                        }
                        "ctrl-shift-k" => {
                            // Ctrl + Shift + K: Clear current cell value
                            if let Some((row, col)) = this.get_current_editing_cell(cx) {
                                this.clear_cell_value(row, col, cx);
                            }
                        }
                        _ => {}
                    }
                }),
            )
            // Handle table events for row-based selection
            .on_action(cx.listener(Self::on_select_row_action))
            // Handle copy and selection actions
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_copy_as_csv))
            .on_action(cx.listener(Self::on_copy_as_json))
            .on_action(cx.listener(Self::on_copy_as_sql))
            .on_action(cx.listener(Self::on_copy_as_markdown))
            .on_action(cx.listener(Self::on_select_row))
            .on_action(cx.listener(Self::on_clear_selection))
            // The table component (table should have built-in scrolling)
            .child(
                div()
                    .id("results-table")
                    .flex_1() // Allow table to fill available space
                    .overflow_hidden()
                    .min_h(px(200.0)) // Minimum height for table
                    .child(self.table.clone()),
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
}
