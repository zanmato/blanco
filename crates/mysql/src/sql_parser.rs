use anyhow::{Result, anyhow};
use sqlparser::{
    ast::{SetExpr, Statement, TableFactor, TableObject},
    dialect::MySqlDialect,
    parser::Parser,
};

/// MySQL-specific SQL table extractor
pub struct MysqlTableExtractor {}

impl MysqlTableExtractor {
    pub fn new() -> Self {
        Self {}
    }

    /// Parse SQL and return the first statement if successful
    fn parse_sql(&self, sql: &str) -> Result<Option<Statement>> {
        let dialect = MySqlDialect {};
        match Parser::parse_sql(&dialect, sql) {
            Ok(statements) => Ok(statements.into_iter().next()),
            Err(_) => Ok(None),
        }
    }

    /// Extract the table name from a SELECT query
    pub fn extract_table(&self, sql: &str, alias: bool) -> Result<String> {
        if let Some(statement) = self.parse_sql(sql)? {
            if let Some(table_name) = self.extract_from_statement(&statement, alias) {
                return Ok(table_name);
            }
        }

        Err(anyhow!("Could not extract table name from query"))
    }

    /// Extract table name from parsed statement
    fn extract_from_statement(&self, statement: &Statement, alias: bool) -> Option<String> {
        match statement {
            Statement::Query(query) => self.extract_from_query(query, alias),
            Statement::Insert(insert) => match &insert.table {
                TableObject::TableName(name) => Some(name.to_string()),
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

    /// Extract table name from a Query
    fn extract_from_query(&self, query: &sqlparser::ast::Query, alias: bool) -> Option<String> {
        if let SetExpr::Select(select) = &*query.body {
            if let Some(first_table) = select.from.first() {
                return self.extract_from_table_factor(&first_table.relation, alias);
            }
        }
        None
    }

    /// Extract table name from a table factor
    fn extract_from_table_factor(&self, table_factor: &TableFactor, alias: bool) -> Option<String> {
        match table_factor {
            TableFactor::Table {
                name,
                alias: table_alias,
                ..
            } => {
                if alias {
                    if let Some(a) = table_alias {
                        return Some(a.name.to_string());
                    }
                }
                Some(name.to_string())
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

impl Default for MysqlTableExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mysql_table_extraction() {
        let extractor = MysqlTableExtractor::new();

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
            "public.users"
        );
    }
}
