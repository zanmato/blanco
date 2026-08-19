use std::collections::HashMap;

use super::ResultsTableDelegate;
use crate::results_panel::cell_edit_state::ChangeType;
use crate::results_panel::table_operations::{
    ColumnChange, OperationType, RowIdentifier, TableChangeOperation,
};

impl ResultsTableDelegate {
    pub fn primary_key_column_names(&self) -> Vec<&str> {
        self.table_columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| c.name.as_str())
            .collect()
    }

    pub(crate) fn primary_key_is_complete(&self) -> bool {
        let pk_names = self.primary_key_column_names();
        if pk_names.is_empty() {
            return false;
        }
        pk_names
            .iter()
            .all(|pk_name| self.columns.iter().any(|col| col.name.as_str() == *pk_name))
    }

    fn primary_key_column_indices(&self) -> Option<Vec<usize>> {
        if !self.primary_key_is_complete() {
            return None;
        }
        let pk_names = self.primary_key_column_names();
        let indices: Vec<usize> = pk_names
            .into_iter()
            .filter_map(|pk_name| {
                self.columns
                    .iter()
                    .position(|col| col.name.as_str() == pk_name)
            })
            .collect();
        if indices.len() == self.primary_key_column_names().len() {
            Some(indices)
        } else {
            None
        }
    }

    /// Get column names for INSERT operations, excluding primary key (for new
    /// rows) and untouched columns with a server-side default.
    pub fn get_insert_column_names(
        &self,
        row_index: usize,
        exclude_primary_key: bool,
    ) -> Vec<String> {
        let pk_indices = if exclude_primary_key {
            self.primary_key_column_indices()
        } else {
            None
        };

        self.columns
            .iter()
            .enumerate()
            .filter_map(|(data_index, col)| {
                if exclude_primary_key
                    && let Some(ref pk_indices) = pk_indices
                    && pk_indices.contains(&data_index)
                {
                    return None;
                }
                if self.cell_uses_default(row_index, data_index) {
                    return None;
                }
                Some(col.name.to_string())
            })
            .collect()
    }

    /// Get column values for INSERT operations, excluding primary key (for new
    /// rows) and untouched columns with a server-side default.
    pub fn get_insert_values(
        &self,
        row_index: usize,
        exclude_primary_key: bool,
    ) -> Vec<Option<String>> {
        let pk_indices = if exclude_primary_key {
            self.primary_key_column_indices()
        } else {
            None
        };

        if let Some(row) = self.rows.get(row_index) {
            row.iter()
                .enumerate()
                .filter_map(|(data_index, val)| {
                    if exclude_primary_key
                        && let Some(ref pk_indices) = pk_indices
                        && pk_indices.contains(&data_index)
                    {
                        return None; // Skip primary key column
                    }
                    if self.cell_uses_default(row_index, data_index) {
                        return None;
                    }

                    // Check if there's an edited value for this cell
                    if let Some(edited_value) =
                        self.edit_state.edited_values.get(&(row_index, data_index))
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

    /// Convert table changes to database-agnostic TableChangeOperations
    /// This method consolidates multiple changes to the same row into single operations.
    pub fn create_change_operations(&self) -> Vec<TableChangeOperation> {
        // Map to consolidate changes by (table_name, sorted PK pairs). `update_order`
        // records first-seen keys so the emitted UPDATEs keep a stable order; a plain
        // HashMap iteration would reshuffle them on every render of the SQL preview.
        type UpdateKey = (String, Vec<(String, String)>);
        let mut update_operations: HashMap<UpdateKey, Vec<ColumnChange>> = HashMap::new();
        let mut update_order: Vec<UpdateKey> = Vec::new();
        let mut insert_operations: Vec<TableChangeOperation> = Vec::new();
        let mut delete_operations: Vec<TableChangeOperation> = Vec::new();

        for change in &self.edit_state.changes {
            match change.change_type {
                ChangeType::DeleteRow => {
                    let pk_columns: Vec<(String, String)> = change
                        .primary_key_values
                        .iter()
                        .filter_map(|(col, val)| val.as_ref().map(|v| (col.clone(), v.clone())))
                        .collect();
                    if pk_columns.len() != change.primary_key_values.len() {
                        continue;
                    }

                    delete_operations.push(TableChangeOperation {
                        operation_type: OperationType::Delete,
                        table_name: change.table_name.clone(),
                        row_identifier: RowIdentifier::PrimaryKey {
                            columns: pk_columns,
                        },
                        changes: vec![],
                    });
                }
                ChangeType::UpdateCell => {
                    let pk_columns: Vec<(String, String)> = change
                        .primary_key_values
                        .iter()
                        .filter_map(|(col, val)| val.as_ref().map(|v| (col.clone(), v.clone())))
                        .collect();
                    if pk_columns.len() != change.primary_key_values.len() {
                        continue;
                    }

                    let column_name = self
                        .columns
                        .get(change.column_index.unwrap_or(0))
                        .map(|col| col.name.to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    let column_change = ColumnChange {
                        column_name,
                        new_value: change.new_value.clone(),
                    };

                    let mut sorted_pk = pk_columns.clone();
                    sorted_pk.sort_by(|a, b| a.0.cmp(&b.0));
                    let key = (change.table_name.clone(), sorted_pk);
                    if !update_operations.contains_key(&key) {
                        update_order.push(key.clone());
                    }
                    update_operations
                        .entry(key)
                        .or_default()
                        .push(column_change);
                }
                ChangeType::InsertRow => {
                    let pk_col_indices = self.primary_key_column_indices();
                    let exclude_primary_key = pk_col_indices.as_ref().is_some_and(|indices| {
                        indices.iter().all(|&idx| {
                            let pk_value = self
                                .edit_state
                                .edited_values
                                .get(&(change.row_index, idx))
                                .or_else(|| {
                                    self.rows.get(change.row_index).and_then(|row| row.get(idx))
                                });
                            pk_value.is_none_or(|v| v.as_ref().is_none_or(|s| s.is_empty()))
                        })
                    });

                    let column_names =
                        self.get_insert_column_names(change.row_index, exclude_primary_key);
                    let row_values = self.get_insert_values(change.row_index, exclude_primary_key);

                    let column_changes: Vec<ColumnChange> = column_names
                        .into_iter()
                        .zip(row_values.iter())
                        .map(|(column_name, value)| ColumnChange {
                            column_name,
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

        let mut operations = Vec::new();
        for key in update_order {
            let Some(column_changes) = update_operations.remove(&key) else {
                continue;
            };
            let (table_name, pk_columns) = key;
            operations.push(TableChangeOperation {
                operation_type: OperationType::Update,
                table_name,
                row_identifier: RowIdentifier::PrimaryKey {
                    columns: pk_columns,
                },
                changes: column_changes,
            });
        }

        operations.extend(insert_operations);
        operations.extend(delete_operations);

        operations
    }
}
