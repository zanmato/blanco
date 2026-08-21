//! SQLite Database Implementation for Blanco SQL Editor
//!
//! This crate provides SQLite-specific functionality including:
//! - Database connection management
//! - SQL parsing with SQLite dialect

pub mod connection;
pub mod schema;

// Re-export main types for convenience
pub use connection::{SqliteConnection, SqliteConnectionKey};

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::env;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_sqlite_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        async {
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

            let _result = sqlx::query(insert_sql).execute(&pool).await?;

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
            let value_map: std::collections::HashMap<String, Option<String>> = query_result
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| (col.clone(), first_row[i].clone()))
                .collect();

            // Helper to get a value as &str
            let get_val =
                |key: &str| -> &str { value_map.get(key).and_then(|v| v.as_deref()).unwrap_or("") };

            // Test integer affinity types
            assert!(
                get_val("id").parse::<i64>().unwrap() > 0,
                "id should be positive"
            );
            assert_eq!(get_val("integer_col"), "2147483647");
            assert_eq!(get_val("bigint_col"), "9223372036854775807");
            assert_eq!(get_val("int_col"), "123456");
            assert_eq!(get_val("smallint_col"), "32767");
            assert_eq!(get_val("tinyint_col"), "255");

            // Test numeric affinity types
            assert!(get_val("real_col").contains("12345"));
            assert!(get_val("numeric_col").contains("98765"));
            assert!(get_val("numeric_affinity_col").contains("123"));
            assert!(get_val("real_affinity_col").contains("3.14159"));

            // Test text affinity types
            assert_eq!(
                get_val("text_col"),
                "This is a test text with unicode: ñiño 你好 🚀"
            );
            assert_eq!(get_val("varchar_col"), "variable_string");
            assert_eq!(get_val("text_affinity_col"), "text affinity");

            // Test CHAR type (may be padded)
            assert!(get_val("char_col").starts_with("fixed_len"));

            // Test boolean (stored as integer in SQLite)
            assert!(
                get_val("bool_col") == "1" || get_val("bool_col") == "true",
                "Boolean should be stored as 1 or true in SQLite"
            );

            // Test date/time types
            assert!(get_val("date_col").contains("2025"));
            assert!(get_val("time_col").contains("20:41"));
            assert!(get_val("datetime_col").contains("2025"));
            assert!(get_val("timestamp_col").contains("2025"));

            // Test JSON types
            assert!(get_val("json_col").contains("\"name\""));
            assert!(get_val("json_col").contains("test"));
            assert!(get_val("jsonb_col").contains("nested"));

            // Test blob affinity types (SQLite converts binary data to text when possible)
            assert!(get_val("blob_col").contains("48656c6c6f")); // "Hello World"
            assert!(get_val("blob_affinity_col").contains("48656c6c6f")); // "Hello"

            // Test custom types (should fall back to string conversion)
            assert_eq!(get_val("custom_type_col"), "custom value");

            Ok(())
        }
        .await
    }

    #[tokio::test]
    async fn test_table_ddl_includes_indexes() -> Result<(), Box<dyn std::error::Error>> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite:{}", path);

        let mut connection = crate::SqliteConnection::new(connection_string.clone())?;
        connection.connect(&connection_string).await?;

        connection
            .execute_query(
                "CREATE TABLE widget (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
                None,
                None,
            )
            .await?;
        connection
            .execute_query(
                "CREATE UNIQUE INDEX idx_widget_name ON widget(name)",
                None,
                None,
            )
            .await?;

        let ddl = connection.table_ddl(None, "widget").await?;

        assert!(
            ddl.contains("CREATE TABLE widget"),
            "DDL should contain the table definition, got: {ddl}"
        );
        assert!(
            ddl.contains("idx_widget_name"),
            "DDL should include the index definition, got: {ddl}"
        );
        // The table definition must come before its index.
        let table_pos = ddl.find("CREATE TABLE").unwrap();
        let index_pos = ddl.find("idx_widget_name").unwrap();
        assert!(table_pos < index_pos, "table should precede index: {ddl}");

        Ok(())
    }

    /// A failing statement in a batch must roll the whole batch back, and the
    /// failure must report `applied == 0` so the results-panel commit path keeps
    /// its edits for a safe retry instead of double-applying the operations that
    /// already ran. This is the connection-level guarantee behind the P0 table
    /// cell-edit commit fix.
    #[tokio::test]
    async fn test_execute_operations_transactional_rolls_back_on_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite:{}", path);

        let mut connection = crate::SqliteConnection::new(connection_string.clone())?;
        connection.connect(&connection_string).await?;

        connection
            .execute_query(
                "CREATE TABLE account (id INTEGER PRIMARY KEY, balance INTEGER NOT NULL CHECK (balance >= 0))",
                None,
                None,
            )
            .await?;
        connection
            .execute_query(
                "INSERT INTO account (id, balance) VALUES (1, 100)",
                None,
                None,
            )
            .await?;

        // First op is valid; second violates the CHECK constraint. The whole
        // batch must roll back, leaving the first op's write undone.
        let operations: Vec<blanco_core::WriteOperation> = vec![
            "UPDATE account SET balance = 50 WHERE id = 1".into(),
            "UPDATE account SET balance = -10 WHERE id = 1".into(),
        ];
        let failure = connection
            .execute_operations_transactional(&operations, None)
            .await
            .expect_err("second UPDATE should fail the batch");
        assert_eq!(
            failure.applied, 0,
            "atomic backend must report nothing applied so retry is safe"
        );

        let result = connection
            .execute_query("SELECT balance FROM account WHERE id = 1", None, None)
            .await?;
        let balance = result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|cell| cell.as_deref())
            .expect("balance cell");
        assert_eq!(
            balance, "100",
            "the valid first UPDATE must have rolled back"
        );

        // A fully valid batch commits and reports the operations applied.
        let good: Vec<blanco_core::WriteOperation> = vec![
            "UPDATE account SET balance = 70 WHERE id = 1".into(),
            "INSERT INTO account (id, balance) VALUES (2, 5)".into(),
        ];
        let outcome = connection
            .execute_operations_transactional(&good, None)
            .await
            .expect("valid batch should commit");
        assert_eq!(outcome.operations_executed, 2);

        let count = connection
            .execute_query("SELECT COUNT(*) FROM account", None, None)
            .await?;
        let rows = count
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|cell| cell.as_deref())
            .expect("count cell");
        assert_eq!(rows, "2", "the committed batch should have inserted a row");

        Ok(())
    }

    /// A multi-statement script must yield one result-set per row-returning
    /// statement rather than collapsing into a single merged result.
    #[tokio::test]
    async fn test_execute_script_splits_statements() -> Result<(), Box<dyn std::error::Error>> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite:{}", path);

        let mut connection = crate::SqliteConnection::new(connection_string.clone())?;
        connection.connect(&connection_string).await?;

        let results = connection
            .execute_script("SELECT 1 AS a; SELECT 2 AS b, 3 AS c;", None)
            .await?;

        assert_eq!(results.len(), 2, "each SELECT should be its own result-set");
        assert_eq!(results[0].columns, vec!["a".to_string()]);
        assert_eq!(results[0].rows.len(), 1);
        assert_eq!(results[1].columns, vec!["b".to_string(), "c".to_string()]);

        Ok(())
    }

    #[tokio::test]
    async fn foreign_key_lookup_quotes_identifiers_and_escapes_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite:{}", path);

        let mut connection = crate::SqliteConnection::new(connection_string.clone())?;
        connection.connect(&connection_string).await?;
        connection
            .execute_query(
                "CREATE TABLE \"order\" (\"select\" TEXT PRIMARY KEY, \"value\" TEXT)",
                None,
                None,
            )
            .await?;
        connection
            .execute_query(
                "INSERT INTO \"order\" (\"select\", \"value\") VALUES ('O''Brien', 'found')",
                None,
                None,
            )
            .await?;

        let result = connection
            .foreign_key_lookup("order", "select", "O'Brien")
            .await?;

        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result
                .rows
                .first()
                .and_then(|row| row.get(1))
                .and_then(Option::as_deref),
            Some("found")
        );
        Ok(())
    }
}
