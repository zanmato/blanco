use anyhow::Result;
use database::DatabaseServiceTrait;

/// Fetch table names using the DbService
pub async fn fetch_tables(
    db_service: &dyn DatabaseServiceTrait,
    connection_id: i64,
    database_name: &str,
) -> Result<Vec<String>> {
    tracing::debug!("Fetching tables for database '{}'", database_name);
    if let Ok(connection) = db_service
        .get_or_create_connection_by_id(connection_id, Some(database_name))
        .await
    {
        let tables = connection.get_tables(None).await?;
        tracing::debug!(
            "Found {} tables for database '{}': {:?}",
            tables.len(),
            database_name,
            tables
        );
        Ok(tables)
    } else {
        tracing::warn!(
            "Failed to get connection for fetching tables from database '{}'",
            database_name
        );
        Ok(Vec::new())
    }
}

/// Fetch column names for a specific table using the DbService
pub async fn fetch_columns(
    db_service: &dyn DatabaseServiceTrait,
    connection_id: i64,
    table_name: &str,
    database_name: &str,
) -> Result<Vec<String>> {
    tracing::debug!(
        "Fetching columns for table '{}', database '{}'",
        table_name,
        database_name
    );
    if let Ok(connection) = db_service
        .get_or_create_connection_by_id(connection_id, Some(database_name))
        .await
    {
        let columns = connection.get_columns_for_table(table_name, None).await?;
        let column_names: Vec<String> = columns.into_iter().map(|col| col.name).collect();
        tracing::debug!(
            "Found {} columns for table '{}': {:?}",
            column_names.len(),
            table_name,
            column_names
        );
        Ok(column_names)
    } else {
        tracing::warn!(
            "Failed to get connection for fetching columns from table '{}', database '{}'",
            table_name,
            database_name
        );
        Ok(Vec::new())
    }
}
