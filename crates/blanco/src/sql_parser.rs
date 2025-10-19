use anyhow::{anyhow, Result};
use sql_parse::{parse_statement, parse_statements, ParseOptions, SQLDialect, Statement, TableReference};

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

        // Fallback to simple regex-based parsing for edge cases
        self.extract_with_regex(sql)
    }

    /// Extract table name from parsed statement
    fn extract_from_statement(&self, statement: &Statement) -> Option<String> {
        match statement {
            Statement::Select(select) => self.extract_from_select(select),
            Statement::InsertReplace(insert) => {
                // Extract table name from INSERT statement
                if let Some(last_id) = insert.table.last() {
                    Some(last_id.value.to_string())
                } else {
                    None
                }
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
                    if let Some(last_id) = first_table_vec.last() {
                        Some(last_id.value.to_string())
                    } else {
                        None
                    }
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
            TableReference::Table { identifier, as_, .. } => {
                // For direct table references, use the alias if provided, otherwise use the last part of the identifier
                if let Some(alias) = as_ {
                    Some(alias.value.to_string())
                } else if let Some(last_id) = identifier.last() {
                    Some(last_id.value.to_string())
                } else {
                    None
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

    /// Fallback regex-based parser for edge cases
    fn extract_with_regex(&self, sql: &str) -> Result<String> {
        let query_lower = sql.to_lowercase();
        let query_lower = query_lower.trim();

        // Handle SELECT queries
        if query_lower.starts_with("select") {
            if let Some(from_pos) = query_lower.find("from") {
                let after_from = &sql[from_pos + 4..];
                let table_part = after_from.split_whitespace().next()
                    .ok_or_else(|| anyhow!("No table name found in query"))?;

                // Clean up table name
                let mut table_name = table_part
                    .trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == ';')
                    .to_string();

                // Handle database.schema.table format
                if let Some(dot_pos) = table_name.rfind('.') {
                    table_name = table_name[dot_pos + 1..].to_string();
                }

                return Ok(table_name);
            }
        }

        // Handle INSERT queries
        if query_lower.starts_with("insert") {
            if let Some(into_pos) = query_lower.find("into") {
                let after_into = &sql[into_pos + 4..];
                let table_part = after_into.split_whitespace().next()
                    .ok_or_else(|| anyhow!("No table name found after INTO"))?;
                let table_name = table_part
                    .trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == ';')
                    .to_string();
                return Ok(table_name);
            }
        }

        // Handle UPDATE queries
        if query_lower.starts_with("update") {
            let after_update = &sql[6..];
            let table_part = after_update.split_whitespace().next()
                    .ok_or_else(|| anyhow!("No table name found after UPDATE"))?;
            let table_name = table_part
                .trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == ';')
                .to_string();
            return Ok(table_name);
        }

        // Handle DELETE queries
        if query_lower.starts_with("delete") {
            if let Some(from_pos) = query_lower.find("from") {
                let after_from = &sql[from_pos + 4..];
                let table_part = after_from.split_whitespace().next()
                    .ok_or_else(|| anyhow!("No table name found in query"))?;
                let table_name = table_part
                    .trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == ';')
                    .to_string();
                return Ok(table_name);
            }
        }

        Err(anyhow!("Could not extract table name from query"))
    }

    /// Extract all table names from a query (useful for debugging)
    pub fn extract_all_tables(&self, sql: &str) -> Result<Vec<String>> {
        let mut tables = Vec::new();
        let options = ParseOptions::new().dialect(self.dialect.clone());
        let mut issues = Vec::new();

        // Parse all statements
        let statements = parse_statements(sql, &mut issues, &options);
        for statement in statements {
            self.collect_tables_from_statement(&statement, &mut tables);
        }

        // Remove duplicates while preserving order
        let mut unique_tables = Vec::new();
        for table in tables {
            if !unique_tables.contains(&table) {
                unique_tables.push(table);
            }
        }

        Ok(unique_tables)
    }

    /// Recursively collect table names from statement
    fn collect_tables_from_statement(&self, statement: &Statement, tables: &mut Vec<String>) {
        match statement {
            Statement::Select(select) => {
                if let Some(table_references) = &select.table_references {
                    for table_ref in table_references {
                        self.collect_tables_from_table_reference(table_ref, tables);
                    }
                }
            }
            Statement::InsertReplace(insert) => {
                if let Some(last_id) = insert.table.last() {
                    let table_name = last_id.value.to_string();
                    if !tables.contains(&table_name) {
                        tables.push(table_name);
                    }
                }
            }
            Statement::Update(update) => {
                for table_ref in &update.tables {
                    self.collect_tables_from_table_reference(table_ref, tables);
                }
            }
            Statement::Delete(delete) => {
                for table_vec in &delete.tables {
                    if let Some(last_id) = table_vec.last() {
                        let table_name = last_id.value.to_string();
                        if !tables.contains(&table_name) {
                            tables.push(table_name);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Collect tables from table reference (handles joins, subqueries, etc.)
    fn collect_tables_from_table_reference(&self, table_ref: &TableReference, tables: &mut Vec<String>) {
        match table_ref {
            TableReference::Table { identifier, .. } => {
                if let Some(last_id) = identifier.last() {
                    let table_name = last_id.value.to_string();
                    if !tables.contains(&table_name) {
                        tables.push(table_name);
                    }
                }
            }
            TableReference::Query { query, .. } => {
                self.collect_tables_from_statement(query, tables);
            }
            TableReference::Join { left, right, .. } => {
                self.collect_tables_from_table_reference(left, tables);
                self.collect_tables_from_table_reference(right, tables);
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
            extractor.extract_primary_table("SELECT * FROM users").unwrap(),
            "users"
        );
        assert_eq!(
            extractor.extract_primary_table("SELECT id, name FROM products").unwrap(),
            "products"
        );
    }

    #[test]
    fn test_quoted_table_names() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM \"my-table\"").unwrap(),
            "my-table"
        );
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM `table_name`").unwrap(),
            "table_name"
        );
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM 'table'").unwrap(),
            "table"
        );
    }

    #[test]
    fn test_database_schema_prefix() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM mydb.users").unwrap(),
            "users"
        );
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM public.customers").unwrap(),
            "customers"
        );
    }

    #[test]
    fn test_insert_queries() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor.extract_primary_table("INSERT INTO users (name) VALUES ('test')").unwrap(),
            "users"
        );
        assert_eq!(
            extractor.extract_primary_table("INSERT INTO `orders` (product_id) VALUES (1)").unwrap(),
            "orders"
        );
    }

    #[test]
    fn test_update_queries() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor.extract_primary_table("UPDATE users SET name = 'test'").unwrap(),
            "users"
        );
        assert_eq!(
            extractor.extract_primary_table("UPDATE products SET price = 10.99").unwrap(),
            "products"
        );
    }

    #[test]
    fn test_delete_queries() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor.extract_primary_table("DELETE FROM users WHERE id = 1").unwrap(),
            "users"
        );
        assert_eq!(
            extractor.extract_primary_table("DELETE FROM orders WHERE status = 'cancelled'").unwrap(),
            "orders"
        );
    }

    #[test]
    fn test_extract_all_tables() {
        let extractor = SqlTableExtractor::new();
        let tables = extractor.extract_all_tables(
            "SELECT u.*, p.* FROM users u JOIN profiles p ON u.id = p.user_id"
        ).unwrap();
        assert_eq!(tables, vec!["users", "profiles"]);
    }

    #[test]
    fn test_subqueries() {
        let extractor = SqlTableExtractor::new();
        // Test with alias
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM (SELECT * FROM users) AS t").unwrap(),
            "t"
        );
        // Test without alias
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM (SELECT * FROM products)").unwrap(),
            "products"
        );
    }

    #[test]
    fn test_whitespace_and_formatting() {
        let extractor = SqlTableExtractor::new();
        assert_eq!(
            extractor.extract_primary_table("  SELECT   *   FROM    users  ").unwrap(),
            "users"
        );
        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM users;").unwrap(),
            "users"
        );
    }
}