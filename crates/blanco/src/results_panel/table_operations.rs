// Database-agnostic table change operations

#[derive(Clone, Debug)]
pub struct TableChangeOperation {
    pub operation_type: OperationType,
    pub table_name: String,
    pub row_identifier: RowIdentifier,
    pub changes: Vec<ColumnChange>,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
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
    PrimaryKey {
        column: String,
        value: String,
    },
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

    pub fn insert_row(table_name: String, column_changes: Vec<ColumnChange>) -> Self {
        Self {
            operation_type: OperationType::Insert,
            table_name,
            row_identifier: RowIdentifier::RowIndex(0), // Will be determined after insertion
            changes: column_changes,
        }
    }

    /// Generate the actual SQL query for logging purposes
    pub fn to_sql_query(&self) -> String {
        // Helper function to escape single quotes in SQL values
        let escape_sql_value = |value: &str| value.replace('\'', "''");

        match self.operation_type {
            OperationType::Update => {
                if let RowIdentifier::PrimaryKey {
                    column: pk_column,
                    value: pk_value,
                } = &self.row_identifier
                {
                    if self.changes.is_empty() {
                        format!("UPDATE {}", self.table_name)
                    } else if self.changes.len() == 1 {
                        // Single column change
                        let change = &self.changes[0];
                        let set_value = match &change.new_value {
                            None => "NULL".to_string(),
                            Some(v) => format!("'{}'", escape_sql_value(v)),
                        };
                        let escaped_pk_value = escape_sql_value(pk_value);
                        format!(
                            "UPDATE {} SET {} = {} WHERE {} = '{}'",
                            self.table_name,
                            change.column_name,
                            set_value,
                            pk_column,
                            escaped_pk_value
                        )
                    } else {
                        // Multiple column changes
                        let set_clauses: Vec<String> = self
                            .changes
                            .iter()
                            .map(|change| {
                                let set_value = match &change.new_value {
                                    None => "NULL".to_string(),
                                    Some(v) => format!("'{}'", escape_sql_value(v)),
                                };
                                format!("{} = {}", change.column_name, set_value)
                            })
                            .collect();
                        let escaped_pk_value = escape_sql_value(pk_value);
                        format!(
                            "UPDATE {} SET {} WHERE {} = '{}'",
                            self.table_name,
                            set_clauses.join(", "),
                            pk_column,
                            escaped_pk_value
                        )
                    }
                } else {
                    format!("UPDATE {}", self.table_name)
                }
            }
            OperationType::Insert => {
                let columns: Vec<String> =
                    self.changes.iter().map(|c| c.column_name.clone()).collect();
                let values: Vec<String> = self
                    .changes
                    .iter()
                    .map(|c| {
                        match &c.new_value {
                            None => "NULL".to_string(), // NULL without quotes
                            Some(value) => {
                                let escaped_value = escape_sql_value(value);
                                format!("'{}'", escaped_value) // Empty string becomes ''
                            }
                        }
                    })
                    .collect();
                format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    self.table_name,
                    columns.join(", "),
                    values.join(", ")
                )
            }
            OperationType::Delete => {
                if let RowIdentifier::PrimaryKey {
                    column: pk_column,
                    value: pk_value,
                } = &self.row_identifier
                {
                    let escaped_pk_value = escape_sql_value(pk_value);
                    format!(
                        "DELETE FROM {} WHERE {} = '{}'",
                        self.table_name, pk_column, escaped_pk_value
                    )
                } else {
                    format!("DELETE FROM {}", self.table_name)
                }
            }
        }
    }
}
