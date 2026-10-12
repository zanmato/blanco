// Database-agnostic table change operations

use blanco_core::ColumnType;

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
    PrimaryKey { columns: Vec<(String, ColumnValue)> },
    RowIndex, // For cases without clear PK
}

#[derive(Clone, Debug)]
pub struct ColumnChange {
    pub column_name: String,
    pub value: ColumnValue,
}

/// What a cell contributes to a statement. Grid values are data and get
/// quoted, unless the user switched the cell to expression input, in which
/// case the text is SQL for the server to evaluate.
#[derive(Clone, Debug, PartialEq)]
pub enum ColumnValue {
    Null,
    Literal(String),
    /// The bytes of a binary column, as hex digits without a prefix. They are
    /// written in the backend's binary literal, not as a string of digits.
    Binary(String),
    Expression(String),
}

impl ColumnValue {
    /// A blank expression would leave a hole in the statement (`SET x = `),
    /// so it falls back to the literal the cell would otherwise hold.
    #[cfg(test)]
    pub fn from_cell(value: Option<String>, is_expression: bool) -> Self {
        Self::from_typed_cell(value, is_expression, ColumnType::Text)
    }

    /// As [`Self::from_cell`], for a column of `column_type`: a binary
    /// column's hex notation becomes the bytes it spells.
    pub fn from_typed_cell(
        value: Option<String>,
        is_expression: bool,
        column_type: ColumnType,
    ) -> Self {
        match value {
            None => Self::Null,
            Some(text) if is_expression && !text.trim().is_empty() => {
                Self::Expression(text.trim().to_string())
            }
            Some(text) => Self::data(text, column_type),
        }
    }

    /// A value that is data by definition, such as a primary key read from
    /// the server.
    pub fn data(text: String, column_type: ColumnType) -> Self {
        if column_type == ColumnType::Binary
            && let Some(hex) = blanco_core::binary::hex_digits(&text)
        {
            return Self::Binary(hex);
        }
        Self::Literal(text)
    }
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
        database_type.dialect().quote_path(identifier)
    }

    fn format_value(value: &ColumnValue, database_type: database::DatabaseType) -> String {
        match value {
            ColumnValue::Null => "NULL".to_string(),
            ColumnValue::Literal(text) => Self::format_literal(text, database_type),
            ColumnValue::Binary(hex) => database_type.dialect().binary_literal(hex),
            ColumnValue::Expression(sql) => sql.clone(),
        }
    }

    /// Always quote, never interpret. Used for primary key values in WHERE
    /// clauses, which come from the server and are data by definition.
    fn format_literal(value: &str, database_type: database::DatabaseType) -> String {
        database_type.dialect().string_literal(value)
    }

    /// Key values come from the server and are data: quoted or written as
    /// bytes, never evaluated.
    fn format_key_value(value: &ColumnValue, database_type: database::DatabaseType) -> String {
        match value {
            ColumnValue::Null => "NULL".to_string(),
            ColumnValue::Literal(text) | ColumnValue::Expression(text) => {
                Self::format_literal(text, database_type)
            }
            ColumnValue::Binary(hex) => database_type.dialect().binary_literal(hex),
        }
    }

    fn format_where_clause(
        columns: &[(String, ColumnValue)],
        database_type: database::DatabaseType,
    ) -> String {
        columns
            .iter()
            .map(|(column, value)| {
                format!(
                    "{} = {}",
                    Self::quote_identifier(column, database_type),
                    Self::format_key_value(value, database_type)
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
                                format!(
                                    "{} = {}",
                                    Self::quote_identifier(&change.column_name, database_type),
                                    Self::format_value(&change.value, database_type)
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
                    return database_type.dialect().insert_defaults_sql(&table_name);
                }
                let columns: Vec<String> = self
                    .changes
                    .iter()
                    .map(|change| Self::quote_identifier(&change.column_name, database_type))
                    .collect();
                let values: Vec<String> = self
                    .changes
                    .iter()
                    .map(|change| Self::format_value(&change.value, database_type))
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

    use super::{ColumnChange, ColumnValue, OperationType, RowIdentifier, TableChangeOperation};

    #[test]
    fn update_quotes_postgres_identifiers_and_escapes_primary_key_values() {
        let operation = TableChangeOperation {
            operation_type: OperationType::Update,
            table_name: "sales.order".to_string(),
            row_identifier: RowIdentifier::PrimaryKey {
                columns: vec![(
                    "user".to_string(),
                    ColumnValue::Literal("O'Brien".to_string()),
                )],
            },
            changes: vec![ColumnChange {
                column_name: "when".to_string(),
                value: ColumnValue::Literal("2026-07-28".to_string()),
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
                    value: ColumnValue::Literal(
                        r#"{"name": "O'Brien", "path": "a\\b"}"#.to_string(),
                    ),
                },
                ColumnChange {
                    column_name: "created_at".to_string(),
                    value: ColumnValue::Null,
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
                value: ColumnValue::Literal(r#"{"a": 1, "b": [2, 3]}"#.to_string()),
            }],
        );

        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            r#"INSERT INTO "public"."event" ("payload") VALUES ('{"a": 1, "b": [2, 3]}')"#
        );
    }

    #[test]
    fn only_expression_values_pass_through_unquoted() {
        let operation = TableChangeOperation::insert_row(
            "public.event".to_string(),
            vec![
                ColumnChange {
                    column_name: "created_at".to_string(),
                    value: ColumnValue::from_cell(
                        Some(" NOW()+'1 day'::interval ".to_string()),
                        true,
                    ),
                },
                ColumnChange {
                    column_name: "label".to_string(),
                    value: ColumnValue::from_cell(Some("NOW()".to_string()), false),
                },
                ColumnChange {
                    column_name: "note".to_string(),
                    value: ColumnValue::from_cell(Some("  ".to_string()), true),
                },
                ColumnChange {
                    column_name: "deleted_at".to_string(),
                    value: ColumnValue::from_cell(None, true),
                },
            ],
        );

        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            "INSERT INTO \"public\".\"event\" (\"created_at\", \"label\", \"note\", \"deleted_at\") \
             VALUES (NOW()+'1 day'::interval, 'NOW()', '  ', NULL)"
        );
    }

    #[test]
    fn binary_values_and_keys_are_written_as_bytes() {
        use blanco_core::ColumnType;

        let operation = TableChangeOperation {
            operation_type: OperationType::Update,
            table_name: "files".to_string(),
            row_identifier: RowIdentifier::PrimaryKey {
                columns: vec![(
                    "id".to_string(),
                    ColumnValue::data("0xCAFE".to_string(), ColumnType::Binary),
                )],
            },
            changes: vec![ColumnChange {
                column_name: "body".to_string(),
                value: ColumnValue::from_typed_cell(
                    Some("0x00ff".to_string()),
                    false,
                    ColumnType::Binary,
                ),
            }],
        };
        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            r#"UPDATE "files" SET "body" = '\x00ff'::bytea WHERE "id" = '\xcafe'::bytea"#
        );
        assert_eq!(
            operation.to_sql_query(DatabaseType::MySQL),
            "UPDATE `files` SET `body` = X'00ff' WHERE `id` = X'cafe'"
        );
        assert_eq!(
            operation.to_sql_query(DatabaseType::SQLite),
            r#"UPDATE "files" SET "body" = X'00ff' WHERE "id" = X'cafe'"#
        );
        assert_eq!(
            operation.to_sql_query(DatabaseType::MsSql),
            "UPDATE [files] SET [body] = 0x00ff WHERE [id] = 0xcafe"
        );

        // Without the prefix the text is a string, and the same text in a
        // text column is never bytes.
        assert_eq!(
            ColumnValue::from_typed_cell(Some("cafe".to_string()), false, ColumnType::Binary),
            ColumnValue::Literal("cafe".to_string())
        );
        assert_eq!(
            ColumnValue::from_typed_cell(Some("0xcafe".to_string()), false, ColumnType::Text),
            ColumnValue::Literal("0xcafe".to_string())
        );
    }

    #[test]
    fn update_sets_expression_verbatim() {
        let operation = TableChangeOperation {
            operation_type: OperationType::Update,
            table_name: "public.item".to_string(),
            row_identifier: RowIdentifier::PrimaryKey {
                columns: vec![("id".to_string(), ColumnValue::Literal("7".to_string()))],
            },
            changes: vec![ColumnChange {
                column_name: "price".to_string(),
                value: ColumnValue::Expression("price * 1.1".to_string()),
            }],
        };

        assert_eq!(
            operation.to_sql_query(DatabaseType::PostgreSQL),
            "UPDATE \"public\".\"item\" SET \"price\" = price * 1.1 WHERE \"id\" = '7'"
        );
    }

    #[test]
    fn primary_key_values_are_never_treated_as_expressions() {
        let operation = TableChangeOperation {
            operation_type: OperationType::Delete,
            table_name: "public.setting".to_string(),
            row_identifier: RowIdentifier::PrimaryKey {
                columns: vec![(
                    "name".to_string(),
                    ColumnValue::Literal("DEFAULT".to_string()),
                )],
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
                columns: vec![(
                    "key]column".to_string(),
                    ColumnValue::Literal("1".to_string()),
                )],
            },
            changes: Vec::new(),
        };

        assert_eq!(
            operation.to_sql_query(DatabaseType::MsSql),
            "DELETE FROM [dbo].[user]]data] WHERE [key]]column] = '1'"
        );
    }
}
