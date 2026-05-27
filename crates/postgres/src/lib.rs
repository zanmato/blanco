//! PostgreSQL Database Implementation for Blanco SQL Editor
//!
//! This crate provides PostgreSQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with PostgreSQL dialect

pub mod connection;
pub mod schema;

// Re-export main types for convenience
pub use connection::{PgConnectionKey, PostgresConnection};

#[cfg(test)]
#[allow(clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use blanco_core::Connection;
    use sqlx::{Column, Row, postgres::PgPoolOptions};
    use std::env;

    fn default_connection_string() -> String {
        env::var("POSTGRES_CONNECTION_STRING").unwrap_or_else(|_| {
            "postgres://blanco:blanco@localhost:5488/blanco?sslmode=disable".to_string()
        })
    }

    /// Decide whether to fail or skip a test when the PostgreSQL server is
    /// not reachable. `BLANCO_RUN_DB_TESTS=1` turns missing servers into a
    /// hard failure; otherwise tests print a skip message and return Ok.
    fn handle_unreachable(
        test_name: &str,
        err: &dyn std::fmt::Display,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1") {
            Err(format!("{test_name}: PostgreSQL unreachable: {err}").into())
        } else {
            eprintln!(
                "skip {test_name}: PostgreSQL unreachable ({err}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_postgres_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        async {
            let connection_string = default_connection_string();

            let test_pool = match PgPoolOptions::new()
                .max_connections(5)
                .connect(&connection_string)
                .await
            {
                Ok(pool) => pool,
                Err(e) => {
                    return handle_unreachable("test_postgres_data_type_serialization", &e);
                }
            };

            sqlx::query("DROP TYPE IF EXISTS custom_enum CASCADE")
                .execute(&test_pool)
                .await
                .ok();

            let create_enum_sql = r#"
                CREATE TYPE custom_enum AS ENUM (
                    'a',
                    'b'
                );
            "#;

            sqlx::query(create_enum_sql).execute(&test_pool).await?;

            // Create simple test table with basic data types
            let create_table_sql = r#"
                CREATE TABLE IF NOT EXISTS comprehensive_test (
                    id SERIAL PRIMARY KEY,

                    -- Numeric types
                    smallint_col SMALLINT,
                    int_col INTEGER,
                    bigint_col BIGINT,
                    decimal_col DECIMAL(10, 2),
                    numeric_col NUMERIC(15, 5),
                    real_col REAL,
                    double_precision_col DOUBLE PRECISION,
                    smallserial_col SMALLSERIAL,
                    serial_col SERIAL,
                    bigserial_col BIGSERIAL,
                    money_col MONEY,

                    -- Character types
                    char_col CHAR(10),
                    varchar_col VARCHAR(255),
                    text_col TEXT,

                    -- Boolean type
                    bool_col BOOLEAN,

                    -- Date/Time types
                    date_col DATE,
                    time_col TIME,
                    timestamp_col TIMESTAMP,
                    timestamp_with_time_zone_col TIMESTAMP WITH TIME ZONE,
                    interval_col INTERVAL,
                    interval_zero_col INTERVAL,
                    interval_neg_col INTERVAL,

                    -- UUID type
                    uuid_col UUID,

                    -- JSON types
                    json_col JSON,
                    jsonb_col JSONB,

                    -- Array types
                    int_array_col INTEGER[],
                    text_array_col TEXT[],
                    uuid_array_col UUID[],

                    -- System catalog types
                    regclass_col regclass,

                    -- Enum types
                    custom_enum custom_enum
                )
            "#;

            // Drop table first to ensure we have the latest schema
            sqlx::query("DROP TABLE IF EXISTS comprehensive_test")
                .execute(&test_pool)
                .await
                .ok();
            sqlx::query(create_table_sql).execute(&test_pool).await?;

            // Insert test data with key data types
            let insert_sql = r#"
                INSERT INTO comprehensive_test (
                    smallint_col, int_col, bigint_col, decimal_col, numeric_col,
                    real_col, double_precision_col, smallserial_col, serial_col, bigserial_col, money_col,
                    char_col, varchar_col, text_col, bool_col,
                    date_col, time_col, timestamp_col, timestamp_with_time_zone_col,
                    interval_col, interval_zero_col, interval_neg_col,
                    uuid_col, json_col, jsonb_col, int_array_col, text_array_col, uuid_array_col, regclass_col, custom_enum
                ) VALUES (
                    32767, 2147483647, 9223372036854775807, 12345.67, 98765.43210,
                    123.456, 987654321.123456789, 100, 1000, 1000000, 12345.67,
                    'fixed_len  ', 'variable_string', 'This is a test text with unicode: ñiño 你好 🚀', true,
                    CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP,
                    INTERVAL '1 year 2 months 3 days 04:05:06.789',
                    INTERVAL '0',
                    INTERVAL '-1 day -02:30:00',
                    '550e8400-e29b-41d4-a716-446655440000',
                    '{"name": "test", "value": 42, "active": true}',
                    '{"nested": {"array": [1,2,3], "text": "hello"}}',
                    ARRAY[1, 2, 3, 4, 5],
                    ARRAY['hello', 'world', 'test'],
                    ARRAY['550e8400-e29b-41d4-a716-446655440000'::uuid, '660e8400-e29b-41d4-a716-446655440001'::uuid],
                    'comprehensive_test'::regclass, 'a'
                )
                ON CONFLICT DO NOTHING;
            "#;

            let _result = sqlx::query(insert_sql).execute(&test_pool).await?;

            // Test with Blanco's PostgreSQL connection
            let mut postgres_connection =
                crate::PostgresConnection::from_connection_string(&connection_string)?;

            // Establish the actual database connection
            postgres_connection.connect(&connection_string).await?;

            let database_name = "blanco".to_string();

            // Test a simple query to make sure Blanco can handle the data
            let query_result = postgres_connection.execute_query("SELECT * FROM comprehensive_test WHERE id = (SELECT MAX(id) FROM comprehensive_test)", Some(&database_name), None).await?;

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

            let get_val =
                |key: &str| -> &str { value_map.get(key).and_then(|v| v.as_deref()).unwrap_or("") };

            // Test basic types
            assert_eq!(get_val("id"), "1");
            assert_eq!(get_val("smallint_col"), "32767");
            assert_eq!(get_val("int_col"), "2147483647");
            assert_eq!(get_val("bigint_col"), "9223372036854775807");
            assert_eq!(get_val("decimal_col"), "12345.67");
            assert_eq!(get_val("numeric_col"), "98765.43210");
            assert_eq!(get_val("real_col"), "123.456");
            assert_eq!(get_val("double_precision_col"), "987654321.1234568");

            // Test string types
            // CHAR type is fixed length and gets padded
            assert!(get_val("char_col").starts_with("fixed_len"));
            assert_eq!(get_val("varchar_col"), "variable_string");
            assert_eq!(
                get_val("text_col"),
                "This is a test text with unicode: ñiño 你好 🚀"
            );

            // Test boolean
            assert_eq!(get_val("bool_col"), "true");

            // Test date/time types, just verify they contain expected patterns
            let date_val = get_val("date_col");
            assert!(
                date_val.contains('-') && date_val.len() >= 10,
                "Date should be in YYYY-MM-DD format, got: {}",
                date_val
            );
            assert!(get_val("time_col").contains(":"));
            let timestamp_val = get_val("timestamp_col");
            assert!(
                timestamp_val.contains('-') && timestamp_val.contains(':'),
                "Timestamp should contain date and time, got: {}",
                timestamp_val
            );
            // Check timestamp with time zone, be more flexible with the time format
            let ts_tz = get_val("timestamp_with_time_zone_col");
            assert!(
                ts_tz.contains('-') || ts_tz.contains("T"),
                "Timestamp with TZ should contain date, got: {}",
                ts_tz
            );
            assert!(
                ts_tz.contains("+00:00")
                    || ts_tz.contains("UTC")
                    || ts_tz.contains("Z")
                    || ts_tz.contains("T")
            );

            // Test interval, must render as text rather than NULL
            assert_eq!(
                get_val("interval_col"),
                "1 year 2 mons 3 days 04:05:06.789"
            );
            assert_eq!(get_val("interval_zero_col"), "00:00:00");
            assert_eq!(get_val("interval_neg_col"), "-1 days -02:30:00");

            // Test UUID
            assert_eq!(get_val("uuid_col"), "550e8400-e29b-41d4-a716-446655440000");

            // Test JSON types (key order may vary, so check for content)
            let json_col = get_val("json_col");
            assert!(json_col.contains("\"name\":\"test\""));
            assert!(json_col.contains("\"value\":42"));
            assert!(json_col.contains("\"active\":true"));
            assert!(get_val("jsonb_col").contains("nested"));

            // Test array types
            assert_eq!(get_val("int_array_col"), "{1,2,3,4,5}");
            assert_eq!(get_val("text_array_col"), "{\"hello\",\"world\",\"test\"}");
            assert_eq!(
                get_val("uuid_array_col"),
                "{550e8400-e29b-41d4-a716-446655440000,660e8400-e29b-41d4-a716-446655440001}"
            );

            assert_eq!(get_val("regclass_col"), "comprehensive_test");
            assert_eq!(get_val("custom_enum"), "a");

            Ok(())
        }
        .await
    }

    #[tokio::test]
    async fn test_computed_column_type_detection() -> Result<(), Box<dyn std::error::Error>> {
        async {
            let connection_string = default_connection_string();

            let test_pool = match PgPoolOptions::new()
                .max_connections(5)
                .connect(&connection_string)
                .await
            {
                Ok(pool) => pool,
                Err(e) => return handle_unreachable("test_computed_column_type_detection", &e),
            };

            // Create test table if it doesn't exist
            sqlx::query("DROP TABLE IF EXISTS computed_test")
                .execute(&test_pool)
                .await
                .ok();

            sqlx::query(
                "CREATE TABLE computed_test (
                id SERIAL PRIMARY KEY,
                smallint_col SMALLINT,
                text_col TEXT
            )",
            )
            .execute(&test_pool)
            .await?;

            // Insert test data
            sqlx::query("INSERT INTO computed_test (smallint_col, text_col) VALUES (42, 'hello')")
                .execute(&test_pool)
                .await?;

            // Test query with computed column (cast)
            let query = "SELECT smallint_col, smallint_col::text FROM computed_test";
            let rows = sqlx::query(query).fetch_all(&test_pool).await?;

            // Verify we got 2 columns with proper type information
            assert_eq!(rows.len(), 1, "Should have 1 row");
            let first_row = &rows[0];
            let columns = first_row.columns();
            assert_eq!(columns.len(), 2, "Should have 2 columns");

            assert_eq!(columns[0].name(), "smallint_col");
            assert_eq!(columns[1].name(), "smallint_col");

            Ok(())
        }
        .await
    }

    #[tokio::test]
    async fn test_postgres_ssl_connection() -> Result<(), Box<dyn std::error::Error>> {
        async {
            // Test SSL connection to the SSL-enabled PostgreSQL container

            // Get the path to the test CA certificate
            let manifest_dir =
                std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
            let ca_cert_path = format!("{}/test_certs/ca.crt", manifest_dir);

            // Test with sslmode=verify-ca using the CA certificate
            let connection_string = format!(
                "postgres://blanco:blanco@localhost:5488/blanco?sslmode=verify-ca&sslrootcert={}",
                ca_cert_path
            );

            let pool_result = PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(&connection_string)
                .await;

            match pool_result {
                Ok(pool) => {
                    let row: sqlx::postgres::PgRow =
                        sqlx::query("SELECT 1 as value").fetch_one(&pool).await?;
                    assert_eq!(row.get::<i32, _>(0), 1);

                    let ssl_row: sqlx::postgres::PgRow =
                        sqlx::query("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
                            .fetch_one(&pool)
                            .await?;
                    assert!(ssl_row.get::<bool, _>(0), "SSL should be in use");

                    pool.close().await;
                    Ok(())
                }
                Err(e) => handle_unreachable("test_postgres_ssl_connection", &e),
            }
        }
        .await
    }
}
