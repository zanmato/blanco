//! PostgreSQL Database Implementation for Blanco SQL Editor
//!
//! This crate provides PostgreSQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with PostgreSQL dialect
//! - Auto-completion for PostgreSQL queries (TODO)
//! - Hover information for tables and columns (TODO)

pub mod completion;
pub mod connection;
pub mod factory;
pub mod hover;
pub mod sql_parser;

// Re-export main types for convenience
pub use connection::{PgConnectionKey, PostgresConnection};

pub use sql_parser::{CompletionKind, ParsedQuery, PostgresTableExtractor, TableAlias};

pub use factory::PostgresConnectionFactory;

// Import completion and hover implementations to make them available
// use completion::*;
// use hover::*;

#[cfg(test)]
mod tests {
    use blanco_core::Connection;
    use sqlx::{postgres::PgPoolOptions, Row, ValueRef};
    use std::env;

    #[async_std::test]
    async fn test_postgres_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        // Use environment variable for connection string or fallback to default
        let connection_string = env::var("POSTGRES_CONNECTION_STRING").unwrap_or_else(|_| {
            "postgres://manager:manager@localhost:5444/postgres?sslmode=disable".to_string()
        });

        println!(
            "Testing PostgreSQL data types with connection: {}",
            connection_string
        );

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

        println!("Successfully connected to PostgreSQL!");

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

                -- Enum types
                custom_enum custom_enum
            )
        "#;

        println!("Creating comprehensive test table...");
        // Drop table first to ensure we have the latest schema
        sqlx::query("DROP TABLE IF EXISTS comprehensive_test")
            .execute(&test_pool)
            .await
            .ok();
        sqlx::query(create_table_sql).execute(&test_pool).await?;
        println!("Table created successfully!");

        // Insert test data with key data types
        let insert_sql = r#"
            INSERT INTO comprehensive_test (
                smallint_col, int_col, bigint_col, decimal_col, numeric_col,
                real_col, double_precision_col, smallserial_col, serial_col, bigserial_col, money_col,
                char_col, varchar_col, text_col, bool_col,
                date_col, time_col, timestamp_col, timestamp_with_time_zone_col,
                uuid_col, json_col, jsonb_col, int_array_col, text_array_col, uuid_array_col, custom_enum
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
                ARRAY['550e8400-e29b-41d4-a716-446655440000'::uuid, '660e8400-e29b-41d4-a716-446655440001'::uuid], 'a'
            )
            ON CONFLICT DO NOTHING;
        "#;

        println!("Inserting test data...");
        let result = sqlx::query(insert_sql).execute(&test_pool).await?;
        println!(
            "Test data inserted successfully! Rows affected: {}",
            result.rows_affected()
        );

        // Read the data back and verify serialization
        println!("\n=== Testing Data Type Serialization ===\n");

        let select_sql = r#"
            SELECT
                id, smallint_col, int_col, bigint_col, decimal_col, numeric_col,
                real_col, double_precision_col, smallserial_col, serial_col, bigserial_col, money_col,
                char_col, varchar_col, text_col, bool_col,
                date_col, time_col, timestamp_col, timestamp_with_time_zone_col,
                uuid_col, json_col, jsonb_col, int_array_col, text_array_col, uuid_array_col,
                custom_enum::text
            FROM comprehensive_test
            WHERE id = (SELECT MAX(id) FROM comprehensive_test)
        "#;

        let row = sqlx::query(select_sql).fetch_one(&test_pool).await?;

        // Helper function to safely extract and print values
        fn safe_print<
            T: std::fmt::Display
                + sqlx::Type<sqlx::Postgres>
                + for<'r> sqlx::Decode<'r, sqlx::Postgres>,
        >(
            name: &str,
            row: &sqlx::postgres::PgRow,
            column: &str,
        ) -> Result<(), sqlx::Error> {
            match row.try_get::<Option<T>, _>(column) {
                Ok(Some(value)) => {
                    println!("{:<25}: {} ({})", name, value, std::any::type_name::<T>())
                }
                Ok(None) => println!("{:<25}: NULL", name),
                Err(e) => println!(
                    "{:<25}: ERROR - {} ({})",
                    name,
                    e,
                    std::any::type_name::<T>()
                ),
            }
            Ok(())
        }

        // Helper function for array types that don't implement Display
        fn safe_print_array<
            T: std::fmt::Debug
                + sqlx::Type<sqlx::Postgres>
                + for<'r> sqlx::Decode<'r, sqlx::Postgres>
                + sqlx::postgres::PgHasArrayType,
        >(
            name: &str,
            row: &sqlx::postgres::PgRow,
            column: &str,
        ) -> Result<(), sqlx::Error> {
            match row.try_get::<Option<Vec<T>>, _>(column) {
                Ok(Some(arr)) => println!(
                    "{:<25}: {:?} ({})",
                    name,
                    arr,
                    std::any::type_name::<Vec<T>>()
                ),
                Ok(None) => println!("{:<25}: NULL", name),
                Err(e) => println!(
                    "{:<25}: ERROR - {} ({})",
                    name,
                    e,
                    std::any::type_name::<Vec<T>>()
                ),
            }
            Ok(())
        }

        // Test key data types
        println!("--- Numeric Types ---");
        safe_print::<i16>("smallint_col", &row, "smallint_col")?;
        safe_print::<i32>("int_col", &row, "int_col")?;
        safe_print::<i64>("bigint_col", &row, "bigint_col")?;
        // Now that we have rust_decimal feature enabled, test decimal types
        safe_print::<rust_decimal::Decimal>("decimal_col", &row, "decimal_col")?;
        safe_print::<rust_decimal::Decimal>("numeric_col", &row, "numeric_col")?;
        // Money type needs special handling - convert via string
        match row.try_get::<Option<rust_decimal::Decimal>, _>("money_col") {
            Ok(Some(value)) => println!(
                "{:<25}: ${} ({})",
                "money_col",
                value,
                std::any::type_name::<rust_decimal::Decimal>()
            ),
            Ok(None) => println!("{:<25}: NULL", "money_col"),
            Err(_) => {
                // Fallback: try as string and parse
                match row.try_get::<Option<String>, _>("money_col") {
                    Ok(Some(value)) => println!(
                        "{:<25}: {} ({})",
                        "money_col",
                        value,
                        std::any::type_name::<String>()
                    ),
                    Ok(None) => println!("{:<25}: NULL", "money_col"),
                    Err(e) => println!(
                        "{:<25}: ERROR - {} ({})",
                        "money_col",
                        e,
                        std::any::type_name::<String>()
                    ),
                }
            }
        }
        safe_print::<f32>("real_col", &row, "real_col")?;
        safe_print::<f64>("double_precision_col", &row, "double_precision_col")?;
        safe_print::<i16>("smallserial_col", &row, "smallserial_col")?;
        safe_print::<i32>("serial_col", &row, "serial_col")?;
        safe_print::<i64>("bigserial_col", &row, "bigserial_col")?;

        println!("\n--- Character Types ---");
        safe_print::<String>("char_col", &row, "char_col")?;
        safe_print::<String>("varchar_col", &row, "varchar_col")?;
        safe_print::<String>("text_col", &row, "text_col")?;

        println!("\n--- Boolean Type ---");
        safe_print::<bool>("bool_col", &row, "bool_col")?;

        println!("\n--- Date/Time Types ---");
        safe_print::<chrono::NaiveDate>("date_col", &row, "date_col")?;
        safe_print::<chrono::NaiveTime>("time_col", &row, "time_col")?;
        safe_print::<chrono::NaiveDateTime>("timestamp_col", &row, "timestamp_col")?;
        safe_print::<chrono::DateTime<chrono::Utc>>(
            "timestamp_with_time_zone_col",
            &row,
            "timestamp_with_time_zone_col",
        )?;

        println!("\n--- UUID Type ---");
        safe_print::<uuid::Uuid>("uuid_col", &row, "uuid_col")?;

        println!("\n--- JSON Types ---");
        safe_print::<serde_json::Value>("json_col", &row, "json_col")?;
        safe_print::<serde_json::Value>("jsonb_col", &row, "jsonb_col")?;

        println!("\n--- Array Types ---");
        safe_print_array::<i32>("int_array_col", &row, "int_array_col")?;
        safe_print_array::<String>("text_array_col", &row, "text_array_col")?;
        safe_print_array::<uuid::Uuid>("uuid_array_col", &row, "uuid_array_col")?;
        // Skip numeric array for now - requires Decimal type support
        // safe_print_array::<rust_decimal::Decimal>("numeric_array_col", &row, "numeric_array_col")?;

        println!("\n--- Enum Types ---");
        // Try to get the enum value using different approaches
        let mut enum_handled = false;

        // Try standard String conversion
        if let Ok(val) = row.try_get::<Option<String>, _>("custom_enum") {
            match val {
                Some(value) => {
                    println!("{:<25}: {} ({})", "custom_enum", value, "String");
                    enum_handled = true;
                }
                None => {
                    println!("{:<25}: NULL", "custom_enum");
                    enum_handled = true;
                }
            }
        }

        // If String conversion failed, try using raw value access
        if !enum_handled {
            match row.try_get_raw("custom_enum") {
                Ok(raw_value) => {
                    if raw_value.is_null() {
                        println!("{:<25}: NULL", "custom_enum");
                    } else {
                        // Try to get the raw bytes and convert to string
                        match raw_value.as_str() {
                            Ok(text_val) => {
                                println!("{:<25}: {} (raw text)", "custom_enum", text_val);
                            }
                            Err(_) => {
                                println!("{:<25}: ERROR - couldn't decode as raw text", "custom_enum");
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("{:<25}: ERROR - {}", "custom_enum", e);
                }
            }
        }

        println!("\n=== Testing Blanco PostgreSQL Connection ===\n");

        // Test with Blanco's PostgreSQL connection
        let mut postgres_connection =
            crate::PostgresConnection::from_connection_string(&test_connection_string)?;

        // Establish the actual database connection
        postgres_connection.connect(&test_connection_string).await?;

        // Test a simple query to make sure Blanco can handle the data
        let query_result = postgres_connection.execute_query("SELECT * FROM comprehensive_test WHERE id = (SELECT MAX(id) FROM comprehensive_test)", None).await?;

        println!("Blanco PostgreSQL connection test successful!");
        println!("Columns returned: {}", query_result.columns.len());
        println!("Rows returned: {}", query_result.rows.len());

        if let Some(first_row) = query_result.rows.first() {
            println!("\n--- Blanco Serialization Results ---");
            for (i, value) in first_row.iter().enumerate() {
                let column_name = query_result
                    .columns
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                println!("{:<25}: {}", column_name, value);
            }
        }

        println!("\n=== PostgreSQL Data Types Test Completed Successfully! ===");
        Ok(())
    }

    #[test]
    fn test_postgres_table_name_extraction() {
        use blanco_core::Connection;

        let postgres_connection = crate::PostgresConnection::new(
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "user".to_string(),
            Some("password".to_string()),
        );

        // Test table name extraction from query with alias
        let query_with_alias = "SELECT id, status FROM orders o WHERE o.id = 1";
        let extracted_table_name = postgres_connection.extract_actual_table_name(query_with_alias).unwrap();
        assert_eq!(extracted_table_name, Some("orders".to_string()));

        // Test alias resolution
        let resolved_table = postgres_connection.resolve_table_alias(query_with_alias, "o").unwrap();
        assert_eq!(resolved_table, Some("orders".to_string()));

        // Test with different query patterns
        let test_cases = vec![
            ("SELECT * FROM customers", Some("customers")),
            ("SELECT * FROM orders o", Some("orders")),
            ("SELECT * FROM products p WHERE p.id = 1", Some("products")),
            ("SELECT * FROM orders JOIN customers c ON orders.customer_id = c.id", Some("orders")),
        ];

        for (query, expected) in test_cases {
            let result = postgres_connection.extract_actual_table_name(query).unwrap();
            assert_eq!(result, expected.map(String::from), "Failed for query: {}", query);
        }

        println!("✅ PostgreSQL table name extraction tests passed!");
        println!("   Extracted table name from 'orders o': {:?}", extracted_table_name);
        println!("   Resolved alias 'o': {:?}", resolved_table);
    }
}
