//! SQLite Database Implementation for Blanco SQL Editor
//!
//! This crate provides SQLite-specific functionality including:
//! - Database connection management
//! - SQL parsing with SQLite dialect

pub mod connection;
pub mod factory;
pub mod schema;
pub mod sql_parser;

// Re-export main types for convenience
pub use connection::{SqliteConnection, SqliteConnectionKey};

pub use sql_parser::{CompletionKind, ParsedQuery, SqliteTableExtractor, TableAlias};

pub use factory::SqliteConnectionFactory;

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::{sqlite::SqlitePoolOptions, Row};
    use std::env;
    use tempfile::NamedTempFile;

    #[smol::test]
    async fn test_sqlite_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        // Use environment variable for connection string or fallback to temporary file
        let connection_string = env::var("SQLITE_CONNECTION_STRING").unwrap_or_else(|_| {
            // Create a temporary file for SQLite database
            let temp_file = NamedTempFile::new().expect("Failed to create temporary file");
            let path = temp_file.path().to_string_lossy().to_string();
            // Keep the temporary file alive by not dropping it
            std::mem::forget(temp_file);
            format!("sqlite:{}", path)
        });

        // Connect to SQLite
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        // Create comprehensive test table with SQLite-specific data types
        let create_table_sql = r#"
            CREATE TABLE IF NOT EXISTS comprehensive_test (
                id INTEGER PRIMARY KEY AUTOINCREMENT,

                -- SQLite numeric types (dynamic typing)
                integer_col INTEGER,
                real_col REAL,
                numeric_col NUMERIC,
                bigint_col BIGINT,
                int_col INT,
                smallint_col SMALLINT,
                tinyint_col TINYINT,

                -- Text types
                text_col TEXT,
                varchar_col VARCHAR(255),
                char_col CHAR(10),

                -- Blob type (binary data)
                blob_col BLOB,

                -- Boolean (SQLite doesn't have native boolean, stored as INTEGER)
                bool_col BOOLEAN,

                -- Date/Time types (stored as TEXT, REAL, or INTEGER)
                date_col DATE,
                time_col TIME,
                datetime_col DATETIME,
                timestamp_col TIMESTAMP,

                -- JSON types (SQLite 3.38.0+)
                json_col JSON,
                jsonb_col JSONB,

                -- Affinity types
                text_affinity_col TEXT,
                numeric_affinity_col NUMERIC,
                integer_affinity_col INTEGER,
                real_affinity_col REAL,
                blob_affinity_col BLOB,

                -- Custom types
                custom_type_col CUSTOM_TYPE
            )
        "#;

        sqlx::query(create_table_sql).execute(&pool).await?;

        // Insert test data with various SQLite types
        let insert_sql = r#"
            INSERT INTO comprehensive_test (
                integer_col, real_col, numeric_col, bigint_col, int_col, smallint_col, tinyint_col,
                text_col, varchar_col, char_col, blob_col, bool_col,
                date_col, time_col, datetime_col, timestamp_col,
                json_col, jsonb_col,
                text_affinity_col, numeric_affinity_col, integer_affinity_col, real_affinity_col, blob_affinity_col,
                custom_type_col
            ) VALUES (
                2147483647, 12345.6789, 98765.43210, 9223372036854775807, 123456, 32767, 255,
                'This is a test text with unicode: ñiño 你好 🚀', 'variable_string', 'fixed_len',
                x'48656c6c6f20576f726c64', 1,
                '2025-10-30', '20:41:00', '2025-10-30 20:41:00', '2025-10-30 20:41:00',
                '{"name": "test", "value": 42, "active": true}',
                '{"nested": {"array": [1,2,3], "text": "hello"}}',
                'text affinity', 123.456, 999, 3.14159, x'48656c6c6f',
                'custom value'
            )
        "#;

        let result = sqlx::query(insert_sql).execute(&pool).await?;

        // For SQLite, let's create a new table using the Blanco connection to test it properly
        let mut sqlite_connection = crate::SqliteConnection::new(connection_string.clone())?;

        // Establish the actual database connection
        sqlite_connection.connect(&connection_string).await?;

        // Create the test table using Blanco connection
        sqlite_connection
            .execute_query(create_table_sql, None, None)
            .await?;

        // Insert test data using Blanco connection
        sqlite_connection
            .execute_query(insert_sql, None, None)
            .await?;

        // Test a simple query to make sure Blanco can handle the data
        let query_result = sqlite_connection.execute_query("SELECT * FROM comprehensive_test WHERE id = (SELECT MAX(id) FROM comprehensive_test)", None, None).await?;

        assert!(
            !query_result.rows.is_empty(),
            "Query should return at least one row"
        );
        let first_row = query_result.rows.first().unwrap();

        // Create a map for easy lookup of values by column name
        let value_map: std::collections::HashMap<String, String> = query_result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| (col.clone(), first_row[i].clone()))
            .collect();

        // Test integer affinity types
        assert!(
            value_map.get("id").unwrap().parse::<i64>().unwrap() > 0,
            "id should be positive"
        );
        assert_eq!(
            value_map.get("integer_col"),
            Some(&"2147483647".to_string())
        );
        assert_eq!(
            value_map.get("bigint_col"),
            Some(&"9223372036854775807".to_string())
        );
        assert_eq!(value_map.get("int_col"), Some(&"123456".to_string()));
        assert_eq!(value_map.get("smallint_col"), Some(&"32767".to_string()));
        assert_eq!(value_map.get("tinyint_col"), Some(&"255".to_string()));

        // Test numeric affinity types
        assert!(value_map.get("real_col").unwrap().contains("12345"));
        assert!(value_map.get("numeric_col").unwrap().contains("98765"));
        assert!(value_map
            .get("numeric_affinity_col")
            .unwrap()
            .contains("123"));
        assert!(value_map
            .get("real_affinity_col")
            .unwrap()
            .contains("3.14159"));

        // Test text affinity types
        assert_eq!(
            value_map.get("text_col"),
            Some(&"This is a test text with unicode: ñiño 你好 🚀".to_string())
        );
        assert_eq!(
            value_map.get("varchar_col"),
            Some(&"variable_string".to_string())
        );
        assert_eq!(
            value_map.get("text_affinity_col"),
            Some(&"text affinity".to_string())
        );

        // Test CHAR type (may be padded)
        assert!(value_map.get("char_col").unwrap().starts_with("fixed_len"));

        // Test boolean (stored as integer in SQLite)
        assert!(
            value_map.get("bool_col") == Some(&"1".to_string())
                || value_map.get("bool_col") == Some(&"true".to_string()),
            "Boolean should be stored as 1 or true in SQLite"
        );

        // Test date/time types
        assert!(value_map.get("date_col").unwrap().contains("2025"));
        assert!(value_map.get("time_col").unwrap().contains("20:41"));
        assert!(value_map.get("datetime_col").unwrap().contains("2025"));
        assert!(value_map.get("timestamp_col").unwrap().contains("2025"));

        // Test JSON types
        assert!(value_map.get("json_col").unwrap().contains("\"name\""));
        assert!(value_map.get("json_col").unwrap().contains("test"));
        assert!(value_map.get("jsonb_col").unwrap().contains("nested"));

        // Test blob affinity types (SQLite converts binary data to text when possible)
        assert!(value_map.get("blob_col").unwrap().contains("48656c6c6f")); // "Hello World"
        assert!(value_map
            .get("blob_affinity_col")
            .unwrap()
            .contains("48656c6c6f")); // "Hello"

        // Test custom types (should fall back to string conversion)
        assert_eq!(
            value_map.get("custom_type_col"),
            Some(&"custom value".to_string())
        );

        println!("✅ All SQLite type conversion tests passed!");
        println!("✅ Integer affinity types working correctly!");
        println!("✅ Text affinity types working correctly!");
        println!("✅ Numeric affinity types working correctly!");
        println!("✅ BLOB affinity types working correctly!");
        println!("✅ Date/time types working correctly!");
        println!("✅ JSON types working correctly!");
        println!("✅ Custom types working correctly!");

        Ok(())
    }
}
