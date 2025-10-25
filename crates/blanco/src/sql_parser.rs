use anyhow::{anyhow, Result};
use sql_parse::{parse_statement, ParseOptions, SQLDialect, Statement, TableReference};

/// Extract table names from SQL queries using proper SQL parsing
pub struct SqlTableExtractor {
    dialect: SQLDialect,
}

impl SqlTableExtractor {
    pub fn new() -> Self {
        Self {
            dialect: SQLDialect::MariaDB, // Best available option for general SQL
        }
    }

    /// Extract the primary table name from a SELECT query
    pub fn extract_primary_table(&self, sql: &str) -> Result<String> {
        // Try parsing with sql_parse first
        let options = ParseOptions::new().dialect(self.dialect.clone());
        let mut issues = Vec::new();

        if let Some(statement) = parse_statement(sql, &mut issues, &options) {
            if let Some(table_name) = self.extract_from_statement(&statement) {
                return Ok(table_name);
            }
        }

        Err(anyhow!("Could not extract table name from query"))
    }

    /// Extract table name from parsed statement
    fn extract_from_statement(&self, statement: &Statement) -> Option<String> {
        match statement {
            Statement::Select(select) => self.extract_from_select(select),
            Statement::InsertReplace(insert) => {
                // Extract table name from INSERT statement
                insert.table.last().map(|last_id| last_id.value.to_string())
            }
            Statement::Update(update) => {
                // Extract table name from UPDATE statement
                if let Some(first_table) = update.tables.first() {
                    self.extract_from_table_reference(first_table)
                } else {
                    None
                }
            }
            Statement::Delete(delete) => {
                // Extract table name from DELETE statement
                if let Some(first_table_vec) = delete.tables.first() {
                    first_table_vec
                        .last()
                        .map(|last_id| last_id.value.to_string())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Extract table name from a SELECT statement
    fn extract_from_select(&self, select: &sql_parse::Select) -> Option<String> {
        // Get the main FROM clause
        if let Some(table_references) = &select.table_references {
            if let Some(first_table) = table_references.first() {
                return self.extract_from_table_reference(first_table);
            }
        }
        None
    }

    /// Extract table name from a table reference (handles joins, subqueries, etc.)
    fn extract_from_table_reference(&self, table_ref: &TableReference) -> Option<String> {
        match table_ref {
            TableReference::Table {
                identifier, as_, ..
            } => {
                // For direct table references, use the alias if provided, otherwise use the last part of the identifier
                if let Some(alias) = as_ {
                    Some(alias.value.to_string())
                } else {
                    identifier.last().map(|last_id| last_id.value.to_string())
                }
            }
            TableReference::Query { query, as_, .. } => {
                // For subqueries, use the alias if provided
                if let Some(alias) = as_ {
                    Some(alias.value.to_string())
                } else {
                    // Fallback to extracting from the subquery
                    self.extract_from_statement(query)
                }
            }
            TableReference::Join { left, right, .. } => {
                // For joins, try the left side first
                self.extract_from_table_reference(left)
                    .or_else(|| self.extract_from_table_reference(right))
            }
        }
    }
}

impl Default for SqlTableExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_select() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM users")
                .unwrap(),
            "users"
        );
        assert_eq!(
            extractor
                .extract_primary_table("SELECT id, name FROM products")
                .unwrap(),
            "products"
        );
    }

    #[test]
    fn test_quoted_table_names() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM \"my-table\"")
                .unwrap(),
            "my-table"
        );
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM `table_name`")
                .unwrap(),
            "table_name"
        );
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM 'table'")
                .unwrap(),
            "table"
        );
    }

    #[test]
    fn test_database_schema_prefix() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM mydb.users")
                .unwrap(),
            "users"
        );
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM public.customers")
                .unwrap(),
            "customers"
        );
    }

    #[test]
    fn test_insert_queries() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor
                .extract_primary_table("INSERT INTO users (name) VALUES ('test')")
                .unwrap(),
            "users"
        );
        assert_eq!(
            extractor
                .extract_primary_table("INSERT INTO `orders` (product_id) VALUES (1)")
                .unwrap(),
            "orders"
        );
    }

    #[test]
    fn test_update_queries() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor
                .extract_primary_table("UPDATE users SET name = 'test'")
                .unwrap(),
            "users"
        );
        assert_eq!(
            extractor
                .extract_primary_table("UPDATE products SET price = 10.99")
                .unwrap(),
            "products"
        );
    }

    #[test]
    fn test_delete_queries() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor
                .extract_primary_table("DELETE FROM users WHERE id = 1")
                .unwrap(),
            "users"
        );
        assert_eq!(
            extractor
                .extract_primary_table("DELETE FROM orders WHERE status = 'cancelled'")
                .unwrap(),
            "orders"
        );
    }

    #[test]
    fn test_subqueries() {
        let extractor = SqlTableExtractor::new();
        // Test with alias
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM (SELECT * FROM users) AS t")
                .unwrap(),
            "t"
        );
        // Test without alias
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM (SELECT * FROM products)")
                .unwrap(),
            "products"
        );
    }

    #[test]
    fn test_whitespace_and_formatting() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor
                .extract_primary_table("  SELECT   *   FROM    users  ")
                .unwrap(),
            "users"
        );
        assert_eq!(
            extractor
                .extract_primary_table("SELECT * FROM users;")
                .unwrap(),
            "users"
        );
    }
}
