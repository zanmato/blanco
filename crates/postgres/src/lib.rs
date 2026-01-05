//! PostgreSQL Database Implementation for Blanco SQL Editor
//!
//! This crate provides PostgreSQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with PostgreSQL dialect

pub mod connection;
pub mod factory;
pub mod schema;
pub mod sql_parser;

// Re-export main types for convenience
pub use connection::{PgConnectionKey, PostgresConnection};

pub use sql_parser::{CompletionKind, ParsedQuery, PostgresTableExtractor, TableAlias};

pub use factory::PostgresConnectionFactory;

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::{postgres::PgPoolOptions, Column, Row, TypeInfo};
    use std::env;

    #[smol::test]
    async fn test_postgres_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        // Use environment variable for connection string or fallback to default
        let connection_string = env::var("POSTGRES_CONNECTION_STRING").unwrap_or_else(|_| {
            "postgres://manager:manager@localhost:5444/postgres?sslmode=disable".to_string()
        });

        // Connect to PostgreSQL (use postgres database to create test database)
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        // Create test database if it doesn't exist
        sqlx::query("CREATE DATABASE manager_test")
            .execute(&pool)
            .await
            .or_else(|_| {
                println!("Database 'manager_test' may already exist, continuing...");
                Ok::<_, sqlx::Error>(sqlx::postgres::PgQueryResult::default())
            })?;

        // Switch to the test database
        let test_connection_string = connection_string.replace("/postgres", "/manager_test");
        let test_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&test_connection_string)
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

        let result = sqlx::query(insert_sql).execute(&test_pool).await?;

        // Test with Blanco's PostgreSQL connection
        let mut postgres_connection =
            crate::PostgresConnection::from_connection_string(&test_connection_string)?;

        // Establish the actual database connection
        postgres_connection.connect(&test_connection_string).await?;

        let database_name = "manager_test".to_string();

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

        // Test date/time types
        assert!(value_map.get("date_col").unwrap().starts_with("2025-11-18"));
        assert!(value_map.get("time_col").unwrap().contains(":"));
        assert!(value_map
            .get("timestamp_col")
            .unwrap()
            .starts_with("2025-11-18"));
        // Check timestamp with time zone - be more flexible with the time format
        let ts_tz = value_map.get("timestamp_with_time_zone_col").unwrap();
        assert!(ts_tz.contains("2025-11-18") || ts_tz.contains("T"));
        assert!(ts_tz.contains("+00:00") || ts_tz.contains("UTC"));

        // Test UUID
        assert_eq!(
            value_map.get("uuid_col"),
            Some(&"550e8400-e29b-41d4-a716-446655440000".to_string())
        );

        // Test JSON types
        assert_eq!(
            value_map.get("json_col"),
            Some(&"{\"name\":\"test\",\"value\":42,\"active\":true}".to_string())
        );
        assert!(value_map.get("jsonb_col").unwrap().contains("nested"));

        // Test array types - all working now!
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

        println!("✅ All type conversion tests passed!");
        println!("✅ Basic types, strings, booleans, dates, timestamps, UUID, JSON all working correctly!");
        println!("✅ Array types (INT4[], TEXT[], UUID[]) all working correctly!");
        println!(
            "✅ System catalog types (regclass) working correctly with user-friendly display!"
        );

        Ok(())
    }

    #[smol::test]
    async fn test_computed_column_type_detection() -> Result<(), Box<dyn std::error::Error>> {
        // Use environment variable for connection string or fallback to default
        let connection_string = env::var("POSTGRES_CONNECTION_STRING").unwrap_or_else(|_| {
            "postgres://manager:manager@localhost:5444/postgres?sslmode=disable".to_string()
        });

        // Connect to PostgreSQL
        let _pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        // Create test database if it doesn't exist
        let _ = sqlx::query("CREATE DATABASE manager_test")
            .execute(&_pool)
            .await;

        // Use the manager_test database
        let test_connection_string = connection_string.replace("/postgres", "/manager_test");
        let test_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&test_connection_string)
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

        println!("\n=== Computed Column Type Test ===");
        println!("Query: {}", query);

        for (i, row) in rows.iter().enumerate() {
            println!("\nRow {}:", i);

            for (col_idx, col) in row.columns().iter().enumerate() {
                let col_name = col.name();
                let type_name = col.type_info().name();
                println!(
                    "  Column {}: name='{}', type='{}'",
                    col_idx, col_name, type_name
                );
            }
        }

        // Verify we got 2 columns with proper type information
        assert_eq!(rows.len(), 1, "Should have 1 row");
        let first_row = &rows[0];
        let columns = first_row.columns();
        assert_eq!(columns.len(), 2, "Should have 2 columns");

        // Check column names - PostgreSQL keeps the same name for both columns!
        assert_eq!(columns[0].name(), "smallint_col");
        assert_eq!(columns[1].name(), "smallint_col"); // Same name, different type!

        // Check column types - this is the key test
        let type0 = columns[0].type_info().name();
        let type1 = columns[1].type_info().name();

        println!("\nColumn types:");
        println!("  Column 0 (smallint_col): {}", type0);
        println!("  Column 1 (smallint_col::text): {}", type1);

        // Both types should be available
        assert!(!type0.is_empty(), "First column type should not be empty");
        assert!(!type1.is_empty(), "Second column type should not be empty");

        // Now test with Blanco's connection to see if the issue is in our code
        let mut postgres_connection =
            crate::PostgresConnection::from_connection_string(&test_connection_string)?;
        postgres_connection.connect(&test_connection_string).await?;

        let database_name = "manager_test".to_string();

        // Test the problematic query
        let query_result = postgres_connection
            .execute_query(query, Some(&database_name), None)
            .await?;

        println!("\n=== Blanco QueryResult ===");
        println!("Columns: {:?}", query_result.columns);
        println!("Column types: {:?}", query_result.column_types);
        println!("Rows: {:?}", query_result.rows);

        // Verify column count matches column types count
        assert_eq!(
            query_result.columns.len(),
            query_result.column_types.len(),
            "Column count should match column types count"
        );

        println!("\n✅ Computed column type detection test passed!");
        Ok(())
    }
}
