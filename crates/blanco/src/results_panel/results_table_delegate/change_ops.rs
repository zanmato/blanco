use std::collections::HashMap;

use blanco_core::ColumnType;

use super::ResultsTableDelegate;
use crate::results_panel::cell_edit_state::ChangeType;
use crate::results_panel::table_operations::{
    ColumnChange, ColumnValue, OperationType, RowIdentifier, TableChangeOperation,
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

    pub(crate) fn primary_key_column_indices(&self) -> Option<Vec<usize>> {
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

    pub(crate) fn column_type(&self, column_index: usize) -> ColumnType {
        self.column_types
            .get(column_index)
            .copied()
            .unwrap_or(ColumnType::Unknown)
    }

    /// Primary key values as they go into a WHERE clause, typed by their
    /// column so a binary key is compared as bytes.
    fn key_values(&self, key: Vec<(String, String)>) -> Vec<(String, ColumnValue)> {
        key.into_iter()
            .map(|(name, value)| {
                let column_type = self
                    .columns
                    .iter()
                    .position(|column| column.name.as_str() == name)
                    .map_or(ColumnType::Unknown, |index| self.column_type(index));
                let value = ColumnValue::data(value, column_type);
                (name, value)
            })
            .collect()
    }

    /// The columns an INSERT of this row names: every column except the
    /// primary key (when it is left to the server) and untouched columns with
    /// a server-side default.
    fn insert_column_indices(&self, row_index: usize, exclude_primary_key: bool) -> Vec<usize> {
        let pk_indices = if exclude_primary_key {
            self.primary_key_column_indices()
        } else {
            None
        };

        (0..self.columns.len())
            .filter(|data_index| {
                !pk_indices
                    .as_ref()
                    .is_some_and(|pk_indices| pk_indices.contains(data_index))
                    && !self.cell_uses_default(row_index, *data_index)
            })
            .collect()
    }

    pub fn get_insert_column_names(
        &self,
        row_index: usize,
        exclude_primary_key: bool,
    ) -> Vec<String> {
        self.insert_column_indices(row_index, exclude_primary_key)
            .into_iter()
            .filter_map(|data_index| Some(self.columns.get(data_index)?.name.to_string()))
            .collect()
    }

    pub fn get_insert_values(
        &self,
        row_index: usize,
        exclude_primary_key: bool,
    ) -> Vec<Option<String>> {
        if self.rows.get(row_index).is_none() {
            return Vec::new();
        }
        self.insert_column_indices(row_index, exclude_primary_key)
            .into_iter()
            .map(|data_index| self.stored_cell_value(row_index, data_index))
            .collect()
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
                            columns: self.key_values(pk_columns),
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

                    let column_index = change.column_index.unwrap_or(0);
                    let column_name = self
                        .columns
                        .get(column_index)
                        .map(|col| col.name.to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    let column_change = ColumnChange {
                        column_name,
                        value: ColumnValue::from_typed_cell(
                            change.new_value.clone(),
                            self.edit_state
                                .is_expression(change.row_index, column_index),
                            self.column_type(column_index),
                        ),
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
                    // An empty key is left to the server: omitted from the
                    // INSERT, or sent as an explicit NULL where the dialect
                    // generates keys that way.
                    let pk_col_indices = self.primary_key_column_indices();
                    let exclude_primary_key = !self.generated_key_is_null()
                        && pk_col_indices.as_ref().is_some_and(|indices| {
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

                    let column_changes: Vec<ColumnChange> = self
                        .insert_column_indices(change.row_index, exclude_primary_key)
                        .into_iter()
                        .filter_map(|data_index| {
                            Some(ColumnChange {
                                column_name: self.columns.get(data_index)?.name.to_string(),
                                value: ColumnValue::from_typed_cell(
                                    self.stored_cell_value(change.row_index, data_index),
                                    self.edit_state.is_expression(change.row_index, data_index),
                                    self.column_type(data_index),
                                ),
                            })
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
                    columns: self.key_values(pk_columns),
                },
                changes: column_changes,
            });
        }

        operations.extend(insert_operations);
        operations.extend(delete_operations);

        operations
    }
}
