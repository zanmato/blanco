//! MySQL/MariaDB Database Implementation for Blanco SQL Editor
//!
//! This crate provides MySQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with MySQL dialect

pub mod connection;
pub mod schema;

// Re-export main types for convenience
pub use connection::{MysqlConnection, MysqlConnectionKey};

#[cfg(test)]
#[allow(clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use blanco_core::Connection;
    use std::env;

    fn default_connection_string() -> String {
        env::var("MYSQL_CONNECTION_STRING")
            .unwrap_or_else(|_| "mysql://root:blanco@127.0.0.1:3306/blanco".to_string())
    }

    /// Decide whether to fail or skip a test when the MySQL server is not
    /// reachable. When `BLANCO_RUN_DB_TESTS=1` is set, an unreachable server
    /// is a hard failure; otherwise the test prints a skip message and
    /// returns Ok(()).
    fn handle_unreachable(
        test_name: &str,
        err: &dyn std::fmt::Display,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1") {
            Err(format!("{test_name}: MySQL unreachable: {err}").into())
        } else {
            eprintln!(
                "skip {test_name}: MySQL unreachable ({err}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_mysql_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let connection_string = default_connection_string();
        let mut conn = crate::MysqlConnection::from_connection_string(&connection_string)?;
        if let Err(e) = conn.connect(&connection_string).await {
            return handle_unreachable("test_mysql_data_type_serialization", &e);
        }
        let database = Some("blanco");

        conn.execute_query("DROP TABLE IF EXISTS comprehensive_test", database, None)
            .await?;

        let create_table_sql = r#"
            CREATE TABLE comprehensive_test (
                id INT AUTO_INCREMENT PRIMARY KEY,

                tinyint_col TINYINT,
                smallint_col SMALLINT,
                mediumint_col MEDIUMINT,
                int_col INT,
                bigint_col BIGINT,

                tinyint_u_col TINYINT UNSIGNED,
                int_u_col INT UNSIGNED,
                bigint_u_col BIGINT UNSIGNED,

                decimal_col DECIMAL(10, 2),
                numeric_col NUMERIC(15, 5),
                float_col FLOAT,
                double_col DOUBLE,

                char_col CHAR(10),
                varchar_col VARCHAR(255),
                text_col TEXT,
                tinytext_col TINYTEXT,
                mediumtext_col MEDIUMTEXT,
                longtext_col LONGTEXT,

                bool_col BOOLEAN,

                binary_col BINARY(5),
                varbinary_col VARBINARY(16),
                blob_col BLOB,

                date_col DATE,
                time_col TIME,
                datetime_col DATETIME,
                timestamp_col TIMESTAMP NULL,
                year_col YEAR,

                json_col JSON,

                enum_col ENUM('a', 'b', 'c'),
                set_col SET('x', 'y', 'z'),

                null_col INT
            )
        "#;
        conn.execute_query(create_table_sql, database, None).await?;

        let insert_sql = r#"
            INSERT INTO comprehensive_test (
                tinyint_col, smallint_col, mediumint_col, int_col, bigint_col,
                tinyint_u_col, int_u_col, bigint_u_col,
                decimal_col, numeric_col, float_col, double_col,
                char_col, varchar_col, text_col, tinytext_col, mediumtext_col, longtext_col,
                bool_col,
                binary_col, varbinary_col, blob_col,
                date_col, time_col, datetime_col, timestamp_col, year_col,
                json_col,
                enum_col, set_col,
                null_col
            ) VALUES (
                127, 32767, 8388607, 2147483647, 9223372036854775807,
                255, 4294967295, 18446744073709551615,
                12345.67, 98765.43210, 123.5, 987654321.123456,
                'fixed_len ', 'variable_string', 'This is a test text with unicode: ñiño 你好 🚀',
                'tiny', 'medium', 'long',
                TRUE,
                X'48656C6C6F', X'48656C6C6F20576F726C64', X'48656C6C6F20576F726C64',
                '2025-10-30', '20:41:00', '2025-10-30 20:41:00', '2025-10-30 20:41:00', 2025,
                '{"name": "test", "value": 42, "active": true}',
                'b', 'x,z',
                NULL
            )
        "#;
        conn.execute_query(insert_sql, database, None).await?;

        let query_result = conn
            .execute_query(
                "SELECT * FROM comprehensive_test ORDER BY id DESC LIMIT 1",
                database,
                None,
            )
            .await?;
        assert!(!query_result.rows.is_empty());
        let first_row = query_result.rows.first().unwrap();
        let value_map: std::collections::HashMap<String, Option<String>> = query_result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| (col.clone(), first_row[i].clone()))
            .collect();

        let get_val =
            |key: &str| -> &str { value_map.get(key).and_then(|v| v.as_deref()).unwrap_or("") };

        assert_eq!(get_val("tinyint_col"), "127");
        assert_eq!(get_val("smallint_col"), "32767");
        assert_eq!(get_val("mediumint_col"), "8388607");
        assert_eq!(get_val("int_col"), "2147483647");
        assert_eq!(get_val("bigint_col"), "9223372036854775807");

        assert_eq!(get_val("tinyint_u_col"), "255");
        assert_eq!(get_val("int_u_col"), "4294967295");
        assert_eq!(get_val("bigint_u_col"), "18446744073709551615");

        assert_eq!(get_val("decimal_col"), "12345.67");
        assert_eq!(get_val("numeric_col"), "98765.43210");
        assert!(get_val("float_col").starts_with("123."));
        assert!(get_val("double_col").starts_with("987654321."));

        assert!(get_val("char_col").starts_with("fixed_len"));
        assert_eq!(get_val("varchar_col"), "variable_string");
        assert_eq!(
            get_val("text_col"),
            "This is a test text with unicode: ñiño 你好 🚀"
        );
        assert_eq!(get_val("tinytext_col"), "tiny");
        assert_eq!(get_val("mediumtext_col"), "medium");
        assert_eq!(get_val("longtext_col"), "long");

        let bool_val = get_val("bool_col");
        assert!(bool_val == "1" || bool_val == "true");

        assert!(
            get_val("binary_col")
                .to_lowercase()
                .starts_with("0x48656c6c6f")
        );
        assert!(
            get_val("varbinary_col")
                .to_lowercase()
                .contains("48656c6c6f20576f726c64")
        );
        assert!(
            get_val("blob_col")
                .to_lowercase()
                .contains("48656c6c6f20576f726c64")
        );

        assert!(
            get_val("date_col").contains("2025"),
            "date_col: {:?}",
            get_val("date_col")
        );
        assert!(
            get_val("time_col").contains("20:41"),
            "time_col: {:?}",
            get_val("time_col")
        );
        assert!(
            get_val("datetime_col").contains("2025"),
            "datetime_col: {:?}",
            get_val("datetime_col")
        );
        assert!(
            get_val("timestamp_col").contains("2025"),
            "timestamp_col: {:?}",
            value_map.get("timestamp_col")
        );
        assert_eq!(get_val("year_col"), "2025");

        // MariaDB reports JSON columns to the driver as BLOB at the wire-protocol
        // level, so JSON currently round-trips as the hex-encoded UTF-8 bytes
        // rather than the JSON text. Both forms still contain the encoded name.
        let json_val = get_val("json_col");
        assert!(
            json_val.contains("\"name\"")
                || json_val.to_lowercase().contains(&hex::encode(b"\"name\"")),
            "json_col: {:?}",
            value_map.get("json_col")
        );

        assert_eq!(get_val("enum_col"), "b");
        assert_eq!(get_val("set_col"), "x,z");

        assert!(
            value_map
                .get("null_col")
                .map(|v| v.is_none())
                .unwrap_or(false),
            "null_col should be SQL NULL, got {:?}",
            value_map.get("null_col")
        );

        conn.execute_query("DROP TABLE IF EXISTS comprehensive_test", database, None)
            .await
            .ok();

        Ok(())
    }

    #[tokio::test]
    async fn test_mysql_basic_connection() -> Result<(), Box<dyn std::error::Error>> {
        let connection_string = default_connection_string();
        let mysql_connection = crate::MysqlConnection::from_connection_string(&connection_string)?;

        match mysql_connection
            .execute_query("SELECT 1 as test_value, 'hello' as test_text", None, None)
            .await
        {
            Ok(result) => {
                assert!(
                    !result.rows.is_empty(),
                    "Simple query should return results"
                );
                assert_eq!(result.columns.len(), 2);
                assert_eq!(result.columns[0], "test_value");
                assert_eq!(result.columns[1], "test_text");
            }
            Err(e) => return handle_unreachable("test_mysql_basic_connection", &e),
        }

        assert_eq!(mysql_connection.get_connection_type(), "MySQL");
        assert!(mysql_connection.get_display_name().contains("MySQL"));

        let from_table = mysql_connection
            .extract_table_name_from_query("SELECT * FROM users WHERE id = 1", false)?;
        assert_eq!(from_table, Some("users".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_mysql_indexes() -> Result<(), Box<dyn std::error::Error>> {
        async {
            let connection_string = default_connection_string();

            let mysql_connection =
                crate::MysqlConnection::from_connection_string(&connection_string)?;

            if let Err(e) = mysql_connection
                .execute_query("SELECT 1 as test_value", None, None)
                .await
            {
                return handle_unreachable("test_mysql_indexes", &e);
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

            // Verify we have at least the PRIMARY key and the two indexes we created
            assert!(
                indexes.len() >= 2,
                "Expected at least 2 indexes, got {}",
                indexes.len()
            );

            // Check for PRIMARY key
            let primary = indexes.iter().find(|i| i.name == "PRIMARY");
            assert!(primary.is_some(), "PRIMARY key index not found");
            let primary = primary.unwrap();
            assert!(primary.is_unique, "PRIMARY key should be unique");
            assert!(
                primary.column_names.contains(&"id".to_string()),
                "PRIMARY key should contain 'id' column"
            );

            // Check for idx_email
            let email_idx = indexes.iter().find(|i| i.name == "idx_email");
            assert!(email_idx.is_some(), "idx_email index not found");
            let email_idx = email_idx.unwrap();
            assert!(!email_idx.is_unique, "idx_email should not be unique");
            assert_eq!(
                email_idx.column_names,
                vec!["email"],
                "idx_email should have email column"
            );

            // Check for idx_name_email (composite index)
            let name_email_idx = indexes.iter().find(|i| i.name == "idx_name_email");
            assert!(name_email_idx.is_some(), "idx_name_email index not found");
            let name_email_idx = name_email_idx.unwrap();
            assert!(
                !name_email_idx.is_unique,
                "idx_name_email should not be unique"
            );
            assert_eq!(
                name_email_idx.column_names,
                vec!["name", "email"],
                "idx_name_email should have name and email columns"
            );

            // Clean up
            let _ = mysql_connection
                .execute_query("DROP TABLE IF EXISTS test_indexes", None, None)
                .await;

            Ok(())
        }
        .await
    }
}
