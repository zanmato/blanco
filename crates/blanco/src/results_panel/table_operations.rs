// Database-agnostic table change operations

#[derive(Clone, Debug)]
pub struct TableChangeOperation {
    pub operation_type: OperationType,
    pub table_name: String,
    pub row_identifier: RowIdentifier,
    pub changes: Vec<ColumnChange>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OperationType {
    Insert,
    Update,
    Delete,
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationType::Insert => write!(f, "INSERT"),
            OperationType::Update => write!(f, "UPDATE"),
            OperationType::Delete => write!(f, "DELETE"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum RowIdentifier {
    PrimaryKey { column: String, value: String },
    #[allow(dead_code)]
    RowIndex(usize), // For cases without clear PK
}

#[derive(Clone, Debug)]
pub struct ColumnChange {
    pub column_name: String,
    #[allow(dead_code)]
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

impl TableChangeOperation {
    #[allow(dead_code)]
    pub fn new(
        operation_type: OperationType,
        table_name: String,
        row_identifier: RowIdentifier,
        changes: Vec<ColumnChange>,
    ) -> Self {
        Self {
            operation_type,
            table_name,
            row_identifier,
            changes,
        }
    }

    pub fn update_cell(
        table_name: String,
        pk_column: String,
        pk_value: String,
        column_name: String,
        old_value: Option<String>,
        new_value: Option<String>,
    ) -> Self {
        Self {
            operation_type: OperationType::Update,
            table_name,
            row_identifier: RowIdentifier::PrimaryKey { column: pk_column, value: pk_value },
            changes: vec![ColumnChange { column_name, old_value, new_value }],
        }
    }

    pub fn insert_row(
        table_name: String,
        column_changes: Vec<ColumnChange>,
    ) -> Self {
        Self {
            operation_type: OperationType::Insert,
            table_name,
            row_identifier: RowIdentifier::RowIndex(0), // Will be determined after insertion
            changes: column_changes,
        }
    }

    pub fn delete_row(
        table_name: String,
        pk_column: String,
        pk_value: String,
    ) -> Self {
        Self {
            operation_type: OperationType::Delete,
            table_name,
            row_identifier: RowIdentifier::PrimaryKey { column: pk_column, value: pk_value },
            changes: vec![],
        }
    }

    /// Generate the actual SQL query for logging purposes
    pub fn to_sql_query(&self) -> String {
        match self.operation_type {
            OperationType::Update => {
                if let RowIdentifier::PrimaryKey { column: pk_column, value: pk_value } = &self.row_identifier {
                    if self.changes.is_empty() {
                        format!("UPDATE {}", self.table_name)
                    } else if self.changes.len() == 1 {
                        // Single column change
                        let change = &self.changes[0];
                        format!(
                            "UPDATE {} SET {} = '{}' WHERE {} = '{}'",
                            self.table_name,
                            change.column_name,
                            change.new_value.as_ref().unwrap_or(&String::new()),
                            pk_column,
                            pk_value
                        )
                    } else {
                        // Multiple column changes
                        let set_clauses: Vec<String> = self.changes
                            .iter()
                            .map(|change| {
                                format!(
                                    "{} = '{}'",
                                    change.column_name,
                                    change.new_value.as_ref().unwrap_or(&String::new())
                                )
                            })
                            .collect();
                        format!(
                            "UPDATE {} SET {} WHERE {} = '{}'",
                            self.table_name,
                            set_clauses.join(", "),
                            pk_column,
                            pk_value
                        )
                    }
                } else {
                    format!("UPDATE {}", self.table_name)
                }
            }
            OperationType::Insert => {
                let columns: Vec<String> = self.changes.iter().map(|c| c.column_name.clone()).collect();
                let values: Vec<String> = self.changes.iter()
                    .map(|c| format!("'{}'", c.new_value.as_ref().unwrap_or(&String::new())))
                    .collect();
                format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    self.table_name,
                    columns.join(", "),
                    values.join(", ")
                )
            }
            OperationType::Delete => {
                if let RowIdentifier::PrimaryKey { column: pk_column, value: pk_value } = &self.row_identifier {
                    format!(
                        "DELETE FROM {} WHERE {} = '{}'",
                        self.table_name,
                        pk_column,
                        pk_value
                    )
                } else {
                    format!("DELETE FROM {}", self.table_name)
                }
            }
        }
    }
}