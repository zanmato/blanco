use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Focusable, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, TextRun, Window, div, px,
};
use gpui_component::popover::{Popover, PopoverState};
use gpui_component::{
    ActiveTheme, Icon, Sizable,
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    h_flex,
    input::{Input, InputState},
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate, TableState},
    tooltip::Tooltip,
    v_flex,
};
use serde_json::Value;

use blanco_core::{QueryResult, connection_trait::ColumnType};

use super::cell_edit_state::{
    CellEditState, ChangeType, TableChange, compare_numeric, format_value_for_display,
};
use super::foreign_key_popover::ForeignKeyPopover;
use crate::app::{
    AddRow, CopyAsCSV, CopyAsJSON, CopyAsMarkdown, CopyAsSQL, DeleteRow, DuplicateRow, ExportAsCSV,
    ExportAsJSON, ExportAsMarkdown, ExportAsSQL, SetCellNull,
};
use crate::results_panel::ResultsPanel;
use crate::results_panel::table_operations::{
    ColumnChange, OperationType, RowIdentifier, TableChangeOperation,
};
use blanco_ui::IconName;

#[derive(Default)]
pub struct ResultsTableDelegate {
    pub columns: Vec<Column>,
    pub column_types: Vec<ColumnType>,
    pub rows: Vec<Vec<Option<String>>>,
    pub edit_state: CellEditState,
    pub table_name: Option<String>,
    pub primary_key_column: Option<String>,
    pub connection_id: i64,
    pub database_name: SharedString,
    pub db_type: Option<database::DatabaseType>,
    pub original_query: Option<String>,
    /// Column metadata for tooltips and rendering
    table_columns: Vec<blanco_core::connection_trait::ColumnInfo>,
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
    pub fn get_insert_values(
        &self,
        row_index: usize,
        exclude_primary_key: bool,
    ) -> Vec<Option<String>> {
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
    pub fn get_cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Option<String>> {
        self.rows
            .get_mut(row)
            .and_then(|row_data| row_data.get_mut(col))
    }

    /// Set the connection ID for database operations
    pub fn set_connection_id(
        &mut self,
        connection_id: i64,
        database_name: &str,
        db_type: database::DatabaseType,
    ) {
        self.connection_id = connection_id;
        self.database_name = database_name.to_string().into();
        self.db_type = Some(db_type);
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
        let mut delete_operations: Vec<TableChangeOperation> = Vec::new();

        for change in &self.edit_state.changes {
            match change.change_type {
                ChangeType::DeleteRow => {
                    // Get primary key information for DELETE operation
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

                    let operation = TableChangeOperation {
                        operation_type: OperationType::Delete,
                        table_name: change.table_name.clone(),
                        row_identifier: RowIdentifier::PrimaryKey {
                            column: pk_column,
                            value: pk_value,
                        },
                        changes: vec![],
                    };
                    delete_operations.push(operation);
                }
                ChangeType::UpdateCell => {
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
                        pk_value.is_none_or(|v| v.as_ref().is_none_or(|s| s.is_empty()))
                    });

                    let column_names = self.get_insert_column_names(exclude_primary_key);
                    let row_values = self.get_insert_values(change.row_index, exclude_primary_key);

                    let column_changes: Vec<ColumnChange> = column_names
                        .into_iter()
                        .zip(row_values.iter())
                        .map(|(column_name, value)| ColumnChange {
                            column_name,
                            old_value: None,
                            new_value: value.clone(),
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

        // Add delete operations
        operations.extend(delete_operations);

        operations
    }

    pub fn set_query_result(&mut self, result: QueryResult, window: &Window, cx: &App) {
        // Clear previous edit state
        self.edit_state.clear_all();
        self.clear_selection();
        self.rows.clear();
        self.rows.shrink_to_fit();

        // Store column types (move instead of clone to avoid memory leak)
        self.column_types = result.column_types;

        // Use the theme's font family for measurement (typically the mono font for tables)
        let text_size = px(12.);
        let font = gpui::font(cx.theme().mono_font_family.clone());

        // Pre-collect sample rows (cloned) to avoid borrow conflicts with result.rows
        let sample_rows: Vec<Vec<Option<String>>> = result.rows.iter().take(5).cloned().collect();

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
                for row in sample_rows.iter() {
                    if let Some(Some(cell_value)) = row.get(i) {
                        let shared_value: SharedString = cell_value.clone().into();
                        let shaped_line = window.text_system().shape_line(
                            shared_value,
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
                let mut new_row = vec![Some((row_index + 1).to_string())];
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

        // Store table columns metadata for tooltips and rendering
        self.table_columns = result.table_columns.clone().unwrap_or_default();
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

    pub fn update_cell_value(&mut self, row: usize, col: usize, new_value: Option<String>) {
        self.edit_state.edited_values.insert((row, col), new_value);
    }

    pub fn commit_cell_edit(&mut self, row: usize, col: usize) -> Option<Option<String>> {
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
            tracing::info!(
                "Found edited value: '{:?}' for ({}, {})",
                new_value,
                row,
                col
            );

            // Get the original value
            let original_value = self.edit_state.original_values.get(&(row, col)).cloned();

            // Update the actual row data
            if let Some(row_data) = self.rows.get_mut(row) {
                if let Some(cell) = row_data.get_mut(col) {
                    tracing::info!("Updating cell from '{:?}' to '{:?}'", cell, new_value);
                    *cell = new_value.clone();
                    tracing::info!("Cell updated successfully");
                } else {
                    tracing::info!("No cell found at column {}", col);
                }
            } else {
                tracing::info!("No row data found at row {}", row);
            }

            // Track the change for SQL generation (but not for new rows)
            // original_value is Option<Option<String>>, the outer Option is whether editing started
            if let (Some(original), Some(table_name)) = (&original_value, &self.table_name) {
                // Check if this is a new row, if so, don't create UPDATE changes
                // New rows should be handled by INSERT operations only
                if !self.edit_state.is_new_row(row) {
                    // Get primary key value, if updating the PK column itself, use the original value
                    let primary_key_value = if let Some(pk_column) = &self.primary_key_column {
                        // Find the index of the primary key column
                        if let Some(pk_index) = self
                            .columns
                            .iter()
                            .position(|col| col.name.as_str() == pk_column)
                        {
                            // If we're updating the primary key column itself, get the original value
                            if pk_index == col {
                                self.edit_state
                                    .original_values
                                    .get(&(row, col))
                                    .and_then(|v| v.clone())
                            } else {
                                // Otherwise get the current value from the row
                                self.rows
                                    .get(row)
                                    .and_then(|r| r.get(pk_index))
                                    .and_then(|v| v.clone())
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
                        original.clone(),
                        new_value.clone(),
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

    pub fn is_editable(&self) -> bool {
        self.table_name.is_some() && self.primary_key_column.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.edit_state.clear_selection();
    }

    pub fn handle_minimize(
        state: &mut TableState<ResultsTableDelegate>,
        cell: (usize, usize),
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        // Get current text before recreating input
        let current_text = state
            .delegate_mut()
            .edit_state
            .editing_input
            .as_ref()
            .map(|input| input.read(cx).text().to_string())
            .unwrap_or_default();

        // Recreate InputState with single-line mode and subscribe to events
        let new_input = cx.new(|cx| InputState::new(window, cx).default_value(current_text));
        state.delegate_mut().edit_state.editing_input = Some(new_input.clone());

        // Re-subscribe to input events (blur/change)
        ResultsPanel::subscribe_to_input_events(state, &new_input, cell.1, cell.0, cx);

        // Re-focus the input after recreation
        new_input.focus_handle(cx).focus(window, cx);

        // Toggle expanded state
        state
            .delegate_mut()
            .edit_state
            .toggle_expanded(cell.1, cell.0);

        state.refresh(cx);
        cx.notify();
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
        self.columns
            .get(col_ix)
            .cloned()
            .unwrap_or_else(|| Column::new(format!("col_{}", col_ix), format!("Column {}", col_ix)))
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_row_number_col = col_ix == 0;
        let col = &self.columns[col_ix];
        let col_info = if !is_row_number_col {
            self.table_columns.get(col_ix - 1)
        } else {
            None
        };
        let has_fk = col_info.is_some_and(|c| c.foreign_key.is_some());
        let is_pk = !is_row_number_col
            && self
                .primary_key_column
                .as_ref()
                .is_some_and(|pk| pk == &col.name);
        let is_nullable = col_info.is_some_and(|c| c.is_nullable);

        let tooltip_id = format!("col-tooltip-{}", col_ix);

        div()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .pt(px(1.))
            .id(tooltip_id.clone())
            .when_some(col_info, |this, col_info| {
                // Clone all needed values before the closure
                let col_name = col.name.to_string();
                let data_type = col_info.data_type.clone();
                let is_nullable = col_info.is_nullable;
                let default_value = col_info.default_value.clone();
                let max_length = col_info.character_maximum_length;
                this.tooltip(move |window, cx| {
                    Tooltip::element({
                        let col_name = col_name.clone();
                        let data_type = data_type.clone();
                        let default_value = default_value.clone();
                        move |_window, _cx| {
                            v_flex()
                                .gap_1()
                                .child(div().font_weight(FontWeight::BOLD).child(col_name.clone()))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child("Type:")
                                        .child(data_type.clone())
                                        .when_some(max_length, |this, len| {
                                            this.child(format!("({})", len))
                                        }),
                                )
                                .child(h_flex().gap_2().child("Nullable:").child(if is_nullable {
                                    "Yes"
                                } else {
                                    "No"
                                }))
                                .when_some(default_value.clone(), |this, default| {
                                    this.child(h_flex().gap_2().child("Default:").child(default))
                                })
                        }
                    })
                    .build(window, cx)
                })
            })
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
                    })
                    .when(is_nullable && !is_pk, |this| {
                        this.child(
                            Icon::new(IconName::Asterisk)
                                .size(px(10.))
                                .text_color(cx.theme().green),
                        )
                    }),
            )
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

        let current_value: Option<String> = if is_edited {
            self.edit_state
                .get_edited_value(row_ix, col_ix)
                .and_then(|v| v.clone())
        } else {
            self.rows
                .get(row_ix)
                .and_then(|row| row.get(col_ix))
                .and_then(|v| v.clone())
        };

        // Check if the value is NULL (None = database NULL)
        let is_null = current_value.is_none();
        let display_text = if is_null {
            "NULL".to_string()
        } else {
            // Format multi-line values for display (replace newlines with visual indicators)
            format_value_for_display(current_value.as_deref().unwrap_or(""))
        };

        if is_editing {
            // Embed Input directly in the cell (not for row number column)
            if let Some(input) = self.edit_state.get_editing_input() {
                let is_expanded = self.edit_state.is_expanded(row_ix, col_ix);
                let is_json = self.column_types.get(col_ix - 1) == Some(&ColumnType::Json);
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
                                    .on_action(cx.listener(
                                        move |table,
                                              _event: &gpui_component::input::Escape,
                                              window,
                                              cx| {
                                            Self::handle_minimize(
                                                table,
                                                (col_ix, row_ix),
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
                                    .child(
                                        Input::new(&input)
                                            .disabled(!self.is_editable())
                                            .size_full()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(12.))
                                            .suffix(
                                                div()
                                                    .cursor_pointer()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(
                                                            move |table, _event, window, cx| {
                                                                Self::handle_minimize(
                                                                    table,
                                                                    (col_ix, row_ix),
                                                                    window,
                                                                    cx,
                                                                );
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
                        .when(
                            self.column_types
                                .get(col_ix - 1)
                                .is_some_and(|ct| ct.is_numeric()),
                            |this| this.justify_end(),
                        )
                        .when(
                            self.column_types.get(col_ix - 1) == Some(&ColumnType::Uuid),
                            |this| this.text_color(cx.theme().blue),
                        )
                        .when(
                            self.column_types.get(col_ix - 1) == Some(&ColumnType::DateTime),
                            |this| this.text_color(cx.theme().green),
                        )
                        .when(
                            self.column_types.get(col_ix - 1) == Some(&ColumnType::Json),
                            |this| this.text_color(cx.theme().yellow),
                        )
                        .when(
                            self.column_types.get(col_ix - 1) == Some(&ColumnType::Array),
                            |this| this.text_color(cx.theme().blue),
                        )
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
            let is_numeric = !is_row_number_col
                && self
                    .column_types
                    .get(col_ix - 1)
                    .is_some_and(|ct| ct.is_numeric());

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
                    self.table_columns
                        .get(col_ix - 1)
                        .and_then(|c| c.foreign_key.as_ref())
                        .filter(|_| !is_row_number_col),
                    |this, fk_info| {
                        let fk_info = fk_info.clone();
                        let cell_value = display_text.clone();
                        let popover_id = format!("fk-popover-{}-{}", row_ix, col_ix);
                        let connection_id = self.connection_id;
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

            let is_deleted = self.edit_state.is_row_deleted(row_ix);

            div()
                .group("")
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(12.))
                .size_full() // Fill the entire cell container
                .flex() // Enable flexbox layout
                .items_center() // Center vertically
                .when(is_deleted, |this| {
                    this.when(is_row_number_col, |this| {
                        this.font_weight(FontWeight::BOLD) // Bold row numbers
                            .text_color(cx.theme().muted_foreground) // Muted color for row numbers
                            .cursor_pointer() // Pointer cursor for row selection
                            .when(self.edit_state.is_new_row(row_ix), |this| {
                                this.border_l_3().border_color(cx.theme().red)
                            })
                    })
                })
                .when(!is_deleted, |this| {
                    this.when(is_row_number_col, |this| {
                        this.font_weight(FontWeight::BOLD) // Bold row numbers
                            .text_color(cx.theme().muted_foreground) // Muted color for row numbers
                            .cursor_pointer() // Pointer cursor for row selection
                            .when(self.edit_state.is_new_row(row_ix), |this| {
                                this.border_l_3().border_color(cx.theme().yellow)
                            })
                    })
                })
                .when(is_numeric && !is_row_number_col, |this| {
                    this.text_align(gpui::TextAlign::Right)
                        .justify_end() // Right-align numeric columns
                        .text_color(cx.theme().foreground) // Ensure numeric text is visible
                })
                .when(
                    !is_row_number_col
                        && self.column_types.get(col_ix - 1) == Some(&ColumnType::Uuid),
                    |this| {
                        this.text_color(cx.theme().blue) // Blue color for UUIDs
                    },
                )
                .when(
                    !is_row_number_col
                        && self.column_types.get(col_ix - 1) == Some(&ColumnType::DateTime),
                    |this| {
                        this.text_color(cx.theme().green) // Green color for timestamps
                    },
                )
                .when(
                    !is_row_number_col
                        && self.column_types.get(col_ix - 1) == Some(&ColumnType::Json),
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

        // Get the column type (adjust for row number column)
        let col_type = self
            .column_types
            .get(col_ix - 1)
            .copied()
            .unwrap_or(ColumnType::Unknown);
        let is_numeric = col_type.is_numeric();

        // Sort rows by the specified column
        self.rows.sort_by(|a, b| {
            let a_val = a.get(col_ix).and_then(|s| s.as_deref()).unwrap_or("");
            let b_val = b.get(col_ix).and_then(|s| s.as_deref()).unwrap_or("");

            let ordering = if is_numeric {
                compare_numeric(a_val, b_val)
            } else {
                a_val.cmp(b_val)
            };

            match sort {
                ColumnSort::Descending => ordering.reverse(),
                _ => ordering,
            }
        });

        // Update row numbers after sorting
        for (index, row) in self.rows.iter_mut().enumerate() {
            if let Some(row_num_cell) = row.get_mut(0) {
                *row_num_cell = Some((index + 1).to_string());
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
        cell: (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        // Check if the column is nullable (cell.1 is column index, 0 is row number column)
        let is_nullable = cell.1 > 0
            && self
                .table_columns
                .get(cell.1 - 1)
                .is_some_and(|c| c.is_nullable);

        menu.menu_with_icon(
            "Copy as CSV",
            Icon::new(IconName::Sheet),
            Box::new(CopyAsCSV),
        )
        .menu_with_icon(
            "Copy as JSON",
            Icon::new(IconName::Braces),
            Box::new(CopyAsJSON),
        )
        .menu_with_icon(
            "Copy as SQL",
            Icon::new(IconName::Database),
            Box::new(CopyAsSQL),
        )
        .menu_with_icon(
            "Copy as Markdown",
            Icon::new(IconName::Markdown),
            Box::new(CopyAsMarkdown),
        )
        .separator()
        // Export operations
        .menu_with_icon(
            "Export as CSV",
            Icon::new(IconName::File),
            Box::new(ExportAsCSV),
        )
        .menu_with_icon(
            "Export as JSON",
            Icon::new(IconName::File),
            Box::new(ExportAsJSON),
        )
        .menu_with_icon(
            "Export as SQL",
            Icon::new(IconName::File),
            Box::new(ExportAsSQL),
        )
        .menu_with_icon(
            "Export as Markdown",
            Icon::new(IconName::File),
            Box::new(ExportAsMarkdown),
        )
        .separator()
        // Row operations
        .menu_with_icon("Add Row", Icon::new(IconName::Plus), Box::new(AddRow))
        .menu_with_icon(
            "Duplicate Row",
            Icon::new(IconName::Copy),
            Box::new(DuplicateRow { row: cell.0 }),
        )
        .menu_with_icon(
            "Delete Row",
            Icon::new(IconName::Delete),
            Box::new(DeleteRow { row: cell.0 }),
        )
        .when(is_nullable, |this| {
            this.menu_with_icon(
                "Set NULL",
                Icon::new(IconName::CircleX),
                Box::new(SetCellNull {
                    row: cell.0,
                    col: cell.1,
                }),
            )
        })
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
        // Test starting editing with string value
        edit_state
            .original_values
            .insert((0, 0), Some("original".to_string()));

        assert!(edit_state.is_editing(0, 0));
        assert!(!edit_state.is_edited(0, 0));

        // Test updating value
        edit_state
            .edited_values
            .insert((0, 0), Some("modified".to_string()));
        assert!(edit_state.is_edited(0, 0));
        assert_eq!(
            edit_state.get_edited_value(0, 0),
            Some(&Some("modified".to_string()))
        );
        assert_eq!(
            edit_state.get_original_value(0, 0),
            Some(&Some("original".to_string()))
        );

        // Test clearing editing state
        edit_state.editing_cell = None;
        assert!(!edit_state.is_editing(0, 0));
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
            .insert((0, 0), Some("original".to_string()));
        edit_state
            .edited_values
            .insert((0, 0), Some("modified".to_string()));
        edit_state.pending_new_rows.push(0);

        // Clear all
        edit_state.clear_all();

        assert!(!edit_state.is_editing(0, 0));
        assert!(!edit_state.is_edited(0, 0));
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
        delegate.rows = vec![vec![
            Some("1".to_string()),
            Some("2".to_string()),
            Some("test".to_string()),
        ]];

        // Simulate editing the primary key column (id) from 2 to 4
        let row = 0;
        let col = 1; // id column

        // Store original value
        delegate
            .edit_state
            .original_values
            .insert((row, col), Some("2".to_string()));
        delegate
            .edit_state
            .edited_values
            .insert((row, col), Some("4".to_string()));

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
