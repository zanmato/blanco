//! SQLite Database Implementation for Blanco SQL Editor
//!
//! This crate provides SQLite-specific functionality including:
//! - Database connection management
//! - SQL parsing with SQLite dialect
//! - Auto-completion for SQLite queries
//! - Hover information for tables and columns

pub mod connection;
pub mod sql_parser;
pub mod completion;
pub mod hover;
pub mod factory;

// Re-export main types for convenience
pub use connection::{
    SqliteConnection, SqliteConnectionKey
};

pub use sql_parser::{
    SqliteTableExtractor, CompletionKind, ParsedQuery, TableAlias
};

pub use factory::SqliteConnectionFactory;

// Import completion and hover implementations to make them available
// use completion::*;
// use hover::*;

#[cfg(test)]
mod tests {
    use sqlx::{Row, sqlite::SqlitePoolOptions};
    use std::env;
    use blanco_core::Connection;
    use tempfile::NamedTempFile;

    #[async_std::test]
    async fn test_sqlite_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        // Use environment variable for connection string or fallback to temporary file
        let connection_string = env::var("SQLITE_CONNECTION_STRING")
            .unwrap_or_else(|_| {
                // Create a temporary file for SQLite database
                let temp_file = NamedTempFile::new().expect("Failed to create temporary file");
                let path = temp_file.path().to_string_lossy().to_string();
                // Keep the temporary file alive by not dropping it
                std::mem::forget(temp_file);
                format!("sqlite:{}", path)
            });

        println!("Testing SQLite data types with connection: {}", connection_string);

        // Connect to SQLite
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        println!("Successfully connected to SQLite!");

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

        println!("Creating comprehensive SQLite test table...");
        sqlx::query(create_table_sql).execute(&pool).await?;
        println!("Table created successfully!");

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

        println!("Inserting test data...");
        let result = sqlx::query(insert_sql).execute(&pool).await?;
        println!("Test data inserted successfully! Rows affected: {}", result.rows_affected());

        // Read the data back and verify serialization
        println!("\n=== Testing Data Type Serialization ===\n");

        let select_sql = r#"
            SELECT
                id, integer_col, real_col, numeric_col, bigint_col, int_col, smallint_col, tinyint_col,
                text_col, varchar_col, char_col, blob_col, bool_col,
                date_col, time_col, datetime_col, timestamp_col,
                json_col, jsonb_col,
                text_affinity_col, numeric_affinity_col, integer_affinity_col, real_affinity_col, blob_affinity_col,
                custom_type_col
            FROM comprehensive_test
            WHERE id = (SELECT MAX(id) FROM comprehensive_test)
        "#;

        let row = sqlx::query(select_sql).fetch_one(&pool).await?;

        // Helper function to safely extract and print values
        fn safe_print<T: std::fmt::Display + sqlx::Type<sqlx::Sqlite> + for<'r> sqlx::Decode<'r, sqlx::Sqlite>>(name: &str, row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<(), sqlx::Error> {
            match row.try_get::<Option<T>, _>(column) {
                Ok(Some(value)) => println!("{:<25}: {} ({})", name, value, std::any::type_name::<T>()),
                Ok(None) => println!("{:<25}: NULL", name),
                Err(e) => println!("{:<25}: ERROR - {} ({})", name, e, std::any::type_name::<T>()),
            }
            Ok(())
        }

        // Helper function for blob types
        fn safe_print_blob(name: &str, row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<(), sqlx::Error> {
            match row.try_get::<Option<Vec<u8>>, _>(column) {
                Ok(Some(bytes)) => {
                    let hex_str = bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();
                    println!("{:<25}: {} (blob: {} bytes)", name, hex_str, bytes.len());
                },
                Ok(None) => println!("{:<25}: NULL", name),
                Err(e) => println!("{:<25}: ERROR - {} ({})", name, e, std::any::type_name::<Vec<u8>>()),
            }
            Ok(())
        }

        // Test SQLite data types
        println!("--- Numeric Types (SQLite Dynamic Typing) ---");
        safe_print::<i64>("integer_col", &row, "integer_col")?;
        safe_print::<f64>("real_col", &row, "real_col")?;
        safe_print::<f64>("numeric_col", &row, "numeric_col")?;
        safe_print::<i64>("bigint_col", &row, "bigint_col")?;
        safe_print::<i64>("int_col", &row, "int_col")?;
        safe_print::<i64>("smallint_col", &row, "smallint_col")?;
        safe_print::<i64>("tinyint_col", &row, "tinyint_col")?;

        println!("\n--- Text Types ---");
        safe_print::<String>("text_col", &row, "text_col")?;
        safe_print::<String>("varchar_col", &row, "varchar_col")?;
        safe_print::<String>("char_col", &row, "char_col")?;

        println!("\n--- Binary Data ---");
        safe_print_blob("blob_col", &row, "blob_col")?;

        println!("\n--- Boolean (stored as INTEGER) ---");
        safe_print::<i64>("bool_col", &row, "bool_col")?;

        println!("\n--- Date/Time Types ---");
        safe_print::<String>("date_col", &row, "date_col")?;
        safe_print::<String>("time_col", &row, "time_col")?;
        safe_print::<String>("datetime_col", &row, "datetime_col")?;
        safe_print::<String>("timestamp_col", &row, "timestamp_col")?;

        println!("\n--- JSON Types ---");
        safe_print::<String>("json_col", &row, "json_col")?;
        safe_print::<String>("jsonb_col", &row, "jsonb_col")?;

        println!("\n--- Affinity Types ---");
        safe_print::<String>("text_affinity_col", &row, "text_affinity_col")?;
        safe_print::<f64>("numeric_affinity_col", &row, "numeric_affinity_col")?;
        safe_print::<i64>("integer_affinity_col", &row, "integer_affinity_col")?;
        safe_print::<f64>("real_affinity_col", &row, "real_affinity_col")?;
        safe_print_blob("blob_affinity_col", &row, "blob_affinity_col")?;

        println!("\n--- Custom Type ---");
        safe_print::<String>("custom_type_col", &row, "custom_type_col")?;

        println!("\n=== Testing Blanco SQLite Connection ===\n");

        // For SQLite, let's create a new table using the Blanco connection to test it properly
        let mut sqlite_connection = crate::SqliteConnection::new(connection_string.clone())?;

        // Establish the actual database connection
        sqlite_connection.connect(&connection_string).await?;

        // Create the test table using Blanco connection
        sqlite_connection.execute_query(create_table_sql).await?;

        // Insert test data using Blanco connection
        sqlite_connection.execute_query(insert_sql).await?;

        // Test a simple query to make sure Blanco can handle the data
        let query_result = sqlite_connection.execute_query("SELECT * FROM comprehensive_test WHERE id = (SELECT MAX(id) FROM comprehensive_test)").await?;

        println!("Blanco SQLite connection test successful!");
        println!("Columns returned: {}", query_result.columns.len());
        println!("Rows returned: {}", query_result.rows.len());

        if let Some(first_row) = query_result.rows.first() {
            println!("\n--- Blanco Serialization Results ---");
            for (i, value) in first_row.iter().enumerate() {
                let column_name = query_result.columns.get(i).cloned().unwrap_or_else(|| "unknown".to_string());
                println!("{:<25}: {}", column_name, value);
            }
        }

        println!("\n=== SQLite Data Types Test Completed Successfully! ===");
        Ok(())
    }
}