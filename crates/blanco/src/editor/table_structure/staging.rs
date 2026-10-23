//! Structure changes waiting for Apply, and what the table looks like with
//! them. The structure tab lists the replayed structure, so a column added
//! by one pending change can be indexed by the next.

use blanco_core::ddl::{
    ColumnOperation, ColumnPosition, ColumnSpec, DdlError, ForeignKeyOperation, IndexOperation,
    TableChange,
};
use blanco_core::{ColumnDefinition, ColumnInfo, DatabaseType, ForeignKeyConstraint, IndexInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StructureChange {
    Column(ColumnOperation),
    Index(IndexOperation),
    ForeignKey(ForeignKeyOperation),
    Table(TableChange),
}

/// What a change does to the row it touches, for the pending changes
/// summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChangeKind {
    Add,
    Modify,
    Drop,
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
            StructureChange::Table(change) => {
                blanco_core::ddl::table_change_sql(db_type, schema, table, change)
            }
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            StructureChange::Column(operation) => operation.label(),
            StructureChange::Index(operation) => operation.label(),
            StructureChange::ForeignKey(operation) => operation.label(),
            StructureChange::Table(change) => change.label(),
        }
    }

    pub(super) fn kind(&self) -> ChangeKind {
        match self {
            StructureChange::Column(ColumnOperation::Add { .. })
            | StructureChange::Index(IndexOperation::Create(_))
            | StructureChange::ForeignKey(ForeignKeyOperation::Create { .. }) => ChangeKind::Add,
            StructureChange::Column(ColumnOperation::Drop { .. })
            | StructureChange::Index(IndexOperation::Drop { .. })
            | StructureChange::ForeignKey(ForeignKeyOperation::Drop { .. }) => ChangeKind::Drop,
            StructureChange::Column(
                ColumnOperation::Rename { .. }
                | ColumnOperation::Alter { .. }
                | ColumnOperation::Move { .. },
            )
            | StructureChange::Table(_) => ChangeKind::Modify,
        }
    }

    /// Follow the renaming of a column still to be added, whose addition an
    /// edit replaced, in a later change that names it. Its other changes
    /// were folded into the addition, so only these can. Returns whether the
    /// change named the column.
    pub(super) fn rename_added_column(&mut self, from: &str, to: &str) -> bool {
        let mut renamed = false;
        let mut rename = |name: &mut String| {
            if name == from {
                *name = to.to_string();
                renamed = true;
            }
        };
        match self {
            StructureChange::Column(ColumnOperation::Add {
                position: Some(ColumnPosition::After(name)),
                ..
            }) => rename(name),
            StructureChange::Column(ColumnOperation::Move { column, position }) => {
                rename(&mut column.name);
                if let ColumnPosition::After(name) = position {
                    rename(name);
                }
            }
            StructureChange::Index(IndexOperation::Create(spec)) => {
                spec.columns.iter_mut().for_each(&mut rename)
            }
            StructureChange::ForeignKey(ForeignKeyOperation::Create { columns, .. }) => {
                columns.iter_mut().for_each(&mut rename)
            }
            _ => {}
        }
        renamed
    }

    /// The change with the object it touches, as the undo menu and the
    /// preview name it.
    pub(super) fn describe(&self) -> String {
        let label = self.label();
        match self {
            StructureChange::Column(
                ColumnOperation::Add { column: spec, .. }
                | ColumnOperation::Move { column: spec, .. },
            ) => {
                format!("{label} \"{}\"", spec.name)
            }
            StructureChange::Column(ColumnOperation::Rename { from, to }) => {
                format!("{label} \"{from}\" to \"{to}\"")
            }
            StructureChange::Column(ColumnOperation::Alter { from, .. }) => {
                format!("{label} \"{}\"", from.name)
            }
            StructureChange::Index(IndexOperation::Create(spec)) => {
                format!("{label} \"{}\"", spec.name)
            }
            StructureChange::Column(ColumnOperation::Drop { name })
            | StructureChange::Index(IndexOperation::Drop { name })
            | StructureChange::ForeignKey(ForeignKeyOperation::Drop { name }) => {
                format!("{label} \"{name}\"")
            }
            StructureChange::Table(_) => label.to_string(),
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
    /// Each column as a change to it starts from, with what the listing
    /// leaves out (MySQL's full type and attributes) carried through renames.
    pub column_specs: Vec<ColumnSpec>,
    pub indexes: Vec<IndexInfo>,
    pub index_marks: Vec<RowMark>,
    pub foreign_keys: Vec<ForeignKeyConstraint>,
    pub foreign_key_marks: Vec<RowMark>,
    pub table_comment: Option<String>,
    pub table_mark: RowMark,
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

    pub(super) fn live_column(&self, name: &str) -> Option<usize> {
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

    /// Where a column placed at `position` lands among the columns.
    fn position_index(&self, position: Option<&ColumnPosition>) -> Result<usize, String> {
        match position {
            None => Ok(self.columns.len()),
            Some(ColumnPosition::First) => Ok(0),
            Some(ColumnPosition::After(name)) => self
                .live_column(name)
                .map(|ix| ix + 1)
                .ok_or_else(|| format!("No column named \"{name}\"")),
        }
    }

    fn missing_columns(&self, columns: &[String]) -> Result<(), String> {
        match columns.iter().find(|name| self.live_column(name).is_none()) {
            Some(name) => Err(format!("No column named \"{name}\"")),
            None => Ok(()),
        }
    }

    /// Apply `change`, and return it as it should be staged now when it
    /// restates a column that has changed since it was staged.
    fn apply(
        &mut self,
        change_ix: usize,
        change: &StructureChange,
    ) -> Result<Option<StructureChange>, String> {
        match change {
            StructureChange::Column(operation) => self.apply_column(change_ix, change, operation),
            StructureChange::Index(operation) => self
                .apply_index(change_ix, change, operation)
                .map(|()| None),
            StructureChange::ForeignKey(operation) => self
                .apply_foreign_key(change_ix, change, operation)
                .map(|()| None),
            StructureChange::Table(TableChange::Comment(comment)) => {
                self.table_comment = comment.clone();
                self.table_mark.touch(change_ix, change, RowState::Modified);
                Ok(None)
            }
        }
    }

    fn apply_column(
        &mut self,
        change_ix: usize,
        change: &StructureChange,
        operation: &ColumnOperation,
    ) -> Result<Option<StructureChange>, String> {
        let mut refreshed = None;
        match operation {
            ColumnOperation::Add {
                column: spec,
                position,
            } => {
                if self.live_column(&spec.name).is_some() {
                    return Err(format!("Column \"{}\" already exists", spec.name));
                }
                let ix = self.position_index(position.as_ref())?;
                let mut mark = RowMark::default();
                mark.touch(change_ix, change, RowState::Added);
                self.columns.insert(ix, column_from_spec(spec));
                self.column_marks.insert(ix, mark);
                self.column_specs.insert(ix, spec.clone());
            }
            ColumnOperation::Move { column, position } => {
                let ix = self
                    .live_column(&column.name)
                    .ok_or_else(|| format!("No column named \"{}\"", column.name))?;
                if matches!(position, ColumnPosition::After(name) if *name == column.name) {
                    return Err("A column cannot follow itself".to_string());
                }
                // The move restates the column as it is at this point, which
                // an edit or undo of an earlier change may have changed.
                if self.column_specs[ix] != *column {
                    refreshed = Some(StructureChange::Column(ColumnOperation::Move {
                        column: self.column_specs[ix].clone(),
                        position: position.clone(),
                    }));
                }
                let moved = self.columns.remove(ix);
                let mut mark = self.column_marks.remove(ix);
                let spec = self.column_specs.remove(ix);
                let target = match self.position_index(Some(position)) {
                    Ok(target) => target,
                    Err(reason) => {
                        self.columns.insert(ix, moved);
                        self.column_marks.insert(ix, mark);
                        self.column_specs.insert(ix, spec);
                        return Err(reason);
                    }
                };
                mark.touch(change_ix, change, RowState::Modified);
                self.columns.insert(target, moved);
                self.column_marks.insert(target, mark);
                self.column_specs.insert(target, spec);
            }
            ColumnOperation::Alter { from, to } => {
                let ix = self
                    .live_column(&from.name)
                    .ok_or_else(|| format!("No column named \"{}\"", from.name))?;
                if from.name != to.name && self.live_column(&to.name).is_some() {
                    return Err(format!("Column \"{}\" already exists", to.name));
                }
                self.rename_column_references(&from.name, &to.name);
                let column = &mut self.columns[ix];
                column.name = to.name.clone();
                if from.data_type != to.data_type {
                    column.data_type = to.data_type.clone();
                    // The catalog's length and precision described the old type.
                    column.character_maximum_length = None;
                    column.numeric_precision = None;
                    column.numeric_scale = None;
                }
                column.is_nullable = to.nullable;
                column.default_value = to.default.clone();
                column.comment = to.comment.clone();
                self.column_specs[ix] = to.clone();
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
                self.column_specs[ix].name = to.clone();
                self.column_marks[ix].touch(change_ix, change, RowState::Modified);
                self.rename_column_references(from, to);
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
        Ok(refreshed)
    }

    /// The server carries a column's rename into the table's indexes and
    /// keys, so the lists show it too.
    fn rename_column_references(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        let rename = |names: &mut Vec<String>| {
            for name in names.iter_mut().filter(|name| *name == from) {
                *name = to.to_string();
            }
        };
        self.indexes
            .iter_mut()
            .for_each(|index| rename(&mut index.column_names));
        self.foreign_keys
            .iter_mut()
            .for_each(|foreign_key| rename(&mut foreign_key.columns));
    }

    fn apply_index(
        &mut self,
        change_ix: usize,
        change: &StructureChange,
        operation: &IndexOperation,
    ) -> Result<(), String> {
        match operation {
            IndexOperation::Create(spec) => {
                if self.live_index(&spec.name).is_some() {
                    return Err(format!("Index \"{}\" already exists", spec.name));
                }
                self.missing_columns(&spec.columns)?;
                let mut mark = RowMark::default();
                mark.touch(change_ix, change, RowState::Added);
                self.indexes.push(IndexInfo {
                    name: spec.name.clone(),
                    algorithm: spec.method.clone().unwrap_or_default(),
                    is_unique: spec.unique,
                    column_names: spec.columns.clone(),
                    condition: spec
                        .condition
                        .clone()
                        .filter(|condition| !condition.is_empty()),
                    comment: None,
                    nulls_not_distinct: spec.nulls_not_distinct,
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

/// A listed column as the column editor starts from it, and as an alter
/// names what it changes. Where the backend reports a column's full
/// definition apart (MySQL), the type and attributes come from there.
pub(super) fn column_spec(column: &ColumnInfo, definitions: &[ColumnDefinition]) -> ColumnSpec {
    let definition = definitions
        .iter()
        .find(|definition| definition.name == column.name);
    ColumnSpec {
        name: column.name.clone(),
        data_type: definition
            .map(|definition| definition.column_type.clone())
            .unwrap_or_else(|| column.display_type()),
        nullable: column.is_nullable,
        default: column.default_value.clone(),
        comment: column.comment.clone(),
        attributes: definition.and_then(|definition| definition.attributes.clone()),
        generated: definition.is_some_and(|definition| definition.generated),
        primary_key: column.is_primary_key,
    }
}

fn column_from_spec(spec: &ColumnSpec) -> ColumnInfo {
    ColumnInfo {
        name: spec.name.clone(),
        data_type: spec.data_type.clone(),
        is_nullable: spec.nullable,
        is_primary_key: spec.primary_key,
        default_value: spec.default.clone(),
        character_maximum_length: None,
        foreign_key: None,
        comment: spec.comment.clone(),
        numeric_precision: None,
        numeric_scale: None,
        enum_labels: Vec::new(),
    }
}

/// The result of replaying pending changes onto the loaded structure.
pub(super) struct Replay {
    pub structure: StagedStructure,
    /// Moves that restate a column changed since they were staged, by index,
    /// as they should be staged now.
    pub refreshed: Vec<(usize, StructureChange)>,
    /// Changes that no longer fit the structure before them (a rename of a
    /// column an earlier, undone change added), by index, with the reason.
    /// They are skipped, so the changes after them are replayed without them.
    pub rejected: Vec<(usize, String)>,
}

/// The structure as the server last reported it.
#[derive(Debug, Clone, Copy)]
pub(super) struct LoadedStructure<'a> {
    pub columns: &'a [ColumnInfo],
    /// The full column definitions where the backend reports them apart
    /// from the listing (MySQL).
    pub column_definitions: &'a [ColumnDefinition],
    pub indexes: &'a [IndexInfo],
    pub foreign_keys: &'a [ForeignKeyConstraint],
    pub table_comment: Option<&'a str>,
}

pub(super) fn replay<'a>(
    loaded: LoadedStructure<'_>,
    changes: impl IntoIterator<Item = &'a StructureChange>,
) -> Replay {
    let mut structure = StagedStructure {
        columns: loaded.columns.to_vec(),
        column_marks: vec![RowMark::default(); loaded.columns.len()],
        column_specs: loaded
            .columns
            .iter()
            .map(|column| column_spec(column, loaded.column_definitions))
            .collect(),
        indexes: loaded.indexes.to_vec(),
        index_marks: vec![RowMark::default(); loaded.indexes.len()],
        foreign_keys: loaded.foreign_keys.to_vec(),
        foreign_key_marks: vec![RowMark::default(); loaded.foreign_keys.len()],
        table_comment: loaded.table_comment.map(str::to_string),
        table_mark: RowMark::default(),
    };
    let mut rejected = Vec::new();
    let mut refreshed = Vec::new();
    for (change_ix, change) in changes.into_iter().enumerate() {
        match structure.apply(change_ix, change) {
            Ok(Some(change)) => refreshed.push((change_ix, change)),
            Ok(None) => {}
            Err(reason) => rejected.push((change_ix, reason)),
        }
    }
    Replay {
        structure,
        refreshed,
        rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::{LoadedStructure, RowState, StructureChange, column_spec, replay};
    use blanco_core::ddl::{
        ColumnOperation, ColumnPosition, ColumnSpec, ForeignKeyOperation, IndexOperation,
        IndexSpec, ReferentialAction, TableChange,
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
            enum_labels: Vec::new(),
        }
    }

    fn spec(name: &str, data_type: &str) -> ColumnSpec {
        ColumnSpec {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable: false,
            default: None,
            ..ColumnSpec::default()
        }
    }

    fn loaded<'a>(columns: &'a [ColumnInfo], indexes: &'a [IndexInfo]) -> LoadedStructure<'a> {
        LoadedStructure {
            columns,
            column_definitions: &[],
            indexes,
            foreign_keys: &[],
            table_comment: None,
        }
    }

    fn index(name: &str, column: &str) -> IndexOperation {
        IndexOperation::Create(IndexSpec {
            name: name.to_string(),
            columns: vec![column.to_string()],
            ..IndexSpec::default()
        })
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
            nulls_not_distinct: false,
        }];
        (columns, indexes)
    }

    #[test]
    fn replay_marks_rows_and_lets_later_changes_use_earlier_ones() {
        let (columns, indexes) = base();
        let changes = [
            StructureChange::Column(ColumnOperation::Add {
                column: spec("age", "integer"),
                position: None,
            }),
            StructureChange::Index(index("users_age_idx", "age")),
            StructureChange::Column(ColumnOperation::Rename {
                from: "email".to_string(),
                to: "mail".to_string(),
            }),
            StructureChange::Column(ColumnOperation::Alter {
                from: column_spec(&column("mail"), &[]),
                to: spec("mail", "text"),
            }),
            StructureChange::Index(IndexOperation::Drop {
                name: "users_email_idx".to_string(),
            }),
        ];
        let replayed = replay(loaded(&columns, &indexes), &changes);
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
            StructureChange::Index(index("users_age_idx", "age")),
            StructureChange::Column(ColumnOperation::Add {
                column: spec("email", "text"),
                position: None,
            }),
            StructureChange::Column(ColumnOperation::Drop {
                name: "id".to_string(),
            }),
            StructureChange::Column(ColumnOperation::Alter {
                from: column_spec(&column("id"), &[]),
                to: spec("id", "bigint"),
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
        let replayed = replay(loaded(&columns, &indexes), &changes);
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
            StructureChange::Column(ColumnOperation::Add {
                column: spec("age", "integer"),
                position: None,
            }),
            StructureChange::Column(ColumnOperation::Drop {
                name: "age".to_string(),
            }),
        ];
        let replayed = replay(loaded(&columns, &indexes), &changes);
        assert_eq!(replayed.rejected.len(), 1);
        assert_eq!(replayed.rejected[0].0, 1);
        assert_eq!(replayed.structure.column_marks[2].state, RowState::Added);
    }

    #[test]
    fn alter_renames_references_and_table_comment_is_staged() {
        let (columns, indexes) = base();
        let changes = [
            StructureChange::Column(ColumnOperation::Alter {
                from: column_spec(&columns[1], &[]),
                to: ColumnSpec {
                    name: "mail".to_string(),
                    comment: Some("Where receipts go".to_string()),
                    ..column_spec(&columns[1], &[])
                },
            }),
            StructureChange::Table(TableChange::Comment(Some("People".to_string()))),
        ];
        let replayed = replay(loaded(&columns, &indexes), &changes);
        assert!(replayed.rejected.is_empty(), "{:?}", replayed.rejected);
        let structure = replayed.structure;
        assert_eq!(structure.columns[1].name, "mail");
        assert_eq!(
            structure.columns[1].comment.as_deref(),
            Some("Where receipts go")
        );
        // The type is unchanged, so its length stays.
        assert_eq!(structure.columns[1].character_maximum_length, Some(40));
        assert_eq!(structure.indexes[0].column_names, ["mail"]);
        assert_eq!(structure.table_comment.as_deref(), Some("People"));
        assert_eq!(structure.table_mark.state, RowState::Modified);
    }

    fn after(name: &str) -> ColumnPosition {
        ColumnPosition::After(name.to_string())
    }

    #[test]
    fn positions_and_moves_reorder_the_columns() {
        let (columns, indexes) = base();
        let email = column_spec(&columns[1], &[]);
        let changes = [
            StructureChange::Column(ColumnOperation::Add {
                column: spec("age", "integer"),
                position: Some(ColumnPosition::First),
            }),
            StructureChange::Column(ColumnOperation::Add {
                column: spec("name", "text"),
                position: Some(after("id")),
            }),
            StructureChange::Column(ColumnOperation::Move {
                column: email.clone(),
                position: after("age"),
            }),
        ];
        let replayed = replay(loaded(&columns, &indexes), &changes);
        assert!(replayed.rejected.is_empty(), "{:?}", replayed.rejected);
        assert!(replayed.refreshed.is_empty());
        let structure = replayed.structure;
        assert_eq!(
            structure.live_column_names(),
            ["age", "email", "id", "name"]
        );
        let states: Vec<RowState> = structure.column_marks.iter().map(|m| m.state).collect();
        assert_eq!(
            states,
            [
                RowState::Added,
                RowState::Modified,
                RowState::Unchanged,
                RowState::Added
            ]
        );
        let names: Vec<&str> = structure
            .column_specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        assert_eq!(names, ["age", "email", "id", "name"]);
        assert_eq!(
            structure.column_marks[1].changes,
            [(2, "Move column \"email\"".to_string())]
        );

        // A dropped column is no place to follow.
        let changes = [
            StructureChange::Column(ColumnOperation::Drop {
                name: "id".to_string(),
            }),
            StructureChange::Column(ColumnOperation::Move {
                column: email,
                position: after("id"),
            }),
        ];
        let replayed = replay(loaded(&columns, &indexes), &changes);
        assert_eq!(replayed.rejected.len(), 1);
        assert_eq!(replayed.rejected[0].0, 1);
        assert_eq!(replayed.structure.columns[1].name, "email");
    }

    #[test]
    fn a_move_restates_the_column_as_it_is_then() {
        let (columns, indexes) = base();
        let altered = spec("email", "text");
        let alter = StructureChange::Column(ColumnOperation::Alter {
            from: column_spec(&columns[1], &[]),
            to: altered.clone(),
        });
        let moved = StructureChange::Column(ColumnOperation::Move {
            column: altered,
            position: ColumnPosition::First,
        });
        let replayed = replay(loaded(&columns, &indexes), [&alter, &moved]);
        assert!(replayed.refreshed.is_empty());
        assert_eq!(replayed.structure.live_column_names(), ["email", "id"]);

        // With the alter undone, the move must not restate the altered type.
        let replayed = replay(loaded(&columns, &indexes), [&moved]);
        assert!(replayed.rejected.is_empty());
        assert_eq!(
            replayed.refreshed,
            [(
                0,
                StructureChange::Column(ColumnOperation::Move {
                    column: column_spec(&columns[1], &[]),
                    position: ColumnPosition::First,
                })
            )]
        );
    }

    #[test]
    fn later_changes_follow_a_renamed_addition() {
        let mut moved = StructureChange::Column(ColumnOperation::Move {
            column: spec("age", "integer"),
            position: after("age_group"),
        });
        assert!(moved.rename_added_column("age", "years"));
        assert_eq!(
            moved,
            StructureChange::Column(ColumnOperation::Move {
                column: spec("years", "integer"),
                position: after("age_group"),
            })
        );
        let mut indexed = StructureChange::Index(index("users_age_idx", "age"));
        assert!(indexed.rename_added_column("age", "years"));
        assert_eq!(
            indexed,
            StructureChange::Index(index("users_age_idx", "years"))
        );
        let mut placed = StructureChange::Column(ColumnOperation::Add {
            column: spec("note", "text"),
            position: Some(after("age")),
        });
        assert!(placed.rename_added_column("age", "years"));
        let mut unrelated = StructureChange::Column(ColumnOperation::Drop {
            name: "age".to_string(),
        });
        assert!(!unrelated.rename_added_column("age", "years"));
    }
}
