use anyhow::{Result, anyhow};
use crate::DriverType;
use sqlparser::{
    ast::{ObjectName, SetExpr, Statement, TableFactor, TableObject},
    dialect::{Dialect, GenericDialect, MySqlDialect, PostgreSqlDialect},
    parser::Parser,
};

pub struct TableExtractor {
    dialect: Box<dyn Dialect>,
}

impl TableExtractor {
    pub fn for_driver(driver: DriverType) -> Self {
        let dialect: Box<dyn Dialect> = match driver {
            DriverType::SQLite => Box::new(GenericDialect {}),
            DriverType::PostgreSQL => Box::new(PostgreSqlDialect {}),
            DriverType::MySQL => Box::new(MySqlDialect {}),
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
            return self.extract_from_table_factor(&first_table.relation, alias);
        }
        None
    }

    fn extract_from_table_factor(
        &self,
        table_factor: &TableFactor,
        alias: bool,
    ) -> Option<String> {
        match table_factor {
            TableFactor::Table {
                name,
                alias: table_alias,
                ..
            } => {
                if alias
                    && let Some(a) = table_alias
                {
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
}
