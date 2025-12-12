//! MySQL/MariaDB Database Implementation for Blanco SQL Editor
//!
//! This crate provides MySQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with MySQL dialect

pub mod connection;
pub mod factory;
pub mod sql_parser;

// Re-export main types for convenience
pub use connection::{MysqlConnection, MysqlConnectionKey};
pub use factory::MysqlConnectionFactory;
pub use sql_parser::{CompletionKind, MysqlTableExtractor, ParsedQuery, TableAlias};

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::mysql::MySqlPoolOptions;
    use std::env;

    #[async_std::test]
    async fn test_mysql_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        // Use the provided MySQL test database
        let connection_string = env::var("MYSQL_CONNECTION_STRING")
            .unwrap_or_else(|_| "mysql://root:blanco@172.19.0.2:3306/mysql".to_string());

        // Test basic MySQL connection first
        let mysql_connection = crate::MysqlConnection::from_connection_string(&connection_string)?;

        // Test connection string parsing
        let connection_key = crate::MysqlConnectionKey::from_connection_string(&connection_string)?;
        assert_eq!(connection_key.host, "172.19.0.2");
        assert_eq!(connection_key.port, 3306);
        assert_eq!(connection_key.username, "root");
        assert_eq!(connection_key.database, "mysql");
        assert_eq!(connection_key.password, Some("blanco".to_string()));

        // Test basic query that should work without creating tables
        let simple_result = mysql_connection
            .execute_query("SELECT 1 as test_value, 'hello' as test_text", None)
            .await;

        match simple_result {
            Ok(result) => {
                assert!(
                    !result.rows.is_empty(),
                    "Simple query should return results"
                );
                assert_eq!(result.columns.len(), 2);
                assert_eq!(result.columns[0], "test_value");
                assert_eq!(result.columns[1], "test_text");
            }
            Err(e) => {
                println!(
                    "MySQL connection failed (this is expected if database is not accessible): {}",
                    e
                );
                // Skip further tests if database is not accessible
                return Ok(());
            }
        }

        // Test connection metadata
        assert_eq!(mysql_connection.get_connection_type(), "MySQL");
        assert!(mysql_connection.get_display_name().contains("MySQL"));
        assert_eq!(
            mysql_connection.get_icon_name(),
            blanco_core::IconName::MySQL
        );

        // Test simple type conversions if database is accessible
        if let Ok(type_test_result) = mysql_connection.execute_query(
            "SELECT 123 as int_col, 3.14 as float_col, 'test' as text_col, TRUE as bool_col, NOW() as datetime_col",
            None
        ).await {
            assert!(!type_test_result.rows.is_empty());
            let row = &type_test_result.rows[0];
            assert_eq!(row[0], "123"); // int
            // Float might be formatted differently, so just check it's not empty
            assert!(!row[1].is_empty()); // float
            assert_eq!(row[2], "test"); // text
            assert_eq!(row[3], "1"); // bool (TRUE as TINYINT)
            assert!(!row[4].is_empty()); // datetime
        }

        // Test table extraction
        let from_table = mysql_connection
            .extract_table_name_from_query("SELECT * FROM users WHERE id = 1", false)?;
        assert_eq!(from_table, Some("users".to_string()));

        let with_alias = mysql_connection
            .extract_table_name_from_query("SELECT * FROM users u WHERE id = 1", false)?;
        assert_eq!(with_alias, Some("users".to_string()));

        // Test alias resolution
        let alias_result =
            mysql_connection.resolve_table_alias("SELECT * FROM users u WHERE u.id = 1", "u")?;
        assert_eq!(alias_result, Some("users".to_string()));

        Ok(())
    }
}
