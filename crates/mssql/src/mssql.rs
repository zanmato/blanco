pub mod connection;
pub mod schema;

pub use connection::MssqlConnection;

#[cfg(test)]
#[allow(clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use blanco_core::{ColumnType, Connection, connection_trait::RoutineKind};
    use std::env;

    fn default_connection_string() -> String {
        env::var("MSSQL_CONNECTION_STRING").unwrap_or_else(|_| {
            "mssql://sa:Blanco_Passw0rd!@localhost:1433/master?trust_cert=true".to_string()
        })
    }

    /// Decide whether to fail or skip a test when the SQL Server is not
    /// reachable. `BLANCO_RUN_DB_TESTS=1` makes missing servers a hard
    /// failure; otherwise prints a skip message and returns Err so the
    /// caller can early-return `Ok(())`.
    fn require_db_or_skip(
        test_name: &str,
        err: &dyn std::fmt::Display,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1") {
            Err(format!("{test_name}: MSSQL unreachable: {err}").into())
        } else {
            eprintln!(
                "skip {test_name}: MSSQL unreachable ({err}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            Ok(())
        }
    }

    enum ConnectOutcome {
        Connected(MssqlConnection),
        Skip(Result<(), Box<dyn std::error::Error>>),
    }

    async fn connect_or_skip(test_name: &str) -> ConnectOutcome {
        let connection_string = default_connection_string();
        let mut conn = match MssqlConnection::from_connection_string(&connection_string) {
            Ok(c) => c,
            Err(e) => return ConnectOutcome::Skip(require_db_or_skip(test_name, &e)),
        };
        match Connection::connect(&mut conn, &connection_string).await {
            Ok(()) => ConnectOutcome::Connected(conn),
            Err(e) => ConnectOutcome::Skip(require_db_or_skip(test_name, &e)),
        }
    }

    #[tokio::test]
    async fn test_mssql_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let conn = match connect_or_skip("test_mssql_data_type_serialization").await {
            ConnectOutcome::Connected(c) => c,
            ConnectOutcome::Skip(r) => return r,
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
        let conn = match connect_or_skip("test_mssql_null_handling").await {
            ConnectOutcome::Connected(c) => c,
            ConnectOutcome::Skip(r) => return r,
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
        let conn = match connect_or_skip("test_mssql_schema_introspection").await {
            ConnectOutcome::Connected(c) => c,
            ConnectOutcome::Skip(r) => return r,
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
        let conn = match connect_or_skip("test_mssql_write_and_rows_affected").await {
            ConnectOutcome::Connected(c) => c,
            ConnectOutcome::Skip(r) => return r,
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

    #[tokio::test]
    async fn test_mssql_ping_validates_credentials() -> Result<(), Box<dyn std::error::Error>> {
        use std::time::{Duration, Instant};

        let test_name = "test_mssql_ping_validates_credentials";

        // Valid credentials should ping successfully (or skip if unreachable).
        let conn = MssqlConnection::from_connection_string(&default_connection_string())?;
        if let Err(e) = conn.ping().await {
            return require_db_or_skip(test_name, &e);
        }

        // A bad password must fail, and fail fast: the single-shot
        // `ManageConnection::connect` surfaces the login error immediately
        // instead of `Pool::get` retrying until the ~30s connection timeout.
        let bad = MssqlConnection::from_connection_string(
            "mssql://sa:Wrong_Passw0rd!@localhost:1433/master?trust_cert=true",
        )?;
        let started = Instant::now();
        let result = bad.ping().await;
        let elapsed = started.elapsed();

        assert!(result.is_err(), "ping with a bad password must error");
        assert!(
            elapsed < Duration::from_secs(10),
            "ping should fail fast, took {elapsed:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mssql_routines_listed_and_ddl() -> Result<(), Box<dyn std::error::Error>> {
        let conn = match connect_or_skip("test_mssql_routines_listed_and_ddl").await {
            ConnectOutcome::Connected(c) => c,
            ConnectOutcome::Skip(r) => return r,
        };

        // Clean slate (trigger must go before its table).
        for stmt in [
            "IF OBJECT_ID('dbo.blanco_routine_trg', 'TR') IS NOT NULL DROP TRIGGER dbo.blanco_routine_trg",
            "DROP PROCEDURE IF EXISTS dbo.blanco_routine_proc",
            "DROP FUNCTION IF EXISTS dbo.blanco_routine_fn",
            "DROP TABLE IF EXISTS dbo.blanco_routine_tbl",
        ] {
            let _ = conn.execute_query(stmt, None, None).await;
        }

        conn.execute_query("CREATE TABLE dbo.blanco_routine_tbl (id INT)", None, None)
            .await?;
        conn.execute_query(
            "EXEC('CREATE PROCEDURE dbo.blanco_routine_proc AS SELECT 1')",
            None,
            None,
        )
        .await?;
        conn.execute_query(
            "EXEC('CREATE FUNCTION dbo.blanco_routine_fn() RETURNS INT AS BEGIN RETURN 1 END')",
            None,
            None,
        )
        .await?;
        conn.execute_query(
            "EXEC('CREATE TRIGGER dbo.blanco_routine_trg ON dbo.blanco_routine_tbl AFTER INSERT AS SELECT 1')",
            None,
            None,
        )
        .await?;

        let procedures = conn.list_procedures(Some("dbo")).await?;
        assert!(
            procedures.iter().any(|p| p == "blanco_routine_proc"),
            "stored procedure should be listed, got {procedures:?}"
        );

        let functions = conn.list_functions(Some("dbo")).await?;
        assert!(
            functions.iter().any(|f| f == "blanco_routine_fn"),
            "function should be listed, got {functions:?}"
        );

        let triggers = conn.list_triggers(Some("dbo")).await?;
        assert!(
            triggers.iter().any(|t| t == "blanco_routine_trg"),
            "trigger should be listed, got {triggers:?}"
        );

        let proc_ddl = conn
            .object_ddl(RoutineKind::Procedure, Some("dbo"), "blanco_routine_proc")
            .await?;
        assert!(
            proc_ddl.contains("CREATE PROCEDURE"),
            "DDL should contain the CREATE PROCEDURE source, got: {proc_ddl}"
        );

        for stmt in [
            "DROP TRIGGER dbo.blanco_routine_trg",
            "DROP PROCEDURE dbo.blanco_routine_proc",
            "DROP FUNCTION dbo.blanco_routine_fn",
            "DROP TABLE dbo.blanco_routine_tbl",
        ] {
            let _ = conn.execute_query(stmt, None, None).await;
        }

        Ok(())
    }
}
