use gpui::{App, SharedString, TextRun, Window, px};
use gpui_component::{ActiveTheme, table::Column};

use blanco_core::{QueryResult, connection_trait::ColumnType};

use super::cell_edit_state::CellEditState;
use super::compare::CompareKind;

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
    /// Mirrors the panel wide pending comparison selection so the context
    /// menu can offer "Compare with Selected" for the matching kind.
    pub compare_selection_kind: Option<CompareKind>,
    /// Column metadata for tooltips and rendering
    table_columns: Vec<blanco_core::ColumnInfo>,
    /// The full result while a text filter is active. `rows` then holds only
    /// the matching subset, which keeps every row-index based code path
    /// (selection, editing, sorting) unaware of filtering. `None` when no
    /// filter is applied.
    unfiltered_rows: Option<Vec<Vec<Option<String>>>>,
}

impl ResultsTableDelegate {
    /// Whether the grid currently shows a filtered subset of the result.
    pub fn is_filtered(&self) -> bool {
        self.unfiltered_rows.is_some()
    }

    /// Number of rows in the full result, ignoring any active filter.
    pub fn unfiltered_row_count(&self) -> usize {
        self.unfiltered_rows
            .as_ref()
            .map_or(self.rows.len(), Vec::len)
    }

    /// Whether there are uncommitted edits. Filtering and sorting rearrange
    /// `rows`, and the edit state is keyed by row index, so both are blocked
    /// while this is true.
    pub fn has_pending_changes(&self) -> bool {
        !self.edit_state.changes.is_empty()
            || !self.edit_state.pending_new_rows.is_empty()
            || !self.edit_state.pending_deleted_rows.is_empty()
    }

    /// Restrict `rows` to those with any cell containing `needle`
    /// (case-insensitive). An empty needle restores the full result. Returns
    /// `false` without touching anything when edits are pending.
    pub fn apply_filter(&mut self, needle: &str) -> bool {
        if self.has_pending_changes() {
            return false;
        }
        let needle = needle.trim().to_lowercase();
        let all_rows = match self.unfiltered_rows.take() {
            Some(all_rows) => all_rows,
            None => std::mem::take(&mut self.rows),
        };
        if needle.is_empty() {
            self.rows = all_rows;
        } else {
            self.rows = all_rows
                .iter()
                .filter(|row| {
                    row.iter().any(|cell| {
                        cell.as_deref()
                            .is_some_and(|value| value.to_lowercase().contains(&needle))
                    })
                })
                .cloned()
                .collect();
            self.unfiltered_rows = Some(all_rows);
        }
        self.clear_selection();
        true
    }

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

    /// Table metadata for a result column, matched by name because the query's
    /// column order need not match the table's (`SELECT b, a FROM t`).
    pub fn table_column_info(&self, data_index: usize) -> Option<&blanco_core::ColumnInfo> {
        let column = self.columns.get(data_index)?;
        self.table_columns
            .iter()
            .find(|info| info.name.as_str() == column.name.as_str())
    }

    /// Whether a cell in a pending new row still holds its untouched initial
    /// NULL while the column has a server-side default. Such cells display as
    /// `DEFAULT` and are omitted from the generated INSERT so the default
    /// applies (SQLite has no `DEFAULT` keyword in a VALUES list, so omitting
    /// the column is the portable form).
    pub fn cell_uses_default(&self, row_index: usize, data_index: usize) -> bool {
        if !self.edit_state.is_new_row(row_index) {
            return false;
        }
        let untouched = !self
            .edit_state
            .edited_values
            .contains_key(&(row_index, data_index))
            && self
                .rows
                .get(row_index)
                .and_then(|row| row.get(data_index))
                .is_none_or(|value| value.is_none());
        // An untouched primary key cell is omitted from the INSERT so the
        // server generates the key, whether or not the column declares a
        // default. On SQLite the key is inserted as NULL instead and shows as
        // such.
        untouched
            && (self
                .table_column_info(data_index)
                .is_some_and(|info| info.default_value.is_some())
                || (!self.generated_key_is_null()
                    && self
                        .primary_key_column_indices()
                        .is_some_and(|indices| indices.contains(&data_index))))
    }

    pub(crate) fn generated_key_is_null(&self) -> bool {
        self.db_type
            .is_some_and(|db_type| db_type.dialect().generated_key_is_null())
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
        self.unfiltered_rows = None;

        // Store column types (move instead of clone to avoid memory leak)
        self.column_types = result.column_types;

        // Use the theme's font family for measurement (typically the mono font for tables)
        let text_size = px(12.);
        let font = gpui::font(cx.theme().mono_font_family.clone());

        // Pre-collect sample rows (cloned) to avoid borrow conflicts with result.rows
        let sample_rows: Vec<Vec<Option<String>>> = result.rows.iter().take(5).cloned().collect();

        // Calculate column widths based on actual text measurement
        let column_widths: Vec<f64> = result
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
                        // The grid shows one line per cell, and `shape_line`
                        // rejects newlines, so measure the widest line only.
                        let widest_line = cell_value
                            .lines()
                            .max_by_key(|line| line.len())
                            .unwrap_or_default();
                        let shared_value: SharedString = widest_line.to_string().into();
                        let shaped_line = window.text_system().shape_line(
                            shared_value,
                            text_size,
                            &[TextRun {
                                len: widest_line.len(),
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

        // Build columns from result with calculated widths. Row numbers are rendered
        // by the table's row header column, so there is no dedicated "#" data column.
        self.columns = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Column::new(format!("col_{}", i + 1), name)
                    .width(column_widths.get(i).copied().unwrap_or(150.0))
                    .resizable(true)
                    .sortable()
            })
            .collect();

        self.rows = result.rows;

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
        self.table_columns = result.table_columns.unwrap_or_default();

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
    use crate::results_panel::table_operations::{
        OperationType, RowIdentifier, TableChangeOperation,
    };
    use crate::results_panel::{ChangeType, TableChange};
    use blanco_core::ColumnInfo;

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

    fn rows(values: &[&str]) -> Vec<Vec<Option<String>>> {
        values
            .iter()
            .map(|value| vec![Some(value.to_string()), None])
            .collect()
    }

    #[test]
    fn apply_filter_narrows_and_restores_rows() {
        let mut delegate = ResultsTableDelegate {
            rows: rows(&["Alice", "Bob", "alina"]),
            ..Default::default()
        };

        assert!(delegate.apply_filter("ali"));
        assert!(delegate.is_filtered());
        assert_eq!(delegate.rows.len(), 2);
        assert_eq!(delegate.unfiltered_row_count(), 3);

        assert!(delegate.apply_filter("bob"));
        assert_eq!(
            delegate.rows.len(),
            1,
            "refining replaces the previous filter"
        );

        assert!(delegate.apply_filter("  "));
        assert!(!delegate.is_filtered());
        assert_eq!(delegate.rows, rows(&["Alice", "Bob", "alina"]));
    }

    #[test]
    fn apply_filter_is_blocked_while_edits_are_pending() {
        let mut delegate = ResultsTableDelegate {
            rows: rows(&["Alice", "Bob"]),
            ..Default::default()
        };
        delegate.edit_state.pending_new_rows.push(1);

        assert!(!delegate.apply_filter("ali"));
        assert!(!delegate.is_filtered());
        assert_eq!(delegate.rows.len(), 2);
    }

    #[test]
    fn new_result_clears_filter_state() {
        let mut delegate = ResultsTableDelegate {
            rows: rows(&["Alice", "Bob"]),
            ..Default::default()
        };
        assert!(delegate.apply_filter("ali"));
        delegate.rows.clear();
        delegate.unfiltered_rows = None;
        assert!(!delegate.is_filtered());
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
    fn test_cell_edit_state_clear_edits_drops_committed_changes() {
        let mut edit_state = CellEditState::default();

        edit_state
            .original_values
            .insert((0, 1), Some("original".to_string()));
        edit_state
            .edited_values
            .insert((0, 1), Some("modified".to_string()));
        edit_state.add_change(TableChange::new(
            ChangeType::UpdateCell,
            "test_table".to_string(),
            0,
            Some(1),
            Some("original".to_string()),
            Some("modified".to_string()),
            vec![("id".to_string(), Some("1".to_string()))],
            None,
        ));

        edit_state.clear_edits();

        assert!(!edit_state.is_edited(0, 1));
        assert!(
            edit_state.changes.is_empty(),
            "committed changes must not survive into the next commit"
        );
    }

    #[test]
    fn test_primary_key_update_preserves_original_value() {
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
                Column::new("id".to_string(), "id".to_string()),
                Column::new("name".to_string(), "name".to_string()),
            ],
            rows: vec![vec![Some("2".to_string()), Some("test".to_string())]],
            ..Default::default()
        };

        let row = 0;
        let col = 0;

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

    fn column_info(name: &str, is_primary_key: bool, default_value: Option<&str>) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "text".to_string(),
            is_nullable: !is_primary_key,
            is_primary_key,
            default_value: default_value.map(str::to_string),
            character_maximum_length: None,
            foreign_key: None,
        }
    }

    fn delegate_with_defaults() -> ResultsTableDelegate {
        ResultsTableDelegate {
            table_name: Some("items".to_string()),
            table_columns: vec![
                column_info("id", true, Some("nextval('items_id_seq')")),
                column_info("created_at", false, Some("now()")),
                column_info("name", false, None),
                column_info("note", false, None),
            ],
            columns: vec![
                Column::new("col_1".to_string(), "id".to_string()),
                Column::new("col_2".to_string(), "created_at".to_string()),
                Column::new("col_3".to_string(), "name".to_string()),
                Column::new("col_4".to_string(), "note".to_string()),
            ],
            rows: vec![vec![None, None, None, None]],
            ..Default::default()
        }
    }

    fn track_insert(delegate: &mut ResultsTableDelegate, row: usize) {
        delegate.edit_state.pending_new_rows.push(row);
        delegate.edit_state.add_change(TableChange::new(
            ChangeType::InsertRow,
            "items".to_string(),
            row,
            None,
            None,
            None,
            Vec::new(),
            None,
        ));
    }

    #[test]
    fn test_new_row_insert_omits_untouched_default_columns() {
        use database::DatabaseType;

        let mut delegate = delegate_with_defaults();
        track_insert(&mut delegate, 0);
        // The user only typed into "name"; "created_at" keeps its default,
        // "note" has no default and stays an explicit NULL.
        delegate
            .edit_state
            .edited_values
            .insert((0, 2), Some("gls".to_string()));

        let operations = delegate.create_change_operations();
        assert_eq!(operations.len(), 1);
        assert_eq!(
            operations[0].to_sql_query(DatabaseType::PostgreSQL),
            r#"INSERT INTO "items" ("name", "note") VALUES ('gls', NULL)"#
        );
    }

    #[test]
    fn test_new_row_with_empty_key_omits_it_or_sends_null_per_dialect() {
        use database::DatabaseType;

        let mut delegate = delegate_with_defaults();
        delegate.db_type = Some(DatabaseType::PostgreSQL);
        track_insert(&mut delegate, 0);
        delegate
            .edit_state
            .edited_values
            .insert((0, 2), Some("gls".to_string()));
        assert!(delegate.cell_uses_default(0, 0));
        let operations = delegate.create_change_operations();
        assert_eq!(
            operations[0].to_sql_query(DatabaseType::PostgreSQL),
            r#"INSERT INTO "items" ("name", "note") VALUES ('gls', NULL)"#
        );

        delegate.db_type = Some(DatabaseType::SQLite);
        delegate.table_columns[0].default_value = None;
        assert!(!delegate.cell_uses_default(0, 0));
        let operations = delegate.create_change_operations();
        assert_eq!(
            operations[0].to_sql_query(DatabaseType::SQLite),
            r#"INSERT INTO "items" ("id", "name", "note") VALUES (NULL, 'gls', NULL)"#
        );
    }

    #[test]
    fn test_new_row_insert_keeps_explicitly_nulled_default_column() {
        use database::DatabaseType;

        let mut delegate = delegate_with_defaults();
        track_insert(&mut delegate, 0);
        delegate
            .edit_state
            .edited_values
            .insert((0, 2), Some("gls".to_string()));
        // Explicit Set NULL on a defaulted column must survive as NULL.
        delegate.edit_state.edited_values.insert((0, 1), None);

        let operations = delegate.create_change_operations();
        assert_eq!(
            operations[0].to_sql_query(DatabaseType::PostgreSQL),
            r#"INSERT INTO "items" ("created_at", "name", "note") VALUES (NULL, 'gls', NULL)"#
        );
    }

    #[test]
    fn test_fully_untouched_row_emits_all_default_insert() {
        use database::DatabaseType;

        // Every non-PK column has a default, so an untouched row omits all
        // columns and falls back to the per-backend all-default INSERT form.
        let mut delegate = ResultsTableDelegate {
            table_name: Some("items".to_string()),
            table_columns: vec![
                column_info("id", true, Some("nextval('items_id_seq')")),
                column_info("created_at", false, Some("now()")),
            ],
            columns: vec![
                Column::new("col_1".to_string(), "id".to_string()),
                Column::new("col_2".to_string(), "created_at".to_string()),
            ],
            rows: vec![vec![None, None]],
            ..Default::default()
        };
        track_insert(&mut delegate, 0);

        let operations = delegate.create_change_operations();
        assert_eq!(
            operations[0].to_sql_query(DatabaseType::PostgreSQL),
            r#"INSERT INTO "items" DEFAULT VALUES"#
        );
        assert_eq!(
            operations[0].to_sql_query(DatabaseType::MySQL),
            "INSERT INTO `items` () VALUES ()"
        );
    }

    /// Consolidated UPDATE operations must come out in a stable, first-seen order
    /// across repeated calls. The SQL preview popover recomputes this on every
    /// render, so a HashMap-iteration order would visibly reshuffle the statements.
    #[test]
    fn test_update_operations_order_is_deterministic() {
        use crate::results_panel::cell_edit_state::{ChangeType, TableChange};

        let mut delegate = ResultsTableDelegate {
            table_name: Some("t".to_string()),
            columns: vec![
                Column::new("id".to_string(), "id".to_string()),
                Column::new("name".to_string(), "name".to_string()),
            ],
            ..Default::default()
        };

        // Updates to several distinct rows, recorded in a known order.
        for (row, pk) in [(0usize, "10"), (1, "20"), (2, "30"), (3, "40"), (4, "50")] {
            delegate.edit_state.add_change(TableChange::new(
                ChangeType::UpdateCell,
                "t".to_string(),
                row,
                Some(1), // "name" column
                Some("old".to_string()),
                Some(format!("new{pk}")),
                vec![("id".to_string(), Some(pk.to_string()))],
                None,
            ));
        }

        let pk_order = |ops: &[TableChangeOperation]| -> Vec<String> {
            ops.iter()
                .filter_map(|op| {
                    if let RowIdentifier::PrimaryKey { columns } = &op.row_identifier {
                        columns.first().map(|c| c.1.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        let expected = vec![
            "10".to_string(),
            "20".to_string(),
            "30".to_string(),
            "40".to_string(),
            "50".to_string(),
        ];
        assert_eq!(pk_order(&delegate.create_change_operations()), expected);

        // Recompute many times; the order must be identical every time.
        for _ in 0..50 {
            assert_eq!(pk_order(&delegate.create_change_operations()), expected);
        }
    }
}
