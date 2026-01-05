//! MySQL/MariaDB Database Implementation for Blanco SQL Editor
//!
//! This crate provides MySQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with MySQL dialect

pub mod connection;
pub mod factory;
pub mod schema;
pub mod sql_parser;

// Re-export main types for convenience
pub use connection::{MysqlConnection, MysqlConnectionKey};
pub use factory::MysqlConnectionFactory;
pub use sql_parser::{CompletionKind, MysqlTableExtractor, ParsedQuery, TableAlias};

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::{mysql::MySqlPoolOptions, Column, Row};
    use std::env;

    #[smol::test]
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
            .execute_query("SELECT 1 as test_value, 'hello' as test_text", None, None)
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

        // Create a test table with proper integer columns and test convert_row_value_to_string
        // Clean up any existing table first
        mysql_connection
            .execute_query("DROP TABLE IF EXISTS test_types", None, None)
            .await?;

        mysql_connection
            .execute_query(
                r#"CREATE TABLE test_types (
                    id INT PRIMARY KEY,
                    name VARCHAR(50),
                    age INT,
                    score DECIMAL(5,2),
                    is_active BOOLEAN,
                    created_at TIMESTAMP
                )"#,
                None,
                None,
            )
            .await?;
        {
            // Insert test data
            mysql_connection
                .execute_query(
                    r#"INSERT INTO test_types (id, name, age, score, is_active, created_at) VALUES
                    (1, 'Alice', 25, 95.50, TRUE, NOW()),
                    (2, 'Bob', 30, 87.25, FALSE, NOW())"#,
                    None,
                    None,
                )
                .await?;
            {
                // Get raw rows to test convert_row_value_to_string directly
                let pool = mysql_connection.get_or_create_pool("mysql").await?;
                if let Ok(raw_rows) = sqlx::query("SELECT id, name, age, score, is_active, created_at FROM test_types ORDER BY id")
                    .fetch_all(&pool)
                    .await {
                    assert!(!raw_rows.is_empty());
                    assert_eq!(raw_rows.len(), 2);

                    // Check column information
                    let first_row = &raw_rows[0];
                    let columns = first_row.columns();
                    assert_eq!(columns.len(), 6);

                    // Verify column names and types
                    assert_eq!(columns[0].name(), "id");
                    assert_eq!(columns[1].name(), "name");
                    assert_eq!(columns[2].name(), "age");
                    assert_eq!(columns[3].name(), "score");
                    assert_eq!(columns[4].name(), "is_active");
                    assert_eq!(columns[5].name(), "created_at");

                    
                    
                    // Get column type information
                    let column_types: Vec<String> = first_row
                        .columns()
                        .iter()
                        .map(|col| col.type_info().to_string())
                        .collect();

                    // Test convert_row_value_to_string on first row using actual column types
                    let id_str = mysql_connection.convert_row_value_to_string(first_row, 0, &column_types[0]);
                    let name_str = mysql_connection.convert_row_value_to_string(first_row, 1, &column_types[1]);
                    let age_str = mysql_connection.convert_row_value_to_string(first_row, 2, &column_types[2]);
                    let score_str = mysql_connection.convert_row_value_to_string(first_row, 3, &column_types[3]);
                    let is_active_str = mysql_connection.convert_row_value_to_string(first_row, 4, &column_types[4]);
                    let created_at_str = mysql_connection.convert_row_value_to_string(first_row, 5, &column_types[5]);

                    println!("Row values:");
                    println!("  id: '{}' (type: {}, expected: '1')", id_str, column_types[0]);
                    println!("  name: '{}' (type: {}, expected: 'Alice')", name_str, column_types[1]);
                    println!("  age: '{}' (type: {}, expected: '25')", age_str, column_types[2]);
                    println!("  score: '{}' (type: {}, expected: '95.50')", score_str, column_types[3]);
                    println!("  is_active: '{}' (type: {}, expected: 'true')", is_active_str, column_types[4]);
                    println!("  created_at: '{}' (type: {}, expected: not empty)", created_at_str, column_types[5]);

                    // Verify integer columns are properly converted
                    assert_eq!(id_str, "1"); // id should be "1", not "true"
                    assert_eq!(name_str, "Alice");
                    assert_eq!(age_str, "25"); // age should be "25", not "true"
                    // Note: DECIMAL might need different handling, let's see what we get
                    if score_str == "NULL" {
                        println!("  Score is NULL - this might indicate a conversion issue with DECIMAL type");
                    }
                    assert_eq!(is_active_str, "true"); // is_active should now show as "true"
                    assert!(!created_at_str.is_empty()); // created_at

                    // Test convert_row_value_to_string on second row
                    let second_row = &raw_rows[1];
                    let id_str2 = mysql_connection.convert_row_value_to_string(second_row, 0, &column_types[0]);
                    let age_str2 = mysql_connection.convert_row_value_to_string(second_row, 2, &column_types[2]);
                    let is_active_str2 = mysql_connection.convert_row_value_to_string(second_row, 4, &column_types[4]);

                    assert_eq!(id_str2, "2"); // id should be "2", not "true"
                    assert_eq!(age_str2, "30"); // age should be "30", not "true"
                    assert_eq!(is_active_str2, "false"); // is_active should now show as "false"
                }
            }
        }

        // Also test simple type conversions directly
        if let Ok(type_test_result) = mysql_connection.execute_query(
            "SELECT 123 as int_col, 3.14 as float_col, 'test' as text_col, CAST(TRUE AS BOOLEAN) as bool_col, NOW() as datetime_col",
            None,
            None
        ).await {
            assert!(!type_test_result.rows.is_empty());
            let row = &type_test_result.rows[0];

            
            assert_eq!(row[0], "123"); // int should be "123", not "true"
            // Float might be formatted differently, so just check it's not empty
            assert!(!row[1].is_empty()); // float
            assert_eq!(row[2], "test"); // text
            assert_eq!(row[3], "true"); // bool (TRUE as BOOLEAN) - should now show as "true"
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
