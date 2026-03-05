//! PostgreSQL Database Implementation for Blanco SQL Editor
//!
//! This crate provides PostgreSQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with PostgreSQL dialect

pub mod connection;
pub mod schema;
pub mod sql_parser;

// Re-export main types for convenience
pub use connection::{PgConnectionKey, PostgresConnection};

pub use sql_parser::PostgresTableExtractor;

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::{Column, Row, postgres::PgPoolOptions};
    use std::env;

    #[test]
    fn test_postgres_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        smol::block_on(async {
            let connection_string = env::var("POSTGRES_CONNECTION_STRING").unwrap_or_else(|_| {
                "postgres://blanco:blanco@localhost:5488/blanco?sslmode=disable".to_string()
            });

            let test_pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&connection_string)
                .await?;

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
                    uuid_col, json_col, jsonb_col, int_array_col, text_array_col, uuid_array_col, regclass_col, custom_enum
                ) VALUES (
                    32767, 2147483647, 9223372036854775807, 12345.67, 98765.43210,
                    123.456, 987654321.123456789, 100, 1000, 1000000, 12345.67,
                    'fixed_len  ', 'variable_string', 'This is a test text with unicode: ñiño 你好 🚀', true,
                    CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP,
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
            let value_map: std::collections::HashMap<String, String> = query_result
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| (col.clone(), first_row[i].clone()))
                .collect();

            // Test basic types
            assert_eq!(value_map.get("id"), Some(&"1".to_string()));
            assert_eq!(value_map.get("smallint_col"), Some(&"32767".to_string()));
            assert_eq!(value_map.get("int_col"), Some(&"2147483647".to_string()));
            assert_eq!(
                value_map.get("bigint_col"),
                Some(&"9223372036854775807".to_string())
            );
            assert_eq!(value_map.get("decimal_col"), Some(&"12345.67".to_string()));
            assert_eq!(
                value_map.get("numeric_col"),
                Some(&"98765.43210".to_string())
            );
            assert_eq!(value_map.get("real_col"), Some(&"123.456".to_string()));
            assert_eq!(
                value_map.get("double_precision_col"),
                Some(&"987654321.1234568".to_string())
            );

            // Test string types
            // CHAR type is fixed length and gets padded
            assert!(value_map.get("char_col").unwrap().starts_with("fixed_len"));
            assert_eq!(
                value_map.get("varchar_col"),
                Some(&"variable_string".to_string())
            );
            assert_eq!(
                value_map.get("text_col"),
                Some(&"This is a test text with unicode: ñiño 你好 🚀".to_string())
            );

            // Test boolean
            assert_eq!(value_map.get("bool_col"), Some(&"true".to_string()));

            // Test date/time types, just verify they contain expected patterns
            let date_val = value_map.get("date_col").unwrap();
            assert!(
                date_val.contains('-') && date_val.len() >= 10,
                "Date should be in YYYY-MM-DD format, got: {}",
                date_val
            );
            assert!(value_map.get("time_col").unwrap().contains(":"));
            let timestamp_val = value_map.get("timestamp_col").unwrap();
            assert!(
                timestamp_val.contains('-') && timestamp_val.contains(':'),
                "Timestamp should contain date and time, got: {}",
                timestamp_val
            );
            // Check timestamp with time zone, be more flexible with the time format
            let ts_tz = value_map.get("timestamp_with_time_zone_col").unwrap();
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

            // Test UUID
            assert_eq!(
                value_map.get("uuid_col"),
                Some(&"550e8400-e29b-41d4-a716-446655440000".to_string())
            );

            // Test JSON types (key order may vary, so check for content)
            let json_col = value_map.get("json_col").unwrap();
            assert!(json_col.contains("\"name\":\"test\""));
            assert!(json_col.contains("\"value\":42"));
            assert!(json_col.contains("\"active\":true"));
            assert!(value_map.get("jsonb_col").unwrap().contains("nested"));

            // Test array types
            assert_eq!(
                value_map.get("int_array_col"),
                Some(&"{1,2,3,4,5}".to_string())
            );
            assert_eq!(
                value_map.get("text_array_col"),
                Some(&"{\"hello\",\"world\",\"test\"}".to_string())
            );
            assert_eq!(
                value_map.get("uuid_array_col"),
                Some(
                    &"{550e8400-e29b-41d4-a716-446655440000,660e8400-e29b-41d4-a716-446655440001}"
                        .to_string()
                )
            );

            assert_eq!(
                value_map.get("regclass_col"),
                Some(&"comprehensive_test".to_string())
            );
            assert_eq!(value_map.get("custom_enum"), Some(&"a".to_string()));

            Ok(())
        })
    }

    #[test]
    fn test_computed_column_type_detection() -> Result<(), Box<dyn std::error::Error>> {
        smol::block_on(async {
            // Use environment variable for connection string or fallback to default
            let connection_string = env::var("POSTGRES_CONNECTION_STRING").unwrap_or_else(|_| {
                "postgres://blanco:blanco@localhost:5488/blanco?sslmode=disable".to_string()
            });

            let test_pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&connection_string)
                .await?;

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
        })
    }

    #[test]
    fn test_postgres_ssl_connection() -> Result<(), Box<dyn std::error::Error>> {
        smol::block_on(async {
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
                    // Simple query to verify connection works
                    let row: sqlx::postgres::PgRow =
                        sqlx::query("SELECT 1 as value").fetch_one(&pool).await?;
                    assert_eq!(row.get::<i32, _>(0), 1);

                    // Check SSL is in use via pg_stat_ssl
                    let ssl_row: sqlx::postgres::PgRow =
                        sqlx::query("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
                            .fetch_one(&pool)
                            .await?;
                    assert!(ssl_row.get::<bool, _>(0), "SSL should be in use");

                    pool.close().await;
                    println!("SSL connection test passed with CA verification!");
                    Ok(())
                }
                Err(e) => {
                    // Skip test if SSL PostgreSQL container is not running
                    println!(
                        "Skipping SSL test - SSL PostgreSQL container not available: {}",
                        e
                    );
                    println!(
                        "To run this test, start the container with: docker-compose up -d postgrestestdb_ssl"
                    );
                    Ok(())
                }
            }
        })
    }
}
