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
    PrimaryKey { columns: Vec<(String, String)> },
    RowIndex, // For cases without clear PK
}

#[derive(Clone, Debug)]
pub struct ColumnChange {
    pub column_name: String,
    pub new_value: Option<String>,
}

impl TableChangeOperation {
    pub fn insert_row(table_name: String, column_changes: Vec<ColumnChange>) -> Self {
        Self {
            operation_type: OperationType::Insert,
            table_name,
            row_identifier: RowIdentifier::RowIndex,
            changes: column_changes,
        }
    }

    fn format_where_clause(columns: &[(String, String)]) -> String {
        let escape_sql_value = |value: &str| value.replace('\'', "''");
        columns
            .iter()
            .map(|(col, val)| format!("{} = '{}'", col, escape_sql_value(val)))
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// Generate the actual SQL query for logging purposes
    pub fn to_sql_query(&self) -> String {
        let escape_sql_value = |value: &str| value.replace('\'', "''");

        match self.operation_type {
            OperationType::Update => {
                if let RowIdentifier::PrimaryKey {
                    columns: pk_columns,
                } = &self.row_identifier
                    && !pk_columns.is_empty()
                {
                    if self.changes.is_empty() {
                        format!("UPDATE {}", self.table_name)
                    } else {
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
                        format!(
                            "UPDATE {} SET {} WHERE {}",
                            self.table_name,
                            set_clauses.join(", "),
                            Self::format_where_clause(pk_columns),
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
                    .map(|c| match &c.new_value {
                        None => "NULL".to_string(),
                        Some(value) => {
                            let escaped_value = escape_sql_value(value);
                            format!("'{}'", escaped_value)
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
                    columns: pk_columns,
                } = &self.row_identifier
                    && !pk_columns.is_empty()
                {
                    format!(
                        "DELETE FROM {} WHERE {}",
                        self.table_name,
                        Self::format_where_clause(pk_columns),
                    )
                } else {
                    format!("DELETE FROM {}", self.table_name)
                }
            }
        }
    }
}
