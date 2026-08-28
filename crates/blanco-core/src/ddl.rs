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
}
