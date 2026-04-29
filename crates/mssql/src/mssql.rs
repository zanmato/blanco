pub mod connection;
pub mod schema;

pub use connection::MssqlConnection;

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::{Connection, connection_trait::ColumnType};
    use std::env;

    fn default_connection_string() -> String {
        env::var("MSSQL_CONNECTION_STRING").unwrap_or_else(|_| {
            "mssql://sa:Blanco_Passw0rd!@localhost:1433/master?trust_cert=true".to_string()
        })
    }

    async fn connect_or_skip() -> Option<MssqlConnection> {
        let connection_string = default_connection_string();
        let mut conn = match MssqlConnection::from_connection_string(&connection_string) {
            Ok(c) => c,
            Err(e) => {
                println!("Skipping MSSQL test - failed to parse connection string: {e}");
                return None;
            }
        };
        match Connection::connect(&mut conn, &connection_string).await {
            Ok(()) => Some(conn),
            Err(e) => {
                println!("Skipping MSSQL test - server not reachable at {connection_string}: {e}");
                println!("To run this test, start the container: docker compose up -d mssqltestdb");
                None
            }
        }
    }

    #[tokio::test]
    async fn test_mssql_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let Some(conn) = connect_or_skip().await else {
            return Ok(());
        };

        let _ = conn
            .execute_query("DROP TABLE IF EXISTS comprehensive_test", None, None)
            .await;

        conn.execute_query(
            r#"
            CREATE TABLE comprehensive_test (
                id INT IDENTITY(1,1) PRIMARY KEY,
                bit_col BIT,
                tinyint_col TINYINT,
                smallint_col SMALLINT,
                int_col INT,
                bigint_col BIGINT,
                decimal_col DECIMAL(10, 2),
                numeric_col NUMERIC(15, 5),
                real_col REAL,
                float_col FLOAT,
                money_col MONEY,
                char_col CHAR(10),
                varchar_col VARCHAR(255),
                nvarchar_col NVARCHAR(255),
                text_col VARCHAR(MAX),
                date_col DATE,
                time_col TIME,
                datetime2_col DATETIME2,
                datetimeoffset_col DATETIMEOFFSET,
                uuid_col UNIQUEIDENTIFIER,
                varbinary_col VARBINARY(16),
                json_col NVARCHAR(MAX)
            )
            "#,
            None,
            None,
        )
        .await?;

        conn.execute_query(
            r#"
            INSERT INTO comprehensive_test (
                bit_col, tinyint_col, smallint_col, int_col, bigint_col,
                decimal_col, numeric_col, real_col, float_col, money_col,
                char_col, varchar_col, nvarchar_col, text_col,
                date_col, time_col, datetime2_col, datetimeoffset_col,
                uuid_col, varbinary_col, json_col
            ) VALUES (
                1, 255, 32767, 2147483647, 9223372036854775807,
                12345.67, 98765.43210, 123.456, 987654321.123456789, 12345.67,
                'fixed_len ', 'variable_string', N'unicode: ñiño 你好',
                'a longer text value',
                '2024-03-14', '12:34:56', '2024-03-14T12:34:56',
                '2024-03-14T12:34:56+00:00',
                '550E8400-E29B-41D4-A716-446655440000',
                0x0102030405,
                '{"name":"test","value":42,"active":true}'
            )
            "#,
            None,
            None,
        )
        .await?;

        let query_result = conn
            .execute_query(
                "SELECT TOP 1 * FROM comprehensive_test ORDER BY id DESC",
                None,
                None,
            )
            .await?;

        assert!(!query_result.rows.is_empty(), "Query should return one row");
        let first_row = &query_result.rows[0];

        let value_map: std::collections::HashMap<String, Option<String>> = query_result
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| (col.clone(), first_row[i].clone()))
            .collect();

        let get_val = |key: &str| -> String {
            value_map
                .get(key)
                .and_then(|v| v.clone())
                .unwrap_or_default()
        };

        assert_eq!(get_val("bit_col"), "true");
        assert_eq!(get_val("tinyint_col"), "255");
        assert_eq!(get_val("smallint_col"), "32767");
        assert_eq!(get_val("int_col"), "2147483647");
        assert_eq!(get_val("bigint_col"), "9223372036854775807");
        assert_eq!(get_val("decimal_col"), "12345.67");
        assert!(get_val("numeric_col").starts_with("98765.4321"));
        assert!(get_val("real_col").starts_with("123.45"));
        assert!(get_val("float_col").starts_with("987654321"));
        assert_eq!(get_val("money_col"), "12345.67");

        assert!(get_val("char_col").starts_with("fixed_len"));
        assert_eq!(get_val("varchar_col"), "variable_string");
        assert_eq!(get_val("nvarchar_col"), "unicode: ñiño 你好");
        assert_eq!(get_val("text_col"), "a longer text value");

        assert_eq!(get_val("date_col"), "2024-03-14");
        assert!(get_val("time_col").starts_with("12:34:56"));
        assert!(get_val("datetime2_col").starts_with("2024-03-14"));
        assert!(get_val("datetimeoffset_col").contains("2024-03-14"));

        assert_eq!(
            get_val("uuid_col").to_lowercase(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(get_val("varbinary_col"), "0102030405");
        assert!(get_val("json_col").contains("\"name\":\"test\""));

        let type_map: std::collections::HashMap<String, ColumnType> = query_result
            .columns
            .iter()
            .zip(query_result.column_types.iter())
            .map(|(c, t)| (c.clone(), *t))
            .collect();
        assert_eq!(type_map["int_col"], ColumnType::Integer);
        assert_eq!(type_map["bit_col"], ColumnType::Boolean);
        assert_eq!(type_map["float_col"], ColumnType::Numeric);
        assert_eq!(type_map["uuid_col"], ColumnType::Uuid);
        assert_eq!(type_map["varbinary_col"], ColumnType::Binary);

        Ok(())
    }

    #[tokio::test]
    async fn test_mssql_null_handling() -> Result<(), Box<dyn std::error::Error>> {
        let Some(conn) = connect_or_skip().await else {
            return Ok(());
        };

        let _ = conn
            .execute_query("DROP TABLE IF EXISTS null_test", None, None)
            .await;
        conn.execute_query(
            "CREATE TABLE null_test (id INT, name NVARCHAR(50) NULL, age INT NULL)",
            None,
            None,
        )
        .await?;

        conn.execute_query(
            "INSERT INTO null_test (id, name, age) VALUES (1, NULL, NULL)",
            None,
            None,
        )
        .await?;

        let result = conn
            .execute_query(
                "SELECT id, name, age FROM null_test WHERE id = 1",
                None,
                None,
            )
            .await?;

        assert_eq!(result.rows.len(), 1);
        let row = &result.rows[0];
        assert_eq!(row[0], Some("1".to_string()));
        assert_eq!(row[1], None, "NULL varchar should deserialize to None");
        assert_eq!(row[2], None, "NULL int should deserialize to None");

        Ok(())
    }

    #[tokio::test]
    async fn test_mssql_schema_introspection() -> Result<(), Box<dyn std::error::Error>> {
        let Some(conn) = connect_or_skip().await else {
            return Ok(());
        };

        let _ = conn
            .execute_query(
                "IF OBJECT_ID('blanco_test.widgets') IS NOT NULL DROP TABLE blanco_test.widgets",
                None,
                None,
            )
            .await;
        let _ = conn
            .execute_query(
                "IF NOT EXISTS (SELECT * FROM sys.schemas WHERE name = 'blanco_test') EXEC('CREATE SCHEMA blanco_test')",
                None,
                None,
            )
            .await?;

        conn.execute_query(
            r#"CREATE TABLE blanco_test.widgets (
                widget_id INT IDENTITY(1,1) PRIMARY KEY,
                label NVARCHAR(100) NOT NULL,
                nullable_note NVARCHAR(MAX) NULL
            )"#,
            None,
            None,
        )
        .await?;

        let schemas = conn.get_schemas().await?;
        assert!(
            schemas.iter().any(|s| s == "blanco_test"),
            "Expected blanco_test in {schemas:?}"
        );

        let tables = conn.get_tables(Some("blanco_test")).await?;
        assert!(
            tables.iter().any(|t| t == "widgets"),
            "Expected widgets in {tables:?}"
        );

        let columns = conn
            .get_columns_for_table("widgets", Some("blanco_test"))
            .await?;
        let widget_id = columns.iter().find(|c| c.name == "widget_id").unwrap();
        assert!(widget_id.is_primary_key);
        assert!(!widget_id.is_nullable);

        let label = columns.iter().find(|c| c.name == "label").unwrap();
        assert!(!label.is_nullable);
        assert!(!label.is_primary_key);

        let note = columns.iter().find(|c| c.name == "nullable_note").unwrap();
        assert!(note.is_nullable);

        Ok(())
    }

    #[tokio::test]
    async fn test_mssql_write_and_rows_affected() -> Result<(), Box<dyn std::error::Error>> {
        let Some(conn) = connect_or_skip().await else {
            return Ok(());
        };

        let _ = conn
            .execute_query("DROP TABLE IF EXISTS write_test", None, None)
            .await;
        conn.execute_query(
            "CREATE TABLE write_test (id INT, name NVARCHAR(50) NULL)",
            None,
            None,
        )
        .await?;

        let affected = conn
            .execute_write(
                "INSERT INTO write_test (id, name) VALUES (@P1, @P2)",
                None,
                &[Some("42".to_string()), None],
            )
            .await?;
        assert_eq!(affected, 1);

        let result = conn
            .execute_query("SELECT id, name FROM write_test", None, None)
            .await?;
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], Some("42".to_string()));
        assert_eq!(
            result.rows[0][1], None,
            "NULL parameter must land in the column as SQL NULL"
        );

        Ok(())
    }
}
