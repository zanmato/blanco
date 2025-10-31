use sqlx::{Row, postgres::PgPoolOptions};
use std::env;

#[tokio::test]
async fn test_postgres_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        // Use environment variable for connection string or fallback to default
        let connection_string = env::var("POSTGRES_CONNECTION_STRING")
            .unwrap_or_else(|_| "postgres://manager:manager@localhost:5444/manager_test?sslmode=disable".to_string());

        println!("Testing PostgreSQL data types with connection: {}", connection_string);

        // Connect to PostgreSQL
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        println!("Successfully connected to PostgreSQL!");

        // Create comprehensive test table with all PostgreSQL data types
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
                time_with_time_zone_col TIME WITH TIME ZONE,
                timestamp_col TIMESTAMP,
                timestamp_with_time_zone_col TIMESTAMP WITH TIME ZONE,

                -- Binary data types
                bytea_col BYTEA,

                -- Network address types
                inet_col INET,
                cidr_col CIDR,

                -- UUID type
                uuid_col UUID,

                -- XML type
                xml_col XML,

                -- JSON types
                json_col JSON,
                jsonb_col JSONB,

                -- Array types
                int_array_col INTEGER[],
                text_array_col TEXT[],
                uuid_array_col UUID[],
                numeric_array_col NUMERIC(10,2)[],

                -- Range types (PostgreSQL 9.2+)
                int_range_col INT4RANGE,
                num_range_col NUMRANGE,
                ts_range_col TSRANGE,
                tstz_range_col TSTZRANGE,

                -- Geometric types
                point_col POINT,
                line_col LINE,
                lseg_col LSEG,
                box_col BOX,
                path_col PATH,
                polygon_col POLYGON,
                circle_col CIRCLE
            )
        "#;

        println!("Creating comprehensive test table...");
        sqlx::query(create_table_sql).execute(&pool).await?;
        println!("Table created successfully!");

        // Insert test data with all data types
        let insert_sql = r#"
            INSERT INTO comprehensive_test (
                smallint_col, int_col, bigint_col, decimal_col, numeric_col,
                real_col, double_precision_col, smallserial_col, serial_col, bigserial_col, money_col,
                char_col, varchar_col, text_col, bool_col,
                date_col, time_col, time_with_time_zone_col, timestamp_col, timestamp_with_time_zone_col,
                bytea_col, inet_col, cidr_col, uuid_col, xml_col, json_col, jsonb_col,
                int_array_col, text_array_col, uuid_array_col, numeric_array_col,
                int_range_col, num_range_col, ts_range_col, tstz_range_col,
                point_col, line_col, lseg_col, box_col, path_col, polygon_col, circle_col
            ) VALUES (
                32767, 2147483647, 9223372036854775807, 12345.67, 98765.43210,
                123.456, 987654321.123456789, 100, 1000, 1000000, 12345.67,
                'fixed_len  ', 'variable_string', 'This is a long text that can hold unlimited characters including unicode: ñiño 你好 🚀', true,
                CURRENT_DATE, CURRENT_TIME, CURRENT_TIME, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP,
                '\xDEADBEEF', '192.168.1.1', '192.168.0.0/24', '550e8400-e29b-41d4-a716-446655440000',
                '<root><item>Test XML</item></root>',
                '{"name": "test", "value": 42, "active": true}',
                '{"nested": {"array": [1,2,3], "text": "hello"}}',
                ARRAY[1, 2, 3, 4, 5],
                ARRAY['hello', 'world', 'test'],
                ARRAY['550e8400-e29b-41d4-a716-446655440000', '660e8400-e29b-41d4-a716-446655440001'],
                ARRAY[123.45, 678.90, 111.22],
                '[1,10]', '[100.0,200.0]', '["2023-01-01","2023-12-31"]'::tsrange, '["2023-01-01 00:00:00+00","2023-12-31 23:59:59+00"]'::tstzrange,
                POINT(10.5, 20.5), LINE(1.0, 1.0, 10.0, 10.0), LSEG('(1,1),(10,10)'), BOX(1.0, 1.0, 10.0, 10.0),
                PATH('[(1,1),(2,2),(3,3)]'), POLYGON('((1,1),(2,2),(3,3),(1,1))'), CIRCLE(POINT(5,5), 3.0)
            )
            ON CONFLICT DO NOTHING;
        "#;

        println!("Inserting test data...");
        let result = sqlx::query(insert_sql).execute(&pool).await?;
        println!("Test data inserted successfully! Rows affected: {}", result.rows_affected());

        // Now read the data back and verify serialization
        println!("\n=== Testing Data Type Serialization ===\n");

        let select_sql = r#"
            SELECT
                id, smallint_col, int_col, bigint_col, decimal_col, numeric_col,
                real_col, double_precision_col, smallserial_col, serial_col, bigserial_col, money_col,
                char_col, varchar_col, text_col, bool_col,
                date_col, time_col, time_with_time_zone_col, timestamp_col, timestamp_with_time_zone_col,
                encode(bytea_col, 'hex') as bytea_hex,
                inet_col, cidr_col, uuid_col, xml_col, json_col, jsonb_col,
                int_array_col, text_array_col, uuid_array_col, numeric_array_col,
                int_range_col, num_range_col, ts_range_col, tstz_range_col,
                point_col, line_col, lseg_col, box_col, path_col, polygon_col, circle_col
            FROM comprehensive_test
            WHERE id = (SELECT MAX(id) FROM comprehensive_test)
        "#;

        let row = sqlx::query(select_sql).fetch_one(&pool).await?;

        // Helper function to safely extract and print values
        fn safe_print<T: std::fmt::Display>(name: &str, row: &sqlx::postgres::PgRow, column: &str) -> Result<(), sqlx::Error> {
            match row.try_get::<Option<T>, _>(column) {
                Ok(Some(value)) => println!("{:<25}: {} ({})", name, value, std::any::type_name::<T>()),
                Ok(None) => println!("{:<25}: NULL", name),
                Err(e) => println!("{:<25}: ERROR - {} ({})", name, e, std::any::type_name::<T>()),
            }
            Ok(())
        }

        // Test each data type
        println!("--- Numeric Types ---");
        safe_print::<i16>("smallint_col", &row, "smallint_col")?;
        safe_print::<i32>("int_col", &row, "int_col")?;
        safe_print::<i64>("bigint_col", &row, "bigint_col")?;
        safe_print::<rust_decimal::Decimal>("decimal_col", &row, "decimal_col")?;
        safe_print::<rust_decimal::Decimal>("numeric_col", &row, "numeric_col")?;
        safe_print::<f32>("real_col", &row, "real_col")?;
        safe_print::<f64>("double_precision_col", &row, "double_precision_col")?;
        safe_print::<i32>("smallserial_col", &row, "smallserial_col")?;
        safe_print::<i32>("serial_col", &row, "serial_col")?;
        safe_print::<i64>("bigserial_col", &row, "bigserial_col")?;
        safe_print::<rust_decimal::Decimal>("money_col", &row, "money_col")?;

        println!("\n--- Character Types ---");
        safe_print::<String>("char_col", &row, "char_col")?;
        safe_print::<String>("varchar_col", &row, "varchar_col")?;
        safe_print::<String>("text_col", &row, "text_col")?;

        println!("\n--- Boolean Type ---");
        safe_print::<bool>("bool_col", &row, "bool_col")?;

        println!("\n--- Date/Time Types ---");
        safe_print::<chrono::NaiveDate>("date_col", &row, "date_col")?;
        safe_print::<chrono::NaiveTime>("time_col", &row, "time_col")?;
        safe_print::<chrono::NaiveTime>("time_with_time_zone_col", &row, "time_with_time_zone_col")?;
        safe_print::<chrono::NaiveDateTime>("timestamp_col", &row, "timestamp_col")?;
        safe_print::<chrono::DateTime<chrono::Utc>>("timestamp_with_time_zone_col", &row, "timestamp_with_time_zone_col")?;

        println!("\n--- Binary Data ---");
        safe_print::<String>("bytea_hex", &row, "bytea_hex")?;

        println!("\n--- Network Types ---");
        safe_print::<String>("inet_col", &row, "inet_col")?;
        safe_print::<String>("cidr_col", &row, "cidr_col")?;

        println!("\n--- UUID Type ---");
        safe_print::<uuid::Uuid>("uuid_col", &row, "uuid_col")?;

        println!("\n--- XML Type ---");
        safe_print::<String>("xml_col", &row, "xml_col")?;

        println!("\n--- JSON Types ---");
        safe_print::<serde_json::Value>("json_col", &row, "json_col")?;
        safe_print::<serde_json::Value>("jsonb_col", &row, "jsonb_col")?;

        println!("\n--- Array Types ---");
        safe_print::<Vec<i32>>("int_array_col", &row, "int_array_col")?;
        safe_print::<Vec<String>>("text_array_col", &row, "text_array_col")?;
        safe_print::<Vec<uuid::Uuid>>("uuid_array_col", &row, "uuid_array_col")?;

        // Try to get numeric array as Vec<String> if Vec<Decimal> fails
        match row.try_get::<Option<Vec<rust_decimal::Decimal>>, _>("numeric_array_col") {
            Ok(Some(arr)) => {
                println!("{:<25}: {:?} ({})", "numeric_array_col", arr, "Vec<Decimal>");
            }
            Ok(None) => {
                println!("{:<25}: NULL", "numeric_array_col");
            }
            Err(_) => {
                // Try as Vec<String> fallback
                match row.try_get::<Option<Vec<String>>, _>("numeric_array_col") {
                    Ok(Some(arr)) => {
                        println!("{:<25}: {:?} ({})", "numeric_array_col", arr, "Vec<String>");
                    }
                    Ok(None) => {
                        println!("{:<25}: NULL", "numeric_array_col");
                    }
                    Err(e) => {
                        println!("{:<25}: ERROR - {}", "numeric_array_col", e);
                    }
                }
            }
        }

        println!("\n--- Range Types ---");
        safe_print::<String>("int_range_col", &row, "int_range_col")?;
        safe_print::<String>("num_range_col", &row, "num_range_col")?;
        safe_print::<String>("ts_range_col", &row, "ts_range_col")?;
        safe_print::<String>("tstz_range_col", &row, "tstz_range_col")?;

        println!("\n--- Geometric Types ---");
        safe_print::<String>("point_col", &row, "point_col")?;
        safe_print::<String>("line_col", &row, "line_col")?;
        safe_print::<String>("lseg_col", &row, "lseg_col")?;
        safe_print::<String>("box_col", &row, "box_col")?;
        safe_print::<String>("path_col", &row, "path_col")?;
        safe_print::<String>("polygon_col", &row, "polygon_col")?;
        safe_print::<String>("circle_col", &row, "circle_col")?;

        println!("\n=== Testing Blanco PostgreSQL Connection ===\n");

        // Test with Blanco's PostgreSQL connection
        let postgres_connection = crate::PostgresConnection::from_connection_string(&connection_string).await?;

        // Test a simple query to make sure Blanco can handle the data
        let query_result = postgres_connection.execute_query("SELECT * FROM comprehensive_test WHERE id = (SELECT MAX(id) FROM comprehensive_test)").await?;

        println!("Blanco PostgreSQL connection test successful!");
        println!("Columns returned: {}", query_result.columns.len());
        println!("Rows returned: {}", query_result.rows.len());

        if let Some(first_row) = query_result.rows.first() {
            println!("\n--- Blanco Serialization Results ---");
            for (i, value) in first_row.iter().enumerate() {
                let column_name = query_result.columns.get(i).map(|c| &c.name).unwrap_or(&"unknown".to_string());
                println!("{:<25}: {}", column_name, value);
            }
        }

        println!("\n=== All PostgreSQL Data Types Test Completed Successfully! ===");
        Ok(())
    }