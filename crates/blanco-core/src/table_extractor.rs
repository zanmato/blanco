use std::collections::{HashMap, HashSet};

use crate::DriverType;
use anyhow::{Result, anyhow};
use sqlparser::{
    ast::{
        Expr, ObjectName, SelectItem, SelectItemQualifiedWildcardKind, SetExpr, Statement,
        TableFactor, TableObject, TableWithJoins,
    },
    dialect::{Dialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect},
    parser::Parser,
};

enum AliasResolution {
    Single(String),
    Mixed,
    Undetermined,
}

pub struct TableExtractor {
    dialect: Box<dyn Dialect>,
}

impl TableExtractor {
    pub fn for_driver(driver: DriverType) -> Self {
        let dialect: Box<dyn Dialect> = match driver {
            DriverType::SQLite => Box::new(GenericDialect {}),
            DriverType::PostgreSQL => Box::new(PostgreSqlDialect {}),
            DriverType::MySQL => Box::new(MySqlDialect {}),
            DriverType::ClickHouse => Box::new(GenericDialect {}),
            DriverType::MsSql => Box::new(MsSqlDialect {}),
            // Redis is not SQL; TableExtractor is never used for it (gated by
            // supports_sql), but the match must remain exhaustive.
            DriverType::Redis => Box::new(GenericDialect {}),
        };
        Self { dialect }
    }

    fn parse_sql(&self, sql: &str) -> Result<Option<Statement>> {
        match Parser::parse_sql(self.dialect.as_ref(), sql) {
            Ok(statements) => Ok(statements.into_iter().next()),
            Err(_) => Ok(None),
        }
    }

    pub fn extract_table(&self, sql: &str, alias: bool) -> Result<String> {
        if let Some(statement) = self.parse_sql(sql)?
            && let Some(table_name) = self.extract_from_statement(&statement, alias)
        {
            return Ok(table_name);
        }

        Err(anyhow!("Could not extract table name from query"))
    }

    fn extract_from_statement(&self, statement: &Statement, alias: bool) -> Option<String> {
        match statement {
            Statement::Query(query) => self.extract_from_query(query, alias),
            Statement::Insert(insert) => match &insert.table {
                TableObject::TableName(name) => Some(table_name_only(name)),
                TableObject::TableFunction(_) => None,
            },
            Statement::Update(update) => {
                self.extract_from_table_factor(&update.table.relation, alias)
            }
            Statement::Delete(delete) => {
                let tables = match &delete.from {
                    sqlparser::ast::FromTable::WithFromKeyword(tables) => tables,
                    sqlparser::ast::FromTable::WithoutKeyword(tables) => tables,
                };
                tables
                    .first()
                    .and_then(|t| self.extract_from_table_factor(&t.relation, alias))
            }
            _ => None,
        }
    }

    fn extract_from_query(&self, query: &sqlparser::ast::Query, alias: bool) -> Option<String> {
        if let SetExpr::Select(select) = &*query.body
            && let Some(first_table) = select.from.first()
        {
            if !first_table.joins.is_empty() {
                match Self::resolve_alias_from_projection(&select.projection) {
                    AliasResolution::Single(ref_alias) => {
                        let alias_map = Self::build_alias_map(first_table);
                        if alias_map.contains_key(&ref_alias) {
                            if alias {
                                return Some(ref_alias);
                            }
                            return alias_map.get(&ref_alias).cloned();
                        }
                    }
                    AliasResolution::Mixed => return None,
                    AliasResolution::Undetermined => {}
                }
            }
            return self.extract_from_table_factor(&first_table.relation, alias);
        }
        None
    }

    fn build_alias_map(table_with_joins: &TableWithJoins) -> HashMap<String, String> {
        let mut map = HashMap::new();
        Self::insert_table_factor_alias(&mut map, &table_with_joins.relation);
        for join in &table_with_joins.joins {
            Self::insert_table_factor_alias(&mut map, &join.relation);
        }
        map
    }

    fn insert_table_factor_alias(map: &mut HashMap<String, String>, factor: &TableFactor) {
        if let TableFactor::Table { name, alias, .. } = factor {
            let table_name = table_name_only(name);
            if let Some(a) = alias {
                map.insert(a.name.to_string(), table_name.clone());
            }
            map.insert(table_name.clone(), table_name);
        }
    }

    fn resolve_alias_from_projection(projection: &[SelectItem]) -> AliasResolution {
        let mut aliases = HashSet::new();

        for item in projection {
            match item {
                SelectItem::QualifiedWildcard(
                    SelectItemQualifiedWildcardKind::ObjectName(name),
                    _,
                ) => {
                    if let Some(first) = name.0.first() {
                        aliases.insert(first.to_string());
                    }
                }
                SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
                    match expr {
                        Expr::CompoundIdentifier(parts) if parts.len() >= 2 => {
                            aliases.insert(parts[0].to_string());
                        }
                        _ => return AliasResolution::Undetermined,
                    }
                }
                SelectItem::Wildcard(_) => return AliasResolution::Undetermined,
                _ => return AliasResolution::Undetermined,
            }
        }

        match aliases.len() {
            1 => AliasResolution::Single(aliases.into_iter().next().expect("checked len == 1")),
            n if n > 1 => AliasResolution::Mixed,
            _ => AliasResolution::Undetermined,
        }
    }

    fn extract_from_table_factor(&self, table_factor: &TableFactor, alias: bool) -> Option<String> {
        match table_factor {
            TableFactor::Table {
                name,
                alias: table_alias,
                ..
            } => {
                if alias && let Some(a) = table_alias {
                    return Some(a.name.to_string());
                }
                Some(table_name_only(name))
            }
            TableFactor::Derived {
                subquery,
                alias: table_alias,
                ..
            } => {
                if let Some(a) = table_alias {
                    Some(a.name.to_string())
                } else {
                    self.extract_from_query(subquery, alias)
                }
            }
            _ => None,
        }
    }
}

/// Extract just the table name from an ObjectName, stripping any schema/database prefix.
/// e.g. "public.users" -> "users", "mydb.public.users" -> "users", "users" -> "users"
fn table_name_only(name: &ObjectName) -> String {
    name.0
        .last()
        .map(|part| part.to_string())
        .unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqlite_table_extraction() {
        let extractor = TableExtractor::for_driver(DriverType::SQLite);

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM users", false)
                .unwrap(),
            "users"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM users u", false)
                .unwrap(),
            "users"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM users u", true)
                .unwrap(),
            "u"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM sqlite_master", false)
                .unwrap(),
            "sqlite_master"
        );
    }

    #[test]
    fn test_postgres_table_extraction() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM users u", false)
                .unwrap(),
            "users"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM public.users", false)
                .unwrap(),
            "users"
        );
    }

    #[test]
    fn test_mysql_table_extraction() {
        let extractor = TableExtractor::for_driver(DriverType::MySQL);

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM users u", false)
                .unwrap(),
            "users"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM mydb.users", false)
                .unwrap(),
            "users"
        );
    }

    #[test]
    fn test_insert_table_extraction() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table("INSERT INTO public.users (name) VALUES ('test')", false)
                .unwrap(),
            "users"
        );
    }

    #[test]
    fn test_update_table_extraction() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table("UPDATE public.users SET name = 'test'", false)
                .unwrap(),
            "users"
        );
    }

    #[test]
    fn test_delete_table_extraction() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table("DELETE FROM public.users WHERE id = 1", false)
                .unwrap(),
            "users"
        );
    }

    #[test]
    fn test_join_qualified_wildcard_resolves_correct_table() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table(
                    "SELECT oi.* FROM orders o INNER JOIN order_items oi ON oi.order_id = o.id",
                    false
                )
                .unwrap(),
            "order_items"
        );

        assert_eq!(
            extractor
                .extract_table(
                    "SELECT oi.* FROM orders o INNER JOIN order_items oi ON oi.order_id = o.id",
                    true
                )
                .unwrap(),
            "oi"
        );
    }

    #[test]
    fn test_join_qualified_columns_resolves_correct_table() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table(
                    "SELECT oi.id, oi.quantity FROM orders o JOIN order_items oi ON oi.order_id = o.id",
                    false
                )
                .unwrap(),
            "order_items"
        );
    }

    #[test]
    fn test_join_mixed_aliases_returns_none() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert!(
            extractor
                .extract_table(
                    "SELECT oi.id, o.created_at FROM orders o JOIN order_items oi ON oi.order_id = o.id",
                    false
                )
                .is_err()
        );
    }

    #[test]
    fn test_join_unqualified_wildcard_falls_back_to_first_table() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table(
                    "SELECT * FROM orders o JOIN order_items oi ON oi.order_id = o.id",
                    false
                )
                .unwrap(),
            "orders"
        );
    }

    #[test]
    fn test_join_unqualified_columns_falls_back_to_first_table() {
        let extractor = TableExtractor::for_driver(DriverType::PostgreSQL);

        assert_eq!(
            extractor
                .extract_table(
                    "SELECT id, name FROM orders o JOIN order_items oi ON oi.order_id = o.id",
                    false
                )
                .unwrap(),
            "orders"
        );
    }
}
