use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString, Styled,
    Subscription, TextRun, Window, div, px,
};
use gpui_component::popover::{Popover, PopoverState};
use gpui_component::{
    ActiveTheme, Icon, Sizable,
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::PopupMenu,
    table::{Column, ColumnSort, Table, TableDelegate, TableState},
    v_flex,
};
use serde_json::Value;

use blanco_core::{
    DatabaseService as DatabaseServiceTrait, QueryResult, connection_trait::ForeignKeyInfo,
};
use database::DatabaseService;

use crate::app::{AddRow, DuplicateRow};
use crate::app_events::AppEvent;
use crate::foreign_key_popover::ForeignKeyPopover;
use crate::results_panel::table_operations::{
    ColumnChange, OperationType, RowIdentifier, TableChangeOperation,
};
use crate::transformers::CopyHandler;
use blanco_ui::IconName;

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

#[derive(Clone, Debug)]
pub struct TableChange {
    pub change_type: ChangeType,
    pub table_name: String,
    pub row_index: usize,
    pub column_index: Option<usize>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub primary_key_value: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChangeType {
    UpdateCell,
    InsertRow,
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
    insert_values: Option<Vec<Option<String>>>,
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

    pub fn insert_values(mut self, values: Option<Vec<Option<String>>>) -> Self {
        self.insert_values = values;
        self
    }

    pub fn build(self) -> TableChange {
        TableChange {
            change_type: self.change_type,
            table_name: self.table_name,
            row_index: self.row_index,
            column_index: self.column_index,
            old_value: self.old_value,
            new_value: self.new_value,
            primary_key_value: self.primary_key_value,
        }
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
        insert_values: Option<Vec<Option<String>>>,
    ) -> Self {
        TableChangeBuilder::new(change_type, table_name, row_index)
            .column_index(column_index)
            .old_value(old_value)
            .new_value(new_value)
            .primary_key_value(primary_key_value)
            .insert_values(insert_values)
            .build()
    }
}

#[derive(Default)]
pub struct ResultsTableDelegate {
    pub columns: Vec<Column>,
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub edit_state: CellEditState,
    pub table_name: Option<String>,
    pub primary_key_column: Option<String>,
    pub pending_edit_cell: Option<(usize, usize)>,
    pub connection_id: i64,
    pub database_name: SharedString,
    pub original_query: Option<String>,
    /// Foreign key metadata: column_index (excluding row number column) -> FK info
    pub foreign_keys: HashMap<usize, ForeignKeyInfo>,
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
        self.database_name = database_name.to_string().into();
    }

    /// Set the original SQL query for alias resolution
    pub fn set_original_query(&mut self, query: String) {
        self.original_query = Some(query);
    }

    /// Convert table changes to database-agnostic TableChangeOperations
    /// This method consolidates multiple changes to the same row into single operations.
    pub fn create_change_operations(&self) -> Vec<TableChangeOperation> {
        use std::collections::HashMap;

        // Map to consolidate changes by (table_name, pk_column, pk_value)
        let mut update_operations: HashMap<(String, String, String), Vec<ColumnChange>> =
            HashMap::new();
        let mut insert_operations: Vec<TableChangeOperation> = Vec::new();

        for change in &self.edit_state.changes {
            match change.change_type {
                ChangeType::UpdateCell => {
                    // Get primary key information - always use delegate's primary key column
                    let (pk_column, pk_value) = if let Some(ref pk_column) = self.primary_key_column
                    {
                        if let Some(pk_val) = &change.primary_key_value {
                            (pk_column.clone(), pk_val.clone())
                        } else {
                            continue;
                        }
                    } else {
                        continue; // Skip this change if we can't determine PK
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
                            // Use None for NULL values, Some for actual values (including empty strings)
                            let new_value = if value == "NULL" {
                                None
                            } else {
                                Some(value.clone())
                            };
                            ColumnChange {
                                column_name,
                                old_value: None,
                                new_value,
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

    pub fn set_query_result(&mut self, result: QueryResult, window: &Window, cx: &App) {
        // Clear previous edit state
        self.edit_state.clear_all();
        self.pending_edit_cell = None;

        // Store column types
        self.column_types = result.column_types.clone();

        // Use the theme's font family for measurement (typically the mono font for tables)
        let text_size = px(12.);
        let font = gpui::font(cx.theme().mono_font_family.clone());

        // Calculate column widths based on actual text measurement
        let mut column_widths: Vec<f64> = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col_name)| {
                // Check if this column has a foreign key
                let has_foreign_key = result
                    .table_columns
                    .as_ref()
                    .and_then(|cols| cols.get(i))
                    .and_then(|col| col.foreign_key.as_ref())
                    .is_some();

                // Check if this column is a primary key
                let is_primary_key = result
                    .table_columns
                    .as_ref()
                    .and_then(|cols| cols.get(i))
                    .map(|col| col.is_primary_key)
                    .unwrap_or(false);

                // Measure column name width
                let shaped_line = window.text_system().shape_line(
                    SharedString::from(col_name),
                    text_size,
                    &[TextRun {
                        len: col_name.len(),
                        font: font.clone(),
                        color: gpui::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                let mut max_width = shaped_line.width.to_f64();

                // Check sample rows to determine content width (limit to first 5 rows for performance)
                for row in result.rows.iter().take(5) {
                    if let Some(cell_value) = row.get(i) {
                        let shaped_line = window.text_system().shape_line(
                            SharedString::from(cell_value),
                            text_size,
                            &[TextRun {
                                len: cell_value.len(),
                                font: font.clone(),
                                color: gpui::black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            None,
                        );
                        max_width = max_width.max(shaped_line.width.to_f64());
                    }
                }

                // Add padding for cell content (px_2 on each side = 8px * 2 = 16px)
                max_width += 16.0;

                // Add padding for sorting icon
                max_width += 24.0;

                // Add padding for the input expansion icon
                max_width += 24.0;

                // Add padding for foreign key or primary key icon if applicable
                if has_foreign_key || is_primary_key {
                    max_width += 16.0;
                }

                // Account for cell borders and extra spacing
                max_width += 2.0;

                // Apply minimum and maximum bounds
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

        // Extract primary key column name from table_columns metadata
        self.primary_key_column = result.table_columns.as_ref().and_then(|columns| {
            columns
                .iter()
                .find(|c| c.is_primary_key)
                .map(|c| c.name.clone())
        });

        // Clear previous foreign keys
        self.foreign_keys.clear();

        // Load foreign key metadata from table_columns if available
        if let Some(ref columns) = result.table_columns {
            let foreign_keys: std::collections::HashMap<usize, ForeignKeyInfo> = columns
                .iter()
                .enumerate()
                .filter_map(|(idx, col)| col.foreign_key.as_ref().map(|fk| (idx, fk.clone())))
                .collect();

            if !foreign_keys.is_empty() {
                tracing::debug!(
                    "Loaded {} foreign key(s) for table '{}' from query result metadata",
                    foreign_keys.len(),
                    self.table_name.as_deref().unwrap_or("unknown")
                );
            }
            self.foreign_keys = foreign_keys;
        }
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

        // Bail early if table is not editable (no table_name or primary_key_column)
        if !self.is_editable() {
            tracing::info!("Table is not editable, bailing commit");
            self.edit_state.editing_cell = None;
            return None;
        }

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
        let has_fk = !is_row_number_col && self.foreign_keys.contains_key(&(col_ix - 1));
        let is_pk = !is_row_number_col
            && self
                .primary_key_column
                .as_ref()
                .is_some_and(|pk| pk == &col.name);

        div()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .pt(px(1.))
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(col.name.to_string())
                    .when(is_pk, |this| {
                        this.child(
                            Icon::new(IconName::Key)
                                .size(px(10.))
                                .text_color(cx.theme().yellow),
                        )
                    })
                    .when(has_fk && !is_pk, |this| {
                        this.child(
                            Icon::new(IconName::Key)
                                .size(px(10.))
                                .text_color(cx.theme().blue),
                        )
                    }),
            )
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
                                        Input::new(&input).disabled(!self.is_editable()).size_full().font_family(cx.theme().mono_font_family.clone()).text_size(px(12.)).suffix(
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
                                                            crate::results_panel::ResultsPanel::subscribe_to_input_events(
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
                                .disabled(!self.is_editable())
                                .flex_1()
                                .text_size(px(12.))
                                .border_0()
                                .pl_0()
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
                                                crate::results_panel::ResultsPanel::subscribe_to_input_events(
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
            let cell_content = h_flex()
                .items_center()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(display_text.clone()),
                )
                .when(!is_row_number_col, |this| {
                    this.child(
                        div()
                            .invisible()
                            .group_hover("", |this| this.visible())
                            .child(
                                Clipboard::new(format!("cell-clipboard-{}-{}", row_ix, col_ix))
                                    .value(display_text.clone()),
                            ),
                    )
                })
                .when_some(
                    self.foreign_keys
                        .get(&(col_ix - 1))
                        .filter(|_| !is_row_number_col),
                    |this, fk_info| {
                        let fk_info = fk_info.clone();
                        let cell_value = display_text.clone();
                        let popover_id = format!("fk-popover-{}-{}", row_ix, col_ix);
                        let connection_id = self.connection_id.clone();
                        let database_name = if self.database_name.is_empty() {
                            None
                        } else {
                            Some(self.database_name.clone())
                        };

                        this.child(
                            div()
                                .invisible()
                                .group_hover("", |this| this.visible())
                                .child(
                                    Popover::new(popover_id.clone())
                                        .anchor(gpui::Corner::BottomRight)
                                        .trigger(
                                            Button::new(format!("fk-trigger-{}", popover_id))
                                                .icon(IconName::Search)
                                                .ghost()
                                                .xsmall(),
                                        )
                                        .content(
                                            move |_state: &mut PopoverState,
                                                  window: &mut Window,
                                                  cx: &mut Context<
                                                PopoverState,
                                            >| {
                                                let database_name_clone = database_name.clone();

                                                // Use use_keyed_state to lazily create the FK popover entity
                                                let fk_popover = window.use_keyed_state(
                                                    popover_id.clone(),
                                                    cx,
                                                    |_id, cx| {
                                                        ForeignKeyPopover::new(
                                                            &fk_info.foreign_table_name,
                                                            &fk_info.foreign_column_name,
                                                            &cell_value,
                                                            connection_id,
                                                            database_name_clone,
                                                            cx,
                                                        )
                                                    },
                                                );
                                                div().max_w(px(300.)).child(fk_popover)
                                            },
                                        ),
                                ),
                        )
                    },
                );

            div()
                .group("")
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
                        .pl_2()
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
                // All data cells (non-row-number) should be selectable for copying
                .when(!is_row_number_col, |this| {
                    this.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |table, event: &gpui::MouseDownEvent, _window, cx| {
                            if event.click_count == 2 {
                                let delegate = table.delegate_mut();
                                delegate.clear_selection();
                                delegate.set_pending_edit_cell(row_ix, col_ix);
                                table.refresh(cx);
                                cx.notify();
                            }
                        }),
                    )
                })
                .py_1()
                .child(cell_content)
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
