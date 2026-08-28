//! Statement builders for the small set of DDL operations offered from the
//! sidebar. Each backend spells these differently, so the dialect knowledge
//! lives here rather than in the UI.

use crate::{DatabaseType, EntityType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableOperation {
    Drop,
    Truncate,
    Rename { new_name: String },
}

impl TableOperation {
    pub fn label(&self) -> &'static str {
        match self {
            TableOperation::Drop => "Drop",
            TableOperation::Truncate => "Truncate",
            TableOperation::Rename { .. } => "Rename",
        }
    }
}

/// Quote one identifier for `db_type`. Input is taken as a bare name; any
/// quote characters in it are escaped, not interpreted.
pub fn quote_identifier(db_type: DatabaseType, identifier: &str) -> String {
    db_type.dialect().quote_identifier(identifier)
}

fn qualified(db_type: DatabaseType, schema: Option<&str>, name: &str) -> String {
    db_type.dialect().quote_qualified(schema, name)
}

/// Build the statement for `operation` on `name`, or `None` when the backend
/// or object kind does not support it (Redis has no tables; views cannot be
/// truncated).
pub fn table_operation_sql(
    db_type: DatabaseType,
    entity: EntityType,
    schema: Option<&str>,
    name: &str,
    operation: &TableOperation,
) -> Option<String> {
    if db_type == DatabaseType::Redis {
        return None;
    }
    let target = qualified(db_type, schema, name);
    let keyword = match entity {
        EntityType::Table => "TABLE",
        EntityType::View => "VIEW",
        EntityType::MaterializedView => "MATERIALIZED VIEW",
    };
    match operation {
        TableOperation::Drop => Some(format!("DROP {keyword} {target}")),
        TableOperation::Truncate => {
            if entity != EntityType::Table {
                return None;
            }
            Some(match db_type {
                // SQLite has no TRUNCATE; an unqualified DELETE is its
                // optimised equivalent.
                DatabaseType::SQLite => format!("DELETE FROM {target}"),
                _ => format!("TRUNCATE TABLE {target}"),
            })
        }
        TableOperation::Rename { new_name } => {
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return None;
            }
            Some(match db_type {
                DatabaseType::MsSql => {
                    let old = match schema.filter(|schema| !schema.is_empty()) {
                        Some(schema) => format!("{schema}.{name}"),
                        None => name.to_string(),
                    };
                    format!(
                        "EXEC sp_rename '{}', '{}'",
                        old.replace('\'', "''"),
                        new_name.replace('\'', "''")
                    )
                }
                DatabaseType::ClickHouse => format!(
                    "RENAME TABLE {target} TO {}",
                    qualified(db_type, schema, new_name)
                ),
                _ => format!(
                    "ALTER {keyword} {target} RENAME TO {}",
                    quote_identifier(db_type, new_name)
                ),
            })
        }
    }
}

/// A column as the user described it in the structure tab's dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    /// Raw default expression (`0`, `'n/a'`, `now()`), `None` for no default.
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnOperation {
    Add(ColumnSpec),
    Rename {
        from: String,
        to: String,
    },
    Drop {
        name: String,
    },
    /// Change type, nullability and/or default of the column called `name`.
    Alter {
        name: String,
        spec: ColumnSpec,
    },
}

impl ColumnOperation {
    pub fn label(&self) -> &'static str {
        match self {
            ColumnOperation::Add(_) => "Add column",
            ColumnOperation::Rename { .. } => "Rename column",
            ColumnOperation::Drop { .. } => "Drop column",
            ColumnOperation::Alter { .. } => "Alter column",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexOperation {
    Create {
        name: String,
        columns: Vec<String>,
        unique: bool,
    },
    Drop {
        name: String,
    },
}

impl IndexOperation {
    pub fn label(&self) -> &'static str {
        match self {
            IndexOperation::Create { .. } => "Create index",
            IndexOperation::Drop { .. } => "Drop index",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdlError {
    /// The backend has no statement for this operation.
    Unsupported(String),
    /// The request itself is malformed (empty name, no columns, ...).
    Invalid(String),
}

impl std::fmt::Display for DdlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DdlError::Unsupported(message) | DdlError::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DdlError {}

fn require_name(name: &str, what: &str) -> Result<(), DdlError> {
    if name.trim().is_empty() {
        Err(DdlError::Invalid(format!("{what} name must not be empty")))
    } else {
        Ok(())
    }
}

fn validate_spec(spec: &ColumnSpec) -> Result<(), DdlError> {
    require_name(&spec.name, "column")?;
    if spec.data_type.trim().is_empty() {
        return Err(DdlError::Invalid(
            "column type must not be empty".to_string(),
        ));
    }
    Ok(())
}

/// `type [NOT NULL] [DEFAULT x]` in the shape each backend accepts in a
/// column definition. ClickHouse expresses nullability in the type.
fn column_definition(db_type: DatabaseType, spec: &ColumnSpec) -> String {
    let dialect = db_type.dialect();
    let mut definition = dialect.quote_identifier(spec.name.trim());
    definition.push(' ');
    let data_type = spec.data_type.trim();
    match db_type {
        DatabaseType::ClickHouse => {
            let already_nullable = data_type.to_ascii_lowercase().starts_with("nullable(");
            if spec.nullable && !already_nullable {
                definition.push_str(&format!("Nullable({data_type})"));
            } else {
                definition.push_str(data_type);
            }
        }
        DatabaseType::MsSql => {
            definition.push_str(data_type);
            definition.push_str(if spec.nullable { " NULL" } else { " NOT NULL" });
        }
        _ => {
            definition.push_str(data_type);
            if !spec.nullable {
                definition.push_str(" NOT NULL");
            }
        }
    }
    if let Some(default) = spec
        .default
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        definition.push_str(&format!(" DEFAULT {default}"));
    }
    definition
}

/// Statements for `operation` on `table`. More than one statement is returned
/// when the backend needs separate steps (a rename plus a modify); run them
/// through `execute_operations_transactional` so a later failure rolls back
/// the earlier ones where the backend allows transactional DDL.
pub fn column_operation_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    operation: &ColumnOperation,
) -> Result<Vec<String>, DdlError> {
    let dialect = db_type.dialect();
    if !dialect.supports_column_ddl() {
        return Err(DdlError::Unsupported(format!(
            "{} has no column DDL",
            db_type.as_str()
        )));
    }
    let target = dialect.quote_qualified(schema, table);
    let quoted = |name: &str| dialect.quote_identifier(name.trim());

    match operation {
        ColumnOperation::Add(spec) => {
            validate_spec(spec)?;
            let definition = column_definition(db_type, spec);
            Ok(vec![match db_type {
                // T-SQL has no COLUMN keyword in ADD.
                DatabaseType::MsSql => format!("ALTER TABLE {target} ADD {definition}"),
                _ => format!("ALTER TABLE {target} ADD COLUMN {definition}"),
            }])
        }
        ColumnOperation::Rename { from, to } => {
            require_name(from, "column")?;
            require_name(to, "new column")?;
            Ok(vec![rename_column_sql(db_type, schema, table, from, to)])
        }
        ColumnOperation::Drop { name } => {
            require_name(name, "column")?;
            Ok(vec![format!(
                "ALTER TABLE {target} DROP COLUMN {}",
                quoted(name)
            )])
        }
        ColumnOperation::Alter { name, spec } => {
            require_name(name, "column")?;
            validate_spec(spec)?;
            let renamed = name.trim() != spec.name.trim();
            let mut statements = Vec::new();
            match db_type {
                DatabaseType::PostgreSQL => {
                    if renamed {
                        statements
                            .push(rename_column_sql(db_type, schema, table, name, &spec.name));
                    }
                    let column = quoted(&spec.name);
                    let null_clause = if spec.nullable {
                        "DROP NOT NULL"
                    } else {
                        "SET NOT NULL"
                    };
                    let default_clause = match spec.default.as_deref().map(str::trim) {
                        Some(default) if !default.is_empty() => format!("SET DEFAULT {default}"),
                        _ => "DROP DEFAULT".to_string(),
                    };
                    statements.push(format!(
                        "ALTER TABLE {target} ALTER COLUMN {column} TYPE {}, ALTER COLUMN {column} {null_clause}, ALTER COLUMN {column} {default_clause}",
                        spec.data_type.trim()
                    ));
                }
                DatabaseType::MySQL => {
                    // CHANGE COLUMN carries the (possibly new) name along with
                    // the full definition, so one statement covers everything.
                    statements.push(format!(
                        "ALTER TABLE {target} CHANGE COLUMN {} {}",
                        quoted(name),
                        column_definition(db_type, spec)
                    ));
                }
                DatabaseType::MsSql => {
                    if spec
                        .default
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|d| !d.is_empty())
                    {
                        return Err(DdlError::Unsupported(
                            "SQL Server stores defaults as named constraints; change the default in a query instead".to_string(),
                        ));
                    }
                    if renamed {
                        statements
                            .push(rename_column_sql(db_type, schema, table, name, &spec.name));
                    }
                    statements.push(format!(
                        "ALTER TABLE {target} ALTER COLUMN {} {} {}",
                        quoted(&spec.name),
                        spec.data_type.trim(),
                        if spec.nullable { "NULL" } else { "NOT NULL" }
                    ));
                }
                DatabaseType::ClickHouse => {
                    if renamed {
                        statements
                            .push(rename_column_sql(db_type, schema, table, name, &spec.name));
                    }
                    statements.push(format!(
                        "ALTER TABLE {target} MODIFY COLUMN {}",
                        column_definition(db_type, spec)
                    ));
                }
                DatabaseType::SQLite => {
                    // A rebuild from ColumnInfo would silently drop CHECK and
                    // UNIQUE constraints, triggers and partial indexes, so the
                    // only safe in-place change is the rename.
                    return Err(DdlError::Unsupported(
                        "SQLite cannot change a column's type, nullability or default in place; recreate the table in a query".to_string(),
                    ));
                }
                DatabaseType::Redis => {
                    return Err(DdlError::Unsupported("Redis has no columns".to_string()));
                }
            }
            Ok(statements)
        }
    }
}

fn rename_column_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    from: &str,
    to: &str,
) -> String {
    let dialect = db_type.dialect();
    match db_type {
        DatabaseType::MsSql => {
            let mut path = String::new();
            if let Some(schema) = schema.filter(|schema| !schema.is_empty()) {
                path.push_str(schema);
                path.push('.');
            }
            path.push_str(table);
            path.push('.');
            path.push_str(from.trim());
            format!(
                "EXEC sp_rename '{}', '{}', 'COLUMN'",
                path.replace('\'', "''"),
                to.trim().replace('\'', "''")
            )
        }
        _ => format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            dialect.quote_qualified(schema, table),
            dialect.quote_identifier(from.trim()),
            dialect.quote_identifier(to.trim())
        ),
    }
}

/// Statements for `operation` on the indexes of `table`.
pub fn index_operation_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    operation: &IndexOperation,
) -> Result<Vec<String>, DdlError> {
    let dialect = db_type.dialect();
    if !dialect.supports_column_ddl() {
        return Err(DdlError::Unsupported(format!(
            "{} has no indexes",
            db_type.as_str()
        )));
    }
    let target = dialect.quote_qualified(schema, table);
    match operation {
        IndexOperation::Create {
            name,
            columns,
            unique,
        } => {
            require_name(name, "index")?;
            if columns.is_empty() {
                return Err(DdlError::Invalid(
                    "an index needs at least one column".to_string(),
                ));
            }
            let column_list = columns
                .iter()
                .map(|column| dialect.quote_identifier(column.trim()))
                .collect::<Vec<_>>()
                .join(", ");
            let index_name = dialect.quote_identifier(name.trim());
            Ok(vec![match db_type {
                DatabaseType::ClickHouse => {
                    if *unique {
                        return Err(DdlError::Unsupported(
                            "ClickHouse has no unique indexes".to_string(),
                        ));
                    }
                    // Only data-skipping indexes can be added after the fact;
                    // minmax is the cheapest general-purpose one.
                    format!(
                        "ALTER TABLE {target} ADD INDEX {index_name} ({column_list}) TYPE minmax GRANULARITY 1"
                    )
                }
                _ => format!(
                    "CREATE {}INDEX {index_name} ON {target} ({column_list})",
                    if *unique { "UNIQUE " } else { "" }
                ),
            }])
        }
        IndexOperation::Drop { name } => {
            require_name(name, "index")?;
            let index_name = dialect.quote_identifier(name.trim());
            Ok(vec![match db_type {
                // Indexes are schema-scoped objects here, not table-scoped.
                DatabaseType::PostgreSQL | DatabaseType::SQLite => {
                    format!(
                        "DROP INDEX {}",
                        dialect.quote_qualified(schema, name.trim())
                    )
                }
                DatabaseType::MySQL | DatabaseType::MsSql => {
                    format!("DROP INDEX {index_name} ON {target}")
                }
                DatabaseType::ClickHouse => format!("ALTER TABLE {target} DROP INDEX {index_name}"),
                DatabaseType::Redis => {
                    return Err(DdlError::Unsupported("Redis has no indexes".to_string()));
                }
            }])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_and_truncate_per_backend() {
        let sql = |db_type, op| {
            table_operation_sql(db_type, EntityType::Table, Some("app"), "order", &op)
        };
        assert_eq!(
            sql(DatabaseType::PostgreSQL, TableOperation::Drop).as_deref(),
            Some("DROP TABLE \"app\".\"order\"")
        );
        assert_eq!(
            sql(DatabaseType::MySQL, TableOperation::Truncate).as_deref(),
            Some("TRUNCATE TABLE `app`.`order`")
        );
        assert_eq!(
            sql(DatabaseType::MsSql, TableOperation::Drop).as_deref(),
            Some("DROP TABLE [app].[order]")
        );
        assert_eq!(
            table_operation_sql(
                DatabaseType::SQLite,
                EntityType::Table,
                None,
                "t",
                &TableOperation::Truncate
            )
            .as_deref(),
            Some("DELETE FROM \"t\"")
        );
        assert!(sql(DatabaseType::Redis, TableOperation::Drop).is_none());
        assert!(
            table_operation_sql(
                DatabaseType::PostgreSQL,
                EntityType::View,
                None,
                "v",
                &TableOperation::Truncate
            )
            .is_none()
        );
    }

    #[test]
    fn rename_per_backend() {
        let rename = TableOperation::Rename {
            new_name: "orders".into(),
        };
        assert_eq!(
            table_operation_sql(
                DatabaseType::PostgreSQL,
                EntityType::Table,
                Some("app"),
                "order",
                &rename
            )
            .as_deref(),
            Some("ALTER TABLE \"app\".\"order\" RENAME TO \"orders\"")
        );
        assert_eq!(
            table_operation_sql(
                DatabaseType::MsSql,
                EntityType::Table,
                Some("dbo"),
                "order",
                &rename
            )
            .as_deref(),
            Some("EXEC sp_rename 'dbo.order', 'orders'")
        );
        assert_eq!(
            table_operation_sql(
                DatabaseType::ClickHouse,
                EntityType::Table,
                Some("db"),
                "order",
                &rename
            )
            .as_deref(),
            Some("RENAME TABLE `db`.`order` TO `db`.`orders`")
        );
        assert!(
            table_operation_sql(
                DatabaseType::SQLite,
                EntityType::Table,
                None,
                "t",
                &TableOperation::Rename {
                    new_name: "  ".into()
                }
            )
            .is_none()
        );
    }

    #[test]
    fn identifiers_are_escaped() {
        assert_eq!(
            quote_identifier(DatabaseType::PostgreSQL, "we\"ird"),
            "\"we\"\"ird\""
        );
        assert_eq!(quote_identifier(DatabaseType::MsSql, "a]b"), "[a]]b]");
    }

    fn spec(name: &str, data_type: &str, nullable: bool, default: Option<&str>) -> ColumnSpec {
        ColumnSpec {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable,
            default: default.map(str::to_string),
        }
    }

    #[test]
    fn add_column_per_backend() {
        let add = ColumnOperation::Add(spec("age", "integer", false, Some("0")));
        let sql = |db_type| column_operation_sql(db_type, Some("app"), "user", &add);
        assert_eq!(
            sql(DatabaseType::PostgreSQL),
            Ok(vec![
                "ALTER TABLE \"app\".\"user\" ADD COLUMN \"age\" integer NOT NULL DEFAULT 0"
                    .to_string()
            ])
        );
        assert_eq!(
            sql(DatabaseType::MySQL),
            Ok(vec![
                "ALTER TABLE `app`.`user` ADD COLUMN `age` integer NOT NULL DEFAULT 0".to_string()
            ])
        );
        assert_eq!(
            sql(DatabaseType::MsSql),
            Ok(vec![
                "ALTER TABLE [app].[user] ADD [age] integer NOT NULL DEFAULT 0".to_string()
            ])
        );
        let nullable = ColumnOperation::Add(spec("note", "String", true, None));
        assert_eq!(
            column_operation_sql(DatabaseType::ClickHouse, None, "t", &nullable),
            Ok(vec![
                "ALTER TABLE `t` ADD COLUMN `note` Nullable(String)".to_string()
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::SQLite, None, "t", &nullable),
            Ok(vec![
                "ALTER TABLE \"t\" ADD COLUMN \"note\" String".to_string()
            ])
        );
        assert!(matches!(
            column_operation_sql(DatabaseType::Redis, None, "t", &nullable),
            Err(DdlError::Unsupported(_))
        ));
        assert!(matches!(
            column_operation_sql(
                DatabaseType::PostgreSQL,
                None,
                "t",
                &ColumnOperation::Add(spec("", "int", true, None))
            ),
            Err(DdlError::Invalid(_))
        ));
    }

    #[test]
    fn rename_and_drop_column_per_backend() {
        let rename = ColumnOperation::Rename {
            from: "old".to_string(),
            to: "new".to_string(),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &rename),
            Ok(vec![
                "ALTER TABLE \"app\".\"t\" RENAME COLUMN \"old\" TO \"new\"".to_string()
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &rename),
            Ok(vec![
                "EXEC sp_rename 'dbo.t.old', 'new', 'COLUMN'".to_string()
            ])
        );
        let drop = ColumnOperation::Drop {
            name: "old".to_string(),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &drop),
            Ok(vec!["ALTER TABLE `t` DROP COLUMN `old`".to_string()])
        );
    }

    #[test]
    fn alter_column_per_backend() {
        let alter = ColumnOperation::Alter {
            name: "qty".to_string(),
            spec: spec("quantity", "bigint", true, None),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, None, "t", &alter),
            Ok(vec![
                "ALTER TABLE \"t\" RENAME COLUMN \"qty\" TO \"quantity\"".to_string(),
                "ALTER TABLE \"t\" ALTER COLUMN \"quantity\" TYPE bigint, ALTER COLUMN \"quantity\" DROP NOT NULL, ALTER COLUMN \"quantity\" DROP DEFAULT".to_string(),
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &alter),
            Ok(vec![
                "ALTER TABLE `t` CHANGE COLUMN `qty` `quantity` bigint".to_string()
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &alter),
            Ok(vec![
                "EXEC sp_rename 'dbo.t.qty', 'quantity', 'COLUMN'".to_string(),
                "ALTER TABLE [dbo].[t] ALTER COLUMN [quantity] bigint NULL".to_string(),
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::ClickHouse, None, "t", &alter),
            Ok(vec![
                "ALTER TABLE `t` RENAME COLUMN `qty` TO `quantity`".to_string(),
                "ALTER TABLE `t` MODIFY COLUMN `quantity` Nullable(bigint)".to_string(),
            ])
        );
        assert!(matches!(
            column_operation_sql(DatabaseType::SQLite, None, "t", &alter),
            Err(DdlError::Unsupported(_))
        ));
        let with_default = ColumnOperation::Alter {
            name: "qty".to_string(),
            spec: spec("qty", "int", false, Some("1")),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, None, "t", &with_default),
            Ok(vec![
                "ALTER TABLE \"t\" ALTER COLUMN \"qty\" TYPE int, ALTER COLUMN \"qty\" SET NOT NULL, ALTER COLUMN \"qty\" SET DEFAULT 1".to_string(),
            ])
        );
        assert!(matches!(
            column_operation_sql(DatabaseType::MsSql, None, "t", &with_default),
            Err(DdlError::Unsupported(_))
        ));
    }

    #[test]
    fn index_operations_per_backend() {
        let create = IndexOperation::Create {
            name: "t_a_b".to_string(),
            columns: vec!["a".to_string(), "b".to_string()],
            unique: true,
        };
        assert_eq!(
            index_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &create),
            Ok(vec![
                "CREATE UNIQUE INDEX \"t_a_b\" ON \"app\".\"t\" (\"a\", \"b\")".to_string()
            ])
        );
        assert_eq!(
            index_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &create),
            Ok(vec![
                "CREATE UNIQUE INDEX [t_a_b] ON [dbo].[t] ([a], [b])".to_string()
            ])
        );
        assert!(matches!(
            index_operation_sql(DatabaseType::ClickHouse, None, "t", &create),
            Err(DdlError::Unsupported(_))
        ));
        let skipping = IndexOperation::Create {
            name: "idx".to_string(),
            columns: vec!["a".to_string()],
            unique: false,
        };
        assert_eq!(
            index_operation_sql(DatabaseType::ClickHouse, None, "t", &skipping),
            Ok(vec![
                "ALTER TABLE `t` ADD INDEX `idx` (`a`) TYPE minmax GRANULARITY 1".to_string()
            ])
        );
        let drop = IndexOperation::Drop {
            name: "idx".to_string(),
        };
        assert_eq!(
            index_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &drop),
            Ok(vec!["DROP INDEX \"app\".\"idx\"".to_string()])
        );
        assert_eq!(
            index_operation_sql(DatabaseType::MySQL, None, "t", &drop),
            Ok(vec!["DROP INDEX `idx` ON `t`".to_string()])
        );
        assert_eq!(
            index_operation_sql(DatabaseType::ClickHouse, None, "t", &drop),
            Ok(vec!["ALTER TABLE `t` DROP INDEX `idx`".to_string()])
        );
        assert!(matches!(
            index_operation_sql(
                DatabaseType::SQLite,
                None,
                "t",
                &IndexOperation::Create {
                    name: "i".to_string(),
                    columns: vec![],
                    unique: false
                }
            ),
            Err(DdlError::Invalid(_))
        ));
    }
}
