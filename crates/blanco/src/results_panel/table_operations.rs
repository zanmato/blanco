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

    fn quote_identifier(identifier: &str, database_type: database::DatabaseType) -> String {
        identifier
            .split('.')
            .map(|part| {
                let part = part.trim();
                match database_type {
                    database::DatabaseType::MySQL => {
                        let unquoted = part
                            .strip_prefix('`')
                            .and_then(|value| value.strip_suffix('`'))
                            .unwrap_or(part);
                        format!("`{}`", unquoted.replace('`', "``"))
                    }
                    database::DatabaseType::MsSql => {
                        let unquoted = part
                            .strip_prefix('[')
                            .and_then(|value| value.strip_suffix(']'))
                            .unwrap_or(part);
                        format!("[{}]", unquoted.replace(']', "]]"))
                    }
                    _ => {
                        let unquoted = part
                            .strip_prefix('"')
                            .and_then(|value| value.strip_suffix('"'))
                            .unwrap_or(part);
                        format!("\"{}\"", unquoted.replace('"', "\"\""))
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Values that pass through unquoted so users can type them into a cell and
    /// have the server evaluate them. Deliberately restricted to bare keywords:
    /// anything else (JSON documents, text, numbers) is data and gets quoted.
    const PASSTHROUGH_KEYWORDS: [&'static str; 8] = [
        "DEFAULT",
        "NULL",
        "CURRENT_TIMESTAMP",
        "CURRENT_DATE",
        "CURRENT_TIME",
        "CURRENT_USER",
        "LOCALTIME",
        "LOCALTIMESTAMP",
    ];

    /// Whether a cell value should be emitted verbatim as a SQL expression.
    ///
    /// Only a keyword from [`Self::PASSTHROUGH_KEYWORDS`] or an argument-less
    /// function call such as `NOW()` / `gen_random_uuid()` qualifies. Anything
    /// with arguments, quotes, whitespace or punctuation is treated as data, so
    /// a JSON document like `{"a": 1}` can never be mistaken for SQL.
    fn is_sql_expression(value: &str) -> bool {
        let value = value.trim();
        if Self::PASSTHROUGH_KEYWORDS
            .iter()
            .any(|keyword| value.eq_ignore_ascii_case(keyword))
        {
            return true;
        }

        let Some(name) = value.strip_suffix("()") else {
            return false;
        };
        !name.is_empty()
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            && !name.starts_with(|character: char| character.is_ascii_digit())
    }

    /// Render a cell value as a SQL string literal. Grid values are plain data
    /// (JSON documents, text with quotes) unless they are one of the recognised
    /// bare expressions, so they are quoted by default. MySQL interprets
    /// backslashes inside string literals unless `NO_BACKSLASH_ESCAPES` is set,
    /// so they need doubling there as well.
    fn format_value(value: &str, database_type: database::DatabaseType) -> String {
        if Self::is_sql_expression(value) {
            return value.trim().to_string();
        }

        Self::format_literal(value, database_type)
    }

    /// Always quote, never interpret. Used for primary key values in WHERE
    /// clauses, which come from the server and are data by definition.
    fn format_literal(value: &str, database_type: database::DatabaseType) -> String {
        let escaped = match database_type {
            database::DatabaseType::MySQL => value.replace('\\', "\\\\").replace('\'', "''"),
            _ => value.replace('\'', "''"),
        };
        format!("'{escaped}'")
    }

    fn format_where_clause(
        columns: &[(String, String)],
        database_type: database::DatabaseType,
    ) -> String {
        columns
            .iter()
            .map(|(column, value)| {
                format!(
                    "{} = {}",
                    Self::quote_identifier(column, database_type),
                    Self::format_literal(value, database_type)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// Generate the actual SQL query for logging purposes
    pub fn to_sql_query(&self, database_type: database::DatabaseType) -> String {
        let table_name = Self::quote_identifier(&self.table_name, database_type);
        match self.operation_type {
            OperationType::Update => {
                if let RowIdentifier::PrimaryKey {
                    columns: pk_columns,
                } = &self.row_identifier
                    && !pk_columns.is_empty()
                {
                    if self.changes.is_empty() {
                        format!("UPDATE {table_name}")
                    } else {
                        let set_clauses: Vec<String> = self
                            .changes
                            .iter()
                            .map(|change| {
                                let set_value = match &change.new_value {
                                    None => "NULL".to_string(),
                                    Some(value) => Self::format_value(value, database_type),
                                };
                                format!(
                                    "{} = {}",
                                    Self::quote_identifier(&change.column_name, database_type),
                                    set_value
                                )
                            })
                            .collect();
                        format!(
                            "UPDATE {} SET {} WHERE {}",
                            table_name,
                            set_clauses.join(", "),
                            Self::format_where_clause(pk_columns, database_type),
                        )
                    }
                } else {
                    format!("UPDATE {table_name}")
                }
            }
            OperationType::Insert => {
                // Every column was omitted (all defaults): MySQL has no
                // `DEFAULT VALUES` form, everything else has no `()` form.
                if self.changes.is_empty() {
                    return match database_type {
                        database::DatabaseType::MySQL => {
                            format!("INSERT INTO {table_name} () VALUES ()")
                        }
                        _ => format!("INSERT INTO {table_name} DEFAULT VALUES"),
                    };
                }
                let columns: Vec<String> = self
                    .changes
                    .iter()
                    .map(|change| Self::quote_identifier(&change.column_name, database_type))
                    .collect();
                let values: Vec<String> = self
                    .changes
                    .iter()
                    .map(|change| match &change.new_value {
                        None => "NULL".to_string(),
                        Some(value) => Self::format_value(value, database_type),
                    })
                    .collect();
                format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    table_name,
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
                        table_name,
                        Self::format_where_clause(pk_columns, database_type),
                    )
                } else {
                    format!("DELETE FROM {table_name}")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use database::DatabaseType;

    use super::{ColumnChange, OperationType, RowIdentifier, TableChangeOperation};

    #[test]
    fn update_quotes_postgres_identifiers_and_escapes_primary_key_values() {
        let operation = TableChangeOperation {
            operation_type: OperationType::Update,
            table_name: "sales.order".to_string(),
            row_identifier: RowIdentifier::PrimaryKey {
                columns: vec![("user".to_string(), "O'Brien".to_string())],
            },
            changes: vec![ColumnChange {
                column_name: "when".to_string(),
                new_value: Some("2026-07-28".to_string()),
            }],
        };

        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            "UPDATE \"sales\".\"order\" SET \"when\" = '2026-07-28' WHERE \"user\" = 'O''Brien'"
        );
    }

    #[test]
    fn insert_quotes_json_values_and_mysql_identifiers() {
        let operation = TableChangeOperation::insert_row(
            "analytics.event".to_string(),
            vec![
                ColumnChange {
                    column_name: "select".to_string(),
                    new_value: Some(r#"{"name": "O'Brien", "path": "a\\b"}"#.to_string()),
                },
                ColumnChange {
                    column_name: "created_at".to_string(),
                    new_value: None,
                },
            ],
        );

        assert_eq!(
            operation.to_sql_query(DatabaseType::MySQL),
            r#"INSERT INTO `analytics`.`event` (`select`, `created_at`) VALUES ('{"name": "O''Brien", "path": "a\\\\b"}', NULL)"#
        );
    }

    #[test]
    fn insert_quotes_json_values_for_postgres_without_backslash_doubling() {
        let operation = TableChangeOperation::insert_row(
            "public.event".to_string(),
            vec![ColumnChange {
                column_name: "payload".to_string(),
                new_value: Some(r#"{"a": 1, "b": [2, 3]}"#.to_string()),
            }],
        );

        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            r#"INSERT INTO "public"."event" ("payload") VALUES ('{"a": 1, "b": [2, 3]}')"#
        );
    }

    #[test]
    fn bare_expressions_pass_through_unquoted() {
        let operation = TableChangeOperation::insert_row(
            "public.event".to_string(),
            vec![
                ColumnChange {
                    column_name: "created_at".to_string(),
                    new_value: Some("NOW()".to_string()),
                },
                ColumnChange {
                    column_name: "id".to_string(),
                    new_value: Some("gen_random_uuid()".to_string()),
                },
                ColumnChange {
                    column_name: "updated_at".to_string(),
                    new_value: Some("current_timestamp".to_string()),
                },
                ColumnChange {
                    column_name: "label".to_string(),
                    new_value: Some("upper('a')".to_string()),
                },
            ],
        );

        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            "INSERT INTO \"public\".\"event\" (\"created_at\", \"id\", \"updated_at\", \"label\") \
             VALUES (NOW(), gen_random_uuid(), current_timestamp, 'upper(''a'')')"
        );
    }

    #[test]
    fn primary_key_values_are_never_treated_as_expressions() {
        let operation = TableChangeOperation {
            operation_type: OperationType::Delete,
            table_name: "public.setting".to_string(),
            row_identifier: RowIdentifier::PrimaryKey {
                columns: vec![("name".to_string(), "DEFAULT".to_string())],
            },
            changes: Vec::new(),
        };

        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            "DELETE FROM \"public\".\"setting\" WHERE \"name\" = 'DEFAULT'"
        );
    }

    #[test]
    fn delete_quotes_mssql_identifiers() {
        let operation = TableChangeOperation {
            operation_type: OperationType::Delete,
            table_name: "dbo.user]data".to_string(),
            row_identifier: RowIdentifier::PrimaryKey {
                columns: vec![("key]column".to_string(), "1".to_string())],
            },
            changes: Vec::new(),
        };

        assert_eq!(
            operation.to_sql_query(DatabaseType::MsSql),
            "DELETE FROM [dbo].[user]]data] WHERE [key]]column] = '1'"
        );
    }
}
