//! Structure changes waiting for Apply, and what the table looks like with
//! them. The structure tab lists the replayed structure, so a column added
//! by one pending change can be indexed by the next.

use blanco_core::ddl::{
    ColumnOperation, ColumnSpec, DdlError, ForeignKeyOperation, IndexOperation,
};
use blanco_core::{ColumnInfo, DatabaseType, ForeignKeyConstraint, IndexInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StructureChange {
    Column(ColumnOperation),
    Index(IndexOperation),
    ForeignKey(ForeignKeyOperation),
}

impl StructureChange {
    /// The statements the change runs, built when it is staged so a change
    /// the backend cannot make is refused then rather than at Apply.
    pub(super) fn statements(
        &self,
        db_type: DatabaseType,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<String>, DdlError> {
        match self {
            StructureChange::Column(operation) => {
                blanco_core::ddl::column_operation_sql(db_type, schema, table, operation)
            }
            StructureChange::Index(operation) => {
                blanco_core::ddl::index_operation_sql(db_type, schema, table, operation)
            }
            StructureChange::ForeignKey(operation) => {
                blanco_core::ddl::foreign_key_operation_sql(db_type, schema, table, operation)
            }
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            StructureChange::Column(operation) => operation.label(),
            StructureChange::Index(operation) => operation.label(),
            StructureChange::ForeignKey(operation) => operation.label(),
        }
    }

    /// The change with the object it touches, as the undo menu and the
    /// preview name it.
    pub(super) fn describe(&self) -> String {
        let label = self.label();
        match self {
            StructureChange::Column(ColumnOperation::Add(spec)) => {
                format!("{label} \"{}\"", spec.name)
            }
            StructureChange::Column(ColumnOperation::Rename { from, to }) => {
                format!("{label} \"{from}\" to \"{to}\"")
            }
            StructureChange::Column(
                ColumnOperation::Drop { name } | ColumnOperation::Alter { name, .. },
            )
            | StructureChange::Index(
                IndexOperation::Create { name, .. } | IndexOperation::Drop { name },
            )
            | StructureChange::ForeignKey(ForeignKeyOperation::Drop { name }) => {
                format!("{label} \"{name}\"")
            }
            StructureChange::ForeignKey(ForeignKeyOperation::Create { name, columns, .. }) => {
                match name {
                    Some(name) => format!("{label} \"{name}\""),
                    None => format!("{label} on ({})", columns.join(", ")),
                }
            }
        }
    }
}

/// A staged change and the statements it runs.
#[derive(Debug, Clone)]
pub(super) struct PendingChange {
    pub change: StructureChange,
    pub statements: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum RowState {
    #[default]
    Unchanged,
    Added,
    Modified,
    Dropped,
}

/// How pending changes touch one row of a structure list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RowMark {
    pub state: RowState,
    /// The pending changes on this row, as indices into the pending list
    /// with their descriptions.
    pub changes: Vec<(usize, String)>,
}

impl RowMark {
    fn touch(&mut self, change_ix: usize, change: &StructureChange, state: RowState) {
        self.changes.push((change_ix, change.describe()));
        self.state = match (self.state, state) {
            (RowState::Added, RowState::Modified) => RowState::Added,
            (_, state) => state,
        };
    }

    pub(super) fn is_live(&self) -> bool {
        self.state != RowState::Dropped
    }
}

/// The table as it will be once the pending changes are applied. Dropped
/// rows stay listed, marked, so their drop can be undone.
#[derive(Debug, Clone, Default)]
pub(super) struct StagedStructure {
    pub columns: Vec<ColumnInfo>,
    pub column_marks: Vec<RowMark>,
    pub indexes: Vec<IndexInfo>,
    pub index_marks: Vec<RowMark>,
    pub foreign_keys: Vec<ForeignKeyConstraint>,
    pub foreign_key_marks: Vec<RowMark>,
}

impl StagedStructure {
    /// The names of the columns the table will have, in table order.
    pub(super) fn live_column_names(&self) -> Vec<String> {
        self.columns
            .iter()
            .zip(&self.column_marks)
            .filter(|(_, mark)| mark.is_live())
            .map(|(column, _)| column.name.clone())
            .collect()
    }

    fn live_column(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .zip(&self.column_marks)
            .position(|(column, mark)| mark.is_live() && column.name == name)
    }

    fn live_index(&self, name: &str) -> Option<usize> {
        self.indexes
            .iter()
            .zip(&self.index_marks)
            .position(|(index, mark)| mark.is_live() && index.name == name)
    }

    fn live_foreign_key(&self, name: &str) -> Option<usize> {
        self.foreign_keys
            .iter()
            .zip(&self.foreign_key_marks)
            .position(|(foreign_key, mark)| mark.is_live() && foreign_key.name == name)
    }

    fn missing_columns(&self, columns: &[String]) -> Result<(), String> {
        match columns.iter().find(|name| self.live_column(name).is_none()) {
            Some(name) => Err(format!("No column named \"{name}\"")),
            None => Ok(()),
        }
    }

    fn apply(&mut self, change_ix: usize, change: &StructureChange) -> Result<(), String> {
        match change {
            StructureChange::Column(operation) => self.apply_column(change_ix, change, operation),
            StructureChange::Index(operation) => self.apply_index(change_ix, change, operation),
            StructureChange::ForeignKey(operation) => {
                self.apply_foreign_key(change_ix, change, operation)
            }
        }
    }

    fn apply_column(
        &mut self,
        change_ix: usize,
        change: &StructureChange,
        operation: &ColumnOperation,
    ) -> Result<(), String> {
        match operation {
            ColumnOperation::Add(spec) => {
                if self.live_column(&spec.name).is_some() {
                    return Err(format!("Column \"{}\" already exists", spec.name));
                }
                let mut mark = RowMark::default();
                mark.touch(change_ix, change, RowState::Added);
                self.columns.push(column_from_spec(spec));
                self.column_marks.push(mark);
            }
            ColumnOperation::Alter { name, spec } => {
                let ix = self
                    .live_column(name)
                    .ok_or_else(|| format!("No column named \"{name}\""))?;
                let column = &mut self.columns[ix];
                if column.data_type != spec.data_type {
                    column.data_type = spec.data_type.clone();
                    // The catalog's length and precision described the old type.
                    column.character_maximum_length = None;
                    column.numeric_precision = None;
                    column.numeric_scale = None;
                }
                column.is_nullable = spec.nullable;
                column.default_value = spec.default.clone();
                self.column_marks[ix].touch(change_ix, change, RowState::Modified);
            }
            ColumnOperation::Rename { from, to } => {
                let ix = self
                    .live_column(from)
                    .ok_or_else(|| format!("No column named \"{from}\""))?;
                if from != to && self.live_column(to).is_some() {
                    return Err(format!("Column \"{to}\" already exists"));
                }
                self.columns[ix].name = to.clone();
                self.column_marks[ix].touch(change_ix, change, RowState::Modified);
                // The server carries the rename into the table's indexes and
                // keys, so the lists show it too.
                let rename = |names: &mut Vec<String>| {
                    for name in names.iter_mut().filter(|name| *name == from) {
                        *name = to.clone();
                    }
                };
                self.indexes
                    .iter_mut()
                    .for_each(|index| rename(&mut index.column_names));
                self.foreign_keys
                    .iter_mut()
                    .for_each(|foreign_key| rename(&mut foreign_key.columns));
            }
            ColumnOperation::Drop { name } => {
                let ix = self
                    .live_column(name)
                    .ok_or_else(|| format!("No column named \"{name}\""))?;
                if self.column_marks[ix].state == RowState::Added {
                    return Err(format!(
                        "\"{name}\" is a new column, undo adding it instead"
                    ));
                }
                self.column_marks[ix].touch(change_ix, change, RowState::Dropped);
            }
        }
        Ok(())
    }

    fn apply_index(
        &mut self,
        change_ix: usize,
        change: &StructureChange,
        operation: &IndexOperation,
    ) -> Result<(), String> {
        match operation {
            IndexOperation::Create {
                name,
                columns,
                unique,
            } => {
                if self.live_index(name).is_some() {
                    return Err(format!("Index \"{name}\" already exists"));
                }
                self.missing_columns(columns)?;
                let mut mark = RowMark::default();
                mark.touch(change_ix, change, RowState::Added);
                self.indexes.push(IndexInfo {
                    name: name.clone(),
                    algorithm: String::new(),
                    is_unique: *unique,
                    column_names: columns.clone(),
                    condition: None,
                    comment: None,
                });
                self.index_marks.push(mark);
            }
            IndexOperation::Drop { name } => {
                let ix = self
                    .live_index(name)
                    .ok_or_else(|| format!("No index named \"{name}\""))?;
                if self.index_marks[ix].state == RowState::Added {
                    return Err(format!(
                        "\"{name}\" is a new index, undo creating it instead"
                    ));
                }
                self.index_marks[ix].touch(change_ix, change, RowState::Dropped);
            }
        }
        Ok(())
    }

    fn apply_foreign_key(
        &mut self,
        change_ix: usize,
        change: &StructureChange,
        operation: &ForeignKeyOperation,
    ) -> Result<(), String> {
        match operation {
            ForeignKeyOperation::Create {
                name,
                columns,
                referenced_schema,
                referenced_table,
                referenced_columns,
                on_update,
                on_delete,
            } => {
                if let Some(name) = name
                    && self.live_foreign_key(name).is_some()
                {
                    return Err(format!("Foreign key \"{name}\" already exists"));
                }
                self.missing_columns(columns)?;
                let mut mark = RowMark::default();
                mark.touch(change_ix, change, RowState::Added);
                self.foreign_keys.push(ForeignKeyConstraint {
                    name: name.clone().unwrap_or_default(),
                    columns: columns.clone(),
                    referenced_schema: referenced_schema.clone(),
                    referenced_table: referenced_table.clone(),
                    referenced_columns: referenced_columns.clone(),
                    on_update: on_update.sql().to_string(),
                    on_delete: on_delete.sql().to_string(),
                });
                self.foreign_key_marks.push(mark);
            }
            ForeignKeyOperation::Drop { name } => {
                let ix = self
                    .live_foreign_key(name)
                    .ok_or_else(|| format!("No foreign key named \"{name}\""))?;
                if self.foreign_key_marks[ix].state == RowState::Added {
                    return Err(format!(
                        "\"{name}\" is a new foreign key, undo creating it instead"
                    ));
                }
                self.foreign_key_marks[ix].touch(change_ix, change, RowState::Dropped);
            }
        }
        Ok(())
    }
}

fn column_from_spec(spec: &ColumnSpec) -> ColumnInfo {
    ColumnInfo {
        name: spec.name.clone(),
        data_type: spec.data_type.clone(),
        is_nullable: spec.nullable,
        is_primary_key: false,
        default_value: spec.default.clone(),
        character_maximum_length: None,
        foreign_key: None,
        comment: None,
        numeric_precision: None,
        numeric_scale: None,
    }
}

/// The result of replaying pending changes onto the loaded structure.
pub(super) struct Replay {
    pub structure: StagedStructure,
    /// Changes that no longer fit the structure before them (a rename of a
    /// column an earlier, undone change added), by index, with the reason.
    /// They are skipped, so the changes after them are replayed without them.
    pub rejected: Vec<(usize, String)>,
}

pub(super) fn replay<'a>(
    columns: &[ColumnInfo],
    indexes: &[IndexInfo],
    foreign_keys: &[ForeignKeyConstraint],
    changes: impl IntoIterator<Item = &'a StructureChange>,
) -> Replay {
    let mut structure = StagedStructure {
        columns: columns.to_vec(),
        column_marks: vec![RowMark::default(); columns.len()],
        indexes: indexes.to_vec(),
        index_marks: vec![RowMark::default(); indexes.len()],
        foreign_keys: foreign_keys.to_vec(),
        foreign_key_marks: vec![RowMark::default(); foreign_keys.len()],
    };
    let mut rejected = Vec::new();
    for (change_ix, change) in changes.into_iter().enumerate() {
        if let Err(reason) = structure.apply(change_ix, change) {
            rejected.push((change_ix, reason));
        }
    }
    Replay {
        structure,
        rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::{RowState, StructureChange, replay};
    use blanco_core::ddl::{
        ColumnOperation, ColumnSpec, ForeignKeyOperation, IndexOperation, ReferentialAction,
    };
    use blanco_core::{ColumnInfo, IndexInfo};

    fn column(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "varchar".to_string(),
            is_nullable: true,
            is_primary_key: false,
            default_value: None,
            character_maximum_length: Some(40),
            foreign_key: None,
            comment: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn spec(name: &str, data_type: &str) -> ColumnSpec {
        ColumnSpec {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable: false,
            default: None,
        }
    }

    fn base() -> (Vec<ColumnInfo>, Vec<IndexInfo>) {
        let columns = vec![column("id"), column("email")];
        let indexes = vec![IndexInfo {
            name: "users_email_idx".to_string(),
            algorithm: "btree".to_string(),
            is_unique: false,
            column_names: vec!["email".to_string()],
            condition: None,
            comment: None,
        }];
        (columns, indexes)
    }

    #[test]
    fn replay_marks_rows_and_lets_later_changes_use_earlier_ones() {
        let (columns, indexes) = base();
        let changes = [
            StructureChange::Column(ColumnOperation::Add(spec("age", "integer"))),
            StructureChange::Index(IndexOperation::Create {
                name: "users_age_idx".to_string(),
                columns: vec!["age".to_string()],
                unique: false,
            }),
            StructureChange::Column(ColumnOperation::Rename {
                from: "email".to_string(),
                to: "mail".to_string(),
            }),
            StructureChange::Column(ColumnOperation::Alter {
                name: "mail".to_string(),
                spec: spec("mail", "text"),
            }),
            StructureChange::Index(IndexOperation::Drop {
                name: "users_email_idx".to_string(),
            }),
        ];
        let replayed = replay(&columns, &indexes, &[], &changes);
        assert!(replayed.rejected.is_empty(), "{:?}", replayed.rejected);
        let structure = replayed.structure;

        assert_eq!(structure.live_column_names(), ["id", "mail", "age"]);
        let states: Vec<RowState> = structure.column_marks.iter().map(|m| m.state).collect();
        assert_eq!(
            states,
            [RowState::Unchanged, RowState::Modified, RowState::Added]
        );
        // The rename and the alter both sit on the row, for undo.
        let mail = &structure.column_marks[1];
        assert_eq!(
            mail.changes,
            [
                (2, "Rename column \"email\" to \"mail\"".to_string()),
                (3, "Alter column \"mail\"".to_string())
            ]
        );
        let mail_column = &structure.columns[1];
        assert_eq!(mail_column.data_type, "text");
        assert_eq!(mail_column.character_maximum_length, None);
        assert!(!mail_column.is_nullable);

        // The renamed column shows in the index on it, which is dropped.
        assert_eq!(structure.indexes[0].column_names, ["mail"]);
        assert_eq!(structure.index_marks[0].state, RowState::Dropped);
        assert_eq!(structure.indexes[1].name, "users_age_idx");
        assert_eq!(structure.index_marks[1].state, RowState::Added);
    }

    #[test]
    fn replay_rejects_changes_that_no_longer_fit() {
        let (columns, indexes) = base();
        // An index on a column whose addition was undone, then a change that
        // does not depend on it.
        let changes = [
            StructureChange::Index(IndexOperation::Create {
                name: "users_age_idx".to_string(),
                columns: vec!["age".to_string()],
                unique: false,
            }),
            StructureChange::Column(ColumnOperation::Add(spec("email", "text"))),
            StructureChange::Column(ColumnOperation::Drop {
                name: "id".to_string(),
            }),
            StructureChange::Column(ColumnOperation::Alter {
                name: "id".to_string(),
                spec: spec("id", "bigint"),
            }),
            StructureChange::ForeignKey(ForeignKeyOperation::Create {
                name: None,
                columns: vec!["missing".to_string()],
                referenced_schema: None,
                referenced_table: "accounts".to_string(),
                referenced_columns: vec!["id".to_string()],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::Cascade,
            }),
        ];
        let replayed = replay(&columns, &indexes, &[], &changes);
        let rejected: Vec<usize> = replayed.rejected.iter().map(|(ix, _)| *ix).collect();
        assert_eq!(rejected, [0, 1, 3, 4]);
        assert_eq!(replayed.rejected[1].1, "Column \"email\" already exists");
        assert_eq!(replayed.structure.column_marks[0].state, RowState::Dropped);
        assert_eq!(replayed.structure.live_column_names(), ["email"]);
    }

    #[test]
    fn a_pending_addition_is_undone_rather_than_dropped() {
        let (columns, indexes) = base();
        let changes = [
            StructureChange::Column(ColumnOperation::Add(spec("age", "integer"))),
            StructureChange::Column(ColumnOperation::Drop {
                name: "age".to_string(),
            }),
        ];
        let replayed = replay(&columns, &indexes, &[], &changes);
        assert_eq!(replayed.rejected.len(), 1);
        assert_eq!(replayed.rejected[0].0, 1);
        assert_eq!(replayed.structure.column_marks[2].state, RowState::Added);
    }
}
