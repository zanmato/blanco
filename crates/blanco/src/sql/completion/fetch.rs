use anyhow::Result;
use blanco_core::connection_trait::QueryableEntity;
use database::DatabaseServiceTrait;

/// Fetch all queryable entities (tables, views, materialized views) using the DbService
pub async fn fetch_queryable_entities(
    db_service: &dyn DatabaseServiceTrait,
    connection_id: i64,
    database_name: &str,
) -> Result<Vec<QueryableEntity>> {
    tracing::debug!(
        "Fetching queryable entities for database '{}'",
        database_name
    );
    if let Ok(connection) = db_service
        .get_or_create_connection_by_id(connection_id, Some(database_name))
        .await
    {
        let entities = connection.get_queryable_entities(None).await?;
        tracing::debug!(
            "Found {} entities for database '{}'",
            entities.len(),
            database_name
        );
        Ok(entities)
    } else {
        tracing::warn!(
            "Failed to get connection for fetching entities from database '{}'",
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
