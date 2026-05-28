use gpui::{App, SharedString, TextRun, Window, px};
use gpui_component::{ActiveTheme, table::Column};

use blanco_core::{QueryResult, connection_trait::ColumnType};

use super::cell_edit_state::CellEditState;

mod cell_edit;
mod change_ops;
mod render;

#[derive(Default)]
pub struct ResultsTableDelegate {
    pub columns: Vec<Column>,
    pub column_types: Vec<ColumnType>,
    pub rows: Vec<Vec<Option<String>>>,
    pub edit_state: CellEditState,
    pub table_name: Option<String>,
    pub connection_id: i64,
    pub database_name: SharedString,
    pub db_type: Option<database::DatabaseType>,
    pub original_query: Option<String>,
    /// Column metadata for tooltips and rendering
    table_columns: Vec<blanco_core::ColumnInfo>,
}

impl ResultsTableDelegate {
    /// Remove a row at the specified index
    pub fn remove_row(&mut self, row_index: usize) {
        if row_index < self.rows.len() {
            self.rows.remove(row_index);
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

    pub fn clear_selection(&mut self) {
        self.edit_state.clear_selection();
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

        // Store table columns metadata for tooltips and rendering
        self.table_columns = result.table_columns.clone().unwrap_or_default();

        // Warn if PK columns are missing from the result set (disables editing)
        if !self.table_columns.is_empty() {
            let pk_names: Vec<&str> = self.primary_key_column_names();
            if !pk_names.is_empty() {
                let missing: Vec<&str> = pk_names
                    .iter()
                    .filter(|pk_name| {
                        !self
                            .columns
                            .iter()
                            .skip(1)
                            .any(|col| col.name.as_str() == **pk_name)
                    })
                    .copied()
                    .collect();
                if !missing.is_empty() {
                    tracing::warn!(
                        "Primary key column(s) missing from result set, editing disabled: {}",
                        missing.join(", ")
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results_panel::table_operations::{OperationType, RowIdentifier};

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
            edit_state.original_values.get(&(0, 0)),
            Some(&Some("original".to_string()))
        );

        // Test clearing editing state
        edit_state.editing_cell = None;
        assert!(!edit_state.is_editing(0, 0));
    }

    #[test]
    fn test_results_table_delegate_default() {
        let delegate = ResultsTableDelegate::default();

        assert!(delegate.table_name.is_none());
        assert!(!delegate.primary_key_is_complete());
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
        use blanco_core::ColumnInfo;

        let mut delegate = ResultsTableDelegate {
            table_name: Some("test_table".to_string()),
            table_columns: vec![
                ColumnInfo {
                    name: "id".to_string(),
                    data_type: "integer".to_string(),
                    is_nullable: false,
                    is_primary_key: true,
                    default_value: None,
                    character_maximum_length: None,
                    foreign_key: None,
                },
                ColumnInfo {
                    name: "name".to_string(),
                    data_type: "text".to_string(),
                    is_nullable: true,
                    is_primary_key: false,
                    default_value: None,
                    character_maximum_length: None,
                    foreign_key: None,
                },
            ],
            columns: vec![
                Column::new("row_number".to_string(), "#".to_string()),
                Column::new("id".to_string(), "id".to_string()),
                Column::new("name".to_string(), "name".to_string()),
            ],
            rows: vec![vec![
                Some("1".to_string()),
                Some("2".to_string()),
                Some("test".to_string()),
            ]],
            ..Default::default()
        };

        let row = 0;
        let col = 1;

        delegate
            .edit_state
            .original_values
            .insert((row, col), Some("2".to_string()));
        delegate
            .edit_state
            .edited_values
            .insert((row, col), Some("4".to_string()));

        delegate.commit_cell_edit(row, col);

        assert_eq!(delegate.edit_state.changes.len(), 1);
        let change = &delegate.edit_state.changes[0];

        assert_eq!(
            change.primary_key_values,
            vec![("id".to_string(), Some("2".to_string()))]
        );
        assert_eq!(change.old_value, Some("2".to_string()));
        assert_eq!(change.new_value, Some("4".to_string()));

        let operations = delegate.create_change_operations();
        assert_eq!(operations.len(), 1);

        if let OperationType::Update = &operations[0].operation_type {
            if let RowIdentifier::PrimaryKey { columns } = &operations[0].row_identifier {
                assert_eq!(columns, &[("id".to_string(), "2".to_string())]);
            } else {
                panic!("Expected PrimaryKey row identifier");
            }
        } else {
            panic!("Expected Update operation");
        }
    }
}
