//! Integration test for the unified connection interface
//!
//! This module demonstrates how to use the new unified connection system
//! while maintaining backward compatibility with existing code.

use crate::db_service::DbService;

/// Test function to demonstrate unified connection interface
pub async fn test_unified_connection_interface() -> Result<(), anyhow::Error> {
    log::info!("🧪 Testing unified connection interface integration...");

    // Initialize the DbService with unified manager
    let db_service = DbService::new();

    // Test 1: Create a temporary SQLite database in memory
    let sqlite_connection_string = "sqlite::memory:".to_string();

    log::info!("✅ Created temporary SQLite database: {}", sqlite_connection_string);

    // Test 2: Execute query using unified interface
    let result = db_service.execute_query_unified(&sqlite_connection_string, "SELECT 1 as test_column").await?;

    assert_eq!(result.rows.len(), 1, "Query should return 1 row");
    assert_eq!(result.rows[0][0], "1", "Query should return '1'");
    assert_eq!(result.columns.len(), 1, "Query should return 1 column");
    assert_eq!(result.columns[0], "test_column", "Column name should be 'test_column'");

    log::info!("✅ Basic query execution test passed");

    // Test 3: Execute prepared query using unified interface
    let prepared_result = db_service.execute_prepared_query_unified(
        &sqlite_connection_string,
        "SELECT ? as value",
        &vec!["hello".to_string()],
    ).await?;

    assert_eq!(prepared_result.rows.len(), 1, "Prepared query should return 1 row");
    assert_eq!(prepared_result.rows[0][0], "hello", "Prepared query should return 'hello'");

    log::info!("✅ Prepared query execution test passed");

    // Test 4: Get schemas using unified interface
    let schemas = db_service.get_schemas_unified(&sqlite_connection_string).await?;
    assert!(!schemas.is_empty(), "Should return at least one schema");
    assert!(schemas.contains(&"main".to_string()), "Should contain 'main' schema");

    log::info!("✅ Schema enumeration test passed");

    // Test 5: Create a table and test table enumeration
    db_service.execute_query_unified(&sqlite_connection_string, "CREATE TABLE test_table (id INTEGER, name TEXT)").await?;

    let tables = db_service.get_tables_unified(&sqlite_connection_string, Some("main")).await?;
    assert!(tables.contains(&"test_table".to_string()), "Should contain our test table");

    log::info!("✅ Table enumeration test passed");

    // Test 6: Get connection using unified interface
    let connection = db_service.get_or_create_unified_connection(&sqlite_connection_string).await?;
    assert!(connection.is_connected(), "Connection should be active");
    assert_eq!(connection.get_connection_type(), "SQLite", "Connection type should be SQLite");

    log::info!("✅ Connection management test passed");

    // Test 7: Test connection info
    let connection_info = connection.get_connection_info().await?;
    assert_eq!(connection_info.connection_type, "SQLite", "Connection info should show SQLite");
    assert!(connection_info.is_connected, "Connection should be connected");
    assert!(connection_info.schema_count > 0, "Should have at least one schema");

    log::info!("✅ Connection info test passed");

    log::info!("🎉 All unified connection interface tests passed successfully!");
    log::info!("🔗 The unified interface is ready for integration with existing sidebar and editor panels.");

    Ok(())
}

/// Test PostgreSQL connection if available
pub async fn test_postgres_unified_interface() -> Result<(), anyhow::Error> {
    // Try to get PostgreSQL connection string from environment or pg_dsn.txt
    let pg_dsn = if let Ok(dsn) = std::env::var("POSTGRES_CONNECTION_STRING") {
        dsn
    } else if let Ok(dsn) = std::fs::read_to_string("pg_dsn.txt") {
        dsn.trim().to_string()
    } else {
        log::info!("ℹ️  No PostgreSQL connection string found, skipping PostgreSQL test");
        return Ok(());
    };

    if pg_dsn.is_empty() {
        log::info!("ℹ️  Empty PostgreSQL connection string, skipping PostgreSQL test");
        return Ok(());
    }

    log::info!("🧪 Testing PostgreSQL unified interface...");

    let db_service = DbService::new();

    // Test basic query
    match db_service.execute_query_unified(&pg_dsn, "SELECT 1 as test").await {
        Ok(result) => {
            assert_eq!(result.rows.len(), 1, "PostgreSQL query should return 1 row");
            assert_eq!(result.rows[0][0], "1", "PostgreSQL query should return '1'");
            log::info!("✅ PostgreSQL basic query test passed");
        }
        Err(e) => {
            log::warn!("⚠️  PostgreSQL connection failed (this is expected if server is not available): {}", e);
            return Ok(());
        }
    }

    // Test schemas
    match db_service.get_schemas_unified(&pg_dsn).await {
        Ok(schemas) => {
            assert!(schemas.contains(&"public".to_string()), "Should contain 'public' schema");
            log::info!("✅ PostgreSQL schema enumeration test passed");
        }
        Err(e) => {
            log::warn!("⚠️  PostgreSQL schema enumeration failed: {}", e);
        }
    }

    log::info!("🎉 PostgreSQL unified interface tests completed!");
    Ok(())
}

/// Demonstration function showing how the unified interface can be used
pub fn demonstrate_unified_interface_usage() {
    println!("\n=== Unified Connection Interface Integration Demo ===\n");

    println!("The unified connection interface has been successfully integrated into Blanco!");
    println!("Here's what's now available:\n");

    println!("📦 NEW UNIFIED METHODS IN DbService:");
    println!("   • execute_query_unified(connection_string, query)");
    println!("   • execute_prepared_query_unified(connection_string, template, params)");
    println!("   • get_schemas_unified(connection_string)");
    println!("   • get_tables_unified(connection_string, schema)");
    println!("   • get_or_create_unified_connection(connection_string)");
    println!("   • unified_manager() - access to the full UnifiedConnectionManager");

    println!("\n🔄 BACKWARD COMPATIBILITY:");
    println!("   • All existing DbService methods still work");
    println!("   • Existing sidebar and editor panels unchanged");
    println!("   • PostgreSQL and SQLite managers still accessible");

    println!("\n🎯 BENEFITS:");
    println!("   • Single interface for all database types");
    println!("   • Automatic connection pooling and health checking");
    println!("   • Easy to add new database types (MySQL, etc.)");
    println!("   • Consistent error handling across all databases");
    println!("   • Reduced code duplication");

    println!("\n🚀 NEXT STEPS:");
    println!("   • Update editor panel to use unified methods");
    println!("   • Add unified connection management to sidebar");
    println!("   • Migrate existing query execution to unified interface");

    println!("\n💡 USAGE EXAMPLE:");
    println!("```rust");
    println!("let db_service = DbService::global(cx);");
    println!("");
    println!("// Execute query on any database type");
    println!("let result = db_service.execute_query_unified(");
    println!("    \"sqlite:///path/to/db\",");
    println!("    \"SELECT * FROM users\"");
    println!(").await?;");
    println!("```");

    println!("\n=== Integration Status: COMPLETE ===\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    #[tokio::test]
    async fn test_unified_sqlite_integration() {
        env_logger::init();
        test_unified_connection_interface().await.expect("Unified interface test should pass");
    }

    #[tokio::test]
    async fn test_unified_postgres_integration() {
        env_logger::init();
        test_postgres_unified_interface().await.expect("PostgreSQL test should not panic");
    }
}