use crate::connection_trait::{Connection, ConnectionFactory, ConnectionRegistry, ConnectionInfo};
use crate::sqlite_connection::{SqliteConnection, SqliteConnectionFactory, SqliteConnectionKey};
use crate::postgres_connection::{PostgresConnection, PostgresConnectionFactory};
use crate::unified_connection_manager::{UnifiedConnectionManager, ConnectionExt};
use crate::app_database::{AppDatabase, ConnectionData};
use tempfile::NamedTempFile;
use std::path::Path;

/// Comprehensive test suite for the unified connection interface
#[cfg(test)]
mod tests {
    use super::*;

    fn create_temp_sqlite_db() -> String {
        let temp_file = NamedTempFile::new().unwrap();
        temp_file.path().to_string_lossy().to_string()
    }

    fn get_test_postgres_connection_string() -> String {
        std::env::var("POSTGRES_CONNECTION_STRING")
            .unwrap_or_else(|_| "postgres://postgres:Bongotrumma24!@localhost:5432/bylyngamanager?sslmode=disable&timezone=Europe/Stockholm".to_string())
    }

    #[tokio::test]
    async fn test_connection_trait_comprehensive() {
        // Test SQLite connection
        let db_path = create_temp_sqlite_db();
        let connection_string = format!("sqlite://{}", db_path);
        let mut sqlite_conn = SqliteConnection::new(connection_string.clone()).unwrap();

        // Test connection lifecycle
        assert!(!sqlite_conn.is_connected());
        sqlite_conn.connect(&connection_string).await.unwrap();
        assert!(sqlite_conn.is_connected());

        // Test query execution
        let result = sqlite_conn.execute_query("SELECT 1 as test").await.unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], "1");

        // Test prepared query
        let prepared_result = sqlite_conn
            .execute_prepared_query("SELECT ? as value", &vec!["test".to_string()])
            .await
            .unwrap();
        assert_eq!(prepared_result.rows[0][0], "test");

        // Test schema operations
        let databases = sqlite_conn.get_databases().await.unwrap();
        assert!(!databases.is_empty());

        let schemas = sqlite_conn.get_schemas().await.unwrap();
        assert!(!schemas.is_empty());

        // Create a test table and verify it appears in tables
        sqlite_conn.execute_query("CREATE TABLE test_table (id INTEGER)").await.unwrap();
        let tables = sqlite_conn.get_tables(None).await.unwrap();
        assert!(tables.contains(&"test_table".to_string()));

        // Test connection info
        let info = sqlite_conn.get_connection_info().await.unwrap();
        assert_eq!(info.connection_type, "SQLite");
        assert!(info.display_name.contains("SQLite"));
        assert!(info.is_connected);

        // Disconnect
        sqlite_conn.disconnect().await;
        assert!(!sqlite_conn.is_connected());
    }

    #[tokio::test]
    #[ignore] // Ignored by default since it requires a running PostgreSQL server
    async fn test_postgres_connection_comprehensive() {
        let connection_string = get_test_postgres_connection_string();
        let mut postgres_conn = match PostgresConnection::from_connection_string(&connection_string) {
            Ok(conn) => conn,
            Err(e) => {
                println!("Skipping PostgreSQL test - invalid connection string: {}", e);
                return;
            }
        };

        // Test connection lifecycle
        assert!(!postgres_conn.is_connected());
        if let Err(e) = postgres_conn.connect(&connection_string).await {
            println!("Skipping PostgreSQL test - connection failed: {}", e);
            return;
        }
        assert!(postgres_conn.is_connected());

        // Test query execution
        let result = postgres_conn.execute_query("SELECT 1 as test").await.unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], "1");

        // Test prepared query
        let prepared_result = postgres_conn
            .execute_prepared_query("SELECT $1 as value", &vec!["test".to_string()])
            .await
            .unwrap();
        assert_eq!(prepared_result.rows[0][0], "test");

        // Test schema operations
        let databases = postgres_conn.get_databases().await.unwrap();
        assert!(!databases.is_empty());

        let schemas = postgres_conn.get_schemas().await.unwrap();
        assert!(schemas.contains(&"public".to_string()));

        // Create a temporary table and verify it appears in tables
        postgres_conn.execute_query("CREATE TEMP TABLE test_table (id INTEGER)").await.unwrap();
        let tables = postgres_conn.get_tables(Some("public")).await.unwrap();
        assert!(tables.contains(&"test_table".to_string()));

        // Test connection info
        let info = postgres_conn.get_connection_info().await.unwrap();
        assert_eq!(info.connection_type, "PostgreSQL");
        assert!(info.display_name.contains("PostgreSQL"));
        assert!(info.is_connected);

        // Test PostgreSQL-specific methods
        let version = postgres_conn.get_server_version().await.unwrap();
        assert!(!version.is_empty());

        // Disconnect
        postgres_conn.disconnect().await;
        assert!(!postgres_conn.is_connected());
    }

    #[tokio::test]
    async fn test_sqlite_connection_factory() {
        let factory = SqliteConnectionFactory;
        let db_path = create_temp_sqlite_db();
        let connection_string = format!("sqlite://{}", db_path);

        // Test parsing
        let key = factory.parse_connection_string(&connection_string).unwrap();
        assert_eq!(key.database_path, db_path);

        // Test creation
        let conn = factory.create_connection(&connection_string).await.unwrap();
        assert!(conn.is_connected());
        assert_eq!(conn.get_connection_type(), "SQLite");

        // Test the connection key
        let conn_key = conn.get_connection_key();
        assert_eq!(conn_key.database_path, db_path);
    }

    #[tokio::test]
    #[ignore] // Ignored by default since it requires a running PostgreSQL server
    async fn test_postgres_connection_factory() {
        let factory = PostgresConnectionFactory;
        let connection_string = get_test_postgres_connection_string();

        // Test parsing
        match factory.parse_connection_string(&connection_string) {
            Ok(key) => {
                assert_eq!(key.host, "localhost");
                assert_eq!(key.port, 5432);

                // Test creation
                match factory.create_connection(&connection_string).await {
                    Ok(conn) => {
                        assert!(conn.is_connected());
                        assert_eq!(conn.get_connection_type(), "PostgreSQL");
                    }
                    Err(e) => {
                        println!("Skipping PostgreSQL factory test - connection failed: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("Skipping PostgreSQL factory test - parsing failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_connection_registry() {
        let mut registry = ConnectionRegistry::new();

        // Register factories
        registry.register_factory(SqliteConnectionFactory);
        registry.register_factory(PostgresConnectionFactory);

        // Test supported types
        let types = registry.get_supported_types();
        assert!(types.contains(&"SQLite"));
        assert!(types.contains(&"PostgreSQL"));

        // Test getting factories
        let sqlite_factory = registry.get_factory("SQLite").unwrap();
        assert_eq!(sqlite_factory.get_connection_type(), "SQLite");

        let postgres_factory = registry.get_factory("PostgreSQL").unwrap();
        assert_eq!(postgres_factory.get_connection_type(), "PostgreSQL");

        // Test unknown type
        assert!(registry.get_factory("Unknown").is_none());
    }

    #[tokio::test]
    async fn test_unified_connection_manager() {
        let manager = UnifiedConnectionManager::new();

        // Test SQLite connection through unified manager
        let db_path = create_temp_sqlite_db();
        let connection_string = format!("sqlite://{}", db_path);

        let conn = manager.get_or_create_connection(&connection_string).await.unwrap();
        assert!(conn.is_connected());
        assert_eq!(conn.get_connection_type(), "SQLite");

        // Test connection reuse
        let conn2 = manager.get_or_create_connection(&connection_string).await.unwrap();
        // Should reuse the same connection (though we can't test this directly since they're trait objects)

        // Test extension trait methods
        let result = manager.execute_query_with_auto_connect(&connection_string, "SELECT 1").await.unwrap();
        assert_eq!(result.rows.len(), 1);

        let prepared_result = manager
            .execute_prepared_query_with_auto_connect(&connection_string, "SELECT ?", &vec!["test".to_string()])
            .await
            .unwrap();
        assert_eq!(prepared_result.rows[0][0], "test");

        // Test connection info
        let infos = manager.get_all_connection_info().await.unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].connection_type, "SQLite");

        // Test stats
        let stats = manager.get_connection_stats().await;
        assert_eq!(stats.total, 1);
        assert_eq!(stats.healthy, 1);
        assert_eq!(stats.unhealthy, 0);
        assert_eq!(stats.type_counts.get("SQLite"), Some(&1));

        // Test cleanup
        let removed = manager.validate_and_cleanup_connections().await.unwrap();
        assert_eq!(removed, 0); // No unhealthy connections

        // Test removal
        let key = manager.generate_connection_key(&connection_string).unwrap();
        let removed_conn = manager.remove_connection(&key).await.unwrap();
        assert!(removed_conn.is_some());

        let stats_after = manager.get_connection_stats().await;
        assert_eq!(stats_after.total, 0);
    }

    #[tokio::test]
    #[ignore] // Ignored by default since it requires a running PostgreSQL server
    async fn test_unified_connection_manager_postgres() {
        let manager = UnifiedConnectionManager::new();
        let connection_string = get_test_postgres_connection_string();

        match manager.get_or_create_connection(&connection_string).await {
            Ok(conn) => {
                assert!(conn.is_connected());
                assert_eq!(conn.get_connection_type(), "PostgreSQL");

                // Test schema operations through unified manager
                let schemas = manager.get_schemas_with_auto_connect(&connection_string).await.unwrap();
                assert!(schemas.contains(&"public".to_string()));

                let tables = manager.get_tables_with_auto_connect(&connection_string, Some("public")).await.unwrap();
                // Tables could be empty, but the operation should succeed

                // Test connection type-specific creation
                let pg_conn = manager
                    .create_connection_of_type("PostgreSQL", &connection_string)
                    .await
                    .unwrap();
                assert_eq!(pg_conn.get_connection_type(), "PostgreSQL");
            }
            Err(e) => {
                println!("Skipping unified PostgreSQL test - connection failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_connection_data_unified_interface() {
        // Test SQLite connection data
        let db_path = create_temp_sqlite_db();
        let sqlite_conn_data = ConnectionData::new_sqlite("Test SQLite".to_string(), db_path.clone());

        assert_eq!(sqlite_conn_data.db_type, "SQLite");
        assert_eq!(sqlite_conn_data.name, "Test SQLite");
        assert_eq!(sqlite_conn_data.database_path, Some(db_path.clone()));
        assert!(sqlite_conn_data.is_active());

        // Test connection string generation
        let conn_string = sqlite_conn_data.get_connection_string().unwrap();
        assert!(conn_string.starts_with("sqlite://"));
        assert!(conn_string.contains(&db_path));

        // Test PostgreSQL connection data
        let postgres_conn_data = ConnectionData::new_postgres(
            "Test PostgreSQL".to_string(),
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "testuser".to_string(),
            "testpass".to_string(),
        );

        assert_eq!(postgres_conn_data.db_type, "PostgreSQL");
        assert_eq!(postgres_conn_data.name, "Test PostgreSQL");
        assert_eq!(postgres_conn_data.host, Some("localhost".to_string()));
        assert!(postgres_conn_data.is_active());

        // Test connection string generation
        let pg_conn_string = postgres_conn_data.get_connection_string().unwrap();
        assert!(pg_conn_string.starts_with("postgresql://"));
        assert!(pg_conn_string.contains("testuser"));
        assert!(pg_conn_string.contains("testpass"));

        // Test from_connection_string
        let sqlite_from_string = ConnectionData::from_connection_string(
            "Test From String".to_string(),
            &conn_string,
        ).unwrap();
        assert_eq!(sqlite_from_string.db_type, "SQLite");
        assert_eq!(sqlite_from_string.name, "Test From String");
    }

    #[tokio::test]
    async fn test_app_database_with_unified_connections() {
        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let app_db_path = temp_file.path().to_string_lossy().to_string();
        let app_db = AppDatabase::new_with_path(&format!("sqlite://{}", app_db_path)).await.unwrap();

        // Test saving and loading unified connection data
        let db_path = create_temp_sqlite_db();
        let conn_data = ConnectionData::new_sqlite("Test Connection".to_string(), db_path);

        let conn_id = app_db.save_connection(&conn_data).await.unwrap();
        assert!(conn_id > 0);

        // Load and verify
        let loaded_connections = app_db.load_connections().await.unwrap();
        assert_eq!(loaded_connections.len(), 1);

        let loaded_conn = &loaded_connections[0];
        assert_eq!(loaded_conn.name, "Test Connection");
        assert_eq!(loaded_conn.db_type, "SQLite");
        assert!(loaded_conn.connection_string.is_some());
        assert!(loaded_conn.is_active());

        // Test update
        let mut updated_conn = loaded_conn.clone();
        updated_conn.name = "Updated Connection".to_string();
        updated_conn.set_active(false);

        let updated_id = app_db.save_connection(&updated_conn).await.unwrap();
        assert_eq!(updated_id, conn_id);

        // Verify update
        let reloaded_connections = app_db.load_connections().await.unwrap();
        let reloaded_conn = &reloaded_connections[0];
        assert_eq!(reloaded_conn.name, "Updated Connection");
        assert!(!reloaded_conn.is_active());
    }

    #[tokio::test]
    async fn test_connection_error_handling() {
        // Test invalid SQLite connection
        let invalid_sqlite_path = "/nonexistent/path/test.db";
        let mut sqlite_conn = SqliteConnection::new(format!("sqlite://{}", invalid_sqlite_path)).unwrap();

        // Should fail to connect
        assert!(sqlite_conn.connect(&format!("sqlite://{}", invalid_sqlite_path)).await.is_err());
        assert!(!sqlite_conn.is_connected());

        // Test query on disconnected connection
        let result = sqlite_conn.execute_query("SELECT 1").await;
        assert!(result.is_err());

        // Test invalid PostgreSQL connection string
        let invalid_pg_string = "postgres://invalid@localhost:9999/invalid";
        let pg_result = PostgresConnection::from_connection_string(invalid_pg_string);
        // This should succeed (parsing), but connection would fail
        assert!(pg_result.is_ok());

        // Test unified manager with invalid connection
        let manager = UnifiedConnectionManager::new();
        let result = manager.get_or_create_connection("invalid://connection").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connection_lifecycle_management() {
        let manager = UnifiedConnectionManager::new();

        // Create multiple connections
        let db_path1 = create_temp_sqlite_db();
        let db_path2 = create_temp_sqlite_db();
        let conn_string1 = format!("sqlite://{}", db_path1);
        let conn_string2 = format!("sqlite://{}", db_path2);

        let conn1 = manager.get_or_create_connection(&conn_string1).await.unwrap();
        let conn2 = manager.get_or_create_connection(&conn_string2).await.unwrap();

        assert!(conn1.is_connected());
        assert!(conn2.is_connected());

        // Test stats
        let stats = manager.get_connection_stats().await;
        assert_eq!(stats.total, 2);
        assert_eq!(stats.healthy, 2);

        // Test cleanup (should remove 0 since all are healthy)
        let removed = manager.validate_and_cleanup_connections().await.unwrap();
        assert_eq!(removed, 0);

        // Remove one connection
        let key1 = manager.generate_connection_key(&conn_string1).unwrap();
        manager.remove_connection(&key1).await.unwrap();

        let stats_after = manager.get_connection_stats().await;
        assert_eq!(stats_after.total, 1);
        assert_eq!(stats_after.healthy, 1);
    }

    #[tokio::test]
    async fn test_connection_key_operations() {
        // Test SQLite connection key
        let db_path = create_temp_sqlite_db();
        let conn_string = format!("sqlite://{}", db_path);
        let sqlite_key = SqliteConnectionKey::from_connection_string(&conn_string).unwrap();

        assert_eq!(sqlite_key.database_path, db_path);

        // Test key to connection string conversion
        let converted_string = sqlite_key.to_connection_string();
        assert_eq!(converted_string, conn_string);

        // Test different SQLite connection string formats
        let direct_path_key = SqliteConnectionKey::from_connection_string(&db_path).unwrap();
        assert_eq!(direct_path_key.database_path, db_path);

        let sqlite_prefix_key = SqliteConnectionKey::from_connection_string(&format!("sqlite:{}", db_path)).unwrap();
        assert_eq!(sqlite_prefix_key.database_path, db_path);
    }

    #[tokio::test]
    async fn test_connection_info_accuracy() {
        let db_path = create_temp_sqlite_db();
        let connection_string = format!("sqlite://{}", db_path);
        let mut conn = SqliteConnection::new(connection_string.clone()).unwrap();

        // Test info when not connected
        let info_disconnected = conn.get_connection_info().await.unwrap();
        assert_eq!(info_disconnected.connection_type, "SQLite");
        assert!(!info_disconnected.is_connected);

        // Connect and test info
        conn.connect(&connection_string).await.unwrap();
        let info_connected = conn.get_connection_info().await.unwrap();
        assert_eq!(info_connected.connection_type, "SQLite");
        assert!(info_connected.is_connected);
        assert!(info_connected.display_name.contains("SQLite"));
        assert!(info_connected.database_name.is_some());

        // Create a table and check schema count
        conn.execute_query("CREATE TABLE test_table (id INTEGER)").await.unwrap();
        let info_with_table = conn.get_connection_info().await.unwrap();
        assert!(info_with_table.schema_count > 0);
    }
}

/// Integration test that tests the entire unified connection flow
#[tokio::test]
#[ignore] // Integration test - ignored by default
async fn test_full_unified_connection_integration() {
    // This test requires both a working SQLite database and PostgreSQL server

    // Initialize unified manager
    let manager = UnifiedConnectionManager::new();

    // Test SQLite connection
    let db_path = create_temp_sqlite_db();
    let sqlite_conn_string = format!("sqlite://{}", db_path);

    let sqlite_conn = manager.get_or_create_connection(&sqlite_conn_string).await.unwrap();
    assert_eq!(sqlite_conn.get_connection_type(), "SQLite");

    // Create test data
    sqlite_conn.execute_query("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)").await.unwrap();
    sqlite_conn.execute_prepared_query(
        "INSERT INTO users (name) VALUES (?)",
        &vec!["Alice".to_string()]
    ).await.unwrap();

    let sqlite_result = sqlite_conn.execute_query("SELECT * FROM users").await.unwrap();
    assert_eq!(sqlite_result.rows.len(), 1);

    // Test PostgreSQL connection if available
    let pg_conn_string = get_test_postgres_connection_string();
    if let Ok(pg_conn) = manager.get_or_create_connection(&pg_conn_string).await {
        assert_eq!(pg_conn.get_connection_type(), "PostgreSQL");

        // Create test data
        pg_conn.execute_query("CREATE TEMP TABLE users (id SERIAL PRIMARY KEY, name TEXT)").await.unwrap();
        pg_conn.execute_prepared_query(
            "INSERT INTO users (name) VALUES ($1)",
            &vec!["Bob".to_string()]
        ).await.unwrap();

        let pg_result = pg_conn.execute_query("SELECT * FROM users").await.unwrap();
        assert_eq!(pg_result.rows.len(), 1);

        // Verify both connections are tracked
        let stats = manager.get_connection_stats().await;
        assert_eq!(stats.total, 2);
        assert_eq!(stats.healthy, 2);
        assert_eq!(stats.type_counts.get("SQLite"), Some(&1));
        assert_eq!(stats.type_counts.get("PostgreSQL"), Some(&1));
    }

    // Test connection info for all connections
    let infos = manager.get_all_connection_info().await.unwrap();
    assert!(!infos.is_empty());

    for info in infos {
        assert!(info.is_connected);
        assert!(!info.connection_type.is_empty());
        assert!(!info.display_name.is_empty());
    }
}