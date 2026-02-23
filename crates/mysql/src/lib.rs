//! MySQL/MariaDB Database Implementation for Blanco SQL Editor
//!
//! This crate provides MySQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with MySQL dialect

pub mod connection;
pub mod schema;
pub mod sql_parser;

// Re-export main types for convenience
pub use connection::{MysqlConnection, MysqlConnectionKey};
pub use sql_parser::{CompletionKind, MysqlTableExtractor, ParsedQuery, TableAlias};

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::Row;
    use std::env;

    #[test]
    fn test_mysql_basic_connection() -> Result<(), Box<dyn std::error::Error>> {
        smol::block_on(async {
            let connection_string = env::var("MYSQL_CONNECTION_STRING")
                .unwrap_or_else(|_| "mysql://root:blanco@172.19.0.2:3306/mysql".to_string());

            let mysql_connection = crate::MysqlConnection::from_connection_string(&connection_string)?;

            // Test basic query that should work without creating tables
            let simple_result = mysql_connection
                .execute_query("SELECT 1 as test_value, 'hello' as test_text", None, None)
                .await;

            match simple_result {
                Ok(result) => {
                    assert!(!result.rows.is_empty(), "Simple query should return results");
                    assert_eq!(result.columns.len(), 2);
                    assert_eq!(result.columns[0], "test_value");
                    assert_eq!(result.columns[1], "test_text");
                    println!("MySQL connection successful");
                }
                Err(e) => {
                    println!(
                        "MySQL connection failed (this is expected if database is not accessible): {}",
                        e
                    );
                    return Ok(());
                }
            }

            // Test connection metadata
            assert_eq!(mysql_connection.get_connection_type(), "MySQL");
            assert!(mysql_connection.get_display_name().contains("MySQL"));

            // Test table extraction
            let from_table = mysql_connection
                .extract_table_name_from_query("SELECT * FROM users WHERE id = 1", false)?;
            assert_eq!(from_table, Some("users".to_string()));

            Ok(())
        })
    }

    #[test]
    fn test_mysql_indexes() -> Result<(), Box<dyn std::error::Error>> {
        smol::block_on(async {
            let connection_string = env::var("MYSQL_CONNECTION_STRING")
                .unwrap_or_else(|_| "mysql://root:blanco@172.19.0.2:3306/mysql".to_string());

            let mysql_connection = crate::MysqlConnection::from_connection_string(&connection_string)?;

            // Test basic connection first
            let simple_result = mysql_connection
                .execute_query("SELECT 1 as test_value", None, None)
                .await;

            if simple_result.is_err() {
                println!("MySQL connection failed, skipping index test");
                return Ok(());
            }

            // Drop test table if exists
            let _ = mysql_connection
                .execute_query("DROP TABLE IF EXISTS test_indexes", None, None)
                .await;

            // Create a test table with various indexes
            mysql_connection
                .execute_query(
                    r#"CREATE TABLE test_indexes (
                        id INT PRIMARY KEY,
                        email VARCHAR(100),
                        name VARCHAR(50),
                        created_at TIMESTAMP,
                        INDEX idx_email (email),
                        INDEX idx_name_email (name, email)
                    )"#,
                    None,
                    None,
                )
                .await?;

            // Get indexes for the table
            let indexes = mysql_connection
                .get_indexes_for_table("test_indexes", None)
                .await?;

            println!("Found {} indexes:", indexes.len());
            for idx in &indexes {
                println!(
                    "  - {} (unique: {}, columns: {:?}, algorithm: {})",
                    idx.name, idx.is_unique, idx.column_names, idx.algorithm
                );
            }

            // Verify we have at least the PRIMARY key and the two indexes we created
            assert!(indexes.len() >= 2, "Expected at least 2 indexes, got {}", indexes.len());

            // Check for PRIMARY key
            let primary = indexes.iter().find(|i| i.name == "PRIMARY");
            assert!(primary.is_some(), "PRIMARY key index not found");
            let primary = primary.unwrap();
            assert!(primary.is_unique, "PRIMARY key should be unique");
            assert!(primary.column_names.contains(&"id".to_string()), "PRIMARY key should contain 'id' column");

            // Check for idx_email
            let email_idx = indexes.iter().find(|i| i.name == "idx_email");
            assert!(email_idx.is_some(), "idx_email index not found");
            let email_idx = email_idx.unwrap();
            assert!(!email_idx.is_unique, "idx_email should not be unique");
            assert_eq!(email_idx.column_names, vec!["email"], "idx_email should have email column");

            // Check for idx_name_email (composite index)
            let name_email_idx = indexes.iter().find(|i| i.name == "idx_name_email");
            assert!(name_email_idx.is_some(), "idx_name_email index not found");
            let name_email_idx = name_email_idx.unwrap();
            assert!(!name_email_idx.is_unique, "idx_name_email should not be unique");
            assert_eq!(name_email_idx.column_names, vec!["name", "email"], "idx_name_email should have name and email columns");

            // Clean up
            let _ = mysql_connection
                .execute_query("DROP TABLE IF EXISTS test_indexes", None, None)
                .await;

            println!("MySQL indexes test passed!");
            Ok(())
        })
    }
}
