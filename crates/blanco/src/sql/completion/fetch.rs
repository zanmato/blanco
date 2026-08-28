use anyhow::Result;
use blanco_core::{ColumnInfo, FunctionSignatureInfo, QueryableEntity};
use database::DatabaseServiceTrait;

/// True for postgres system schemas that should never be offered as completion
/// namespaces. `get_schemas()` already strips `pg_temp*`/`pg_toast*` but still
/// returns `pg_catalog` and `information_schema`.
fn is_system_schema(name: &str) -> bool {
    name == "information_schema" || name.starts_with("pg_")
}

/// Fetch the user schemas for a connection along with whether the backend
/// supports schemas at all (false for MySQL/SQLite). System schemas are removed.
pub async fn fetch_schemas(
    db_service: &dyn DatabaseServiceTrait,
    connection_id: i64,
    database_name: &str,
) -> Result<(Vec<String>, bool)> {
    let Ok(connection) = db_service
        .get_or_create_connection_by_id(connection_id, Some(database_name))
        .await
    else {
        tracing::warn!(
            "Failed to get connection for fetching schemas from database '{}'",
            database_name
        );
        return Ok((Vec::new(), false));
    };

    if !connection.supports_schemas() {
        return Ok((Vec::new(), false));
    }

    let schemas = connection
        .get_schemas()
        .await?
        .into_iter()
        .filter(|s| !is_system_schema(s))
        .collect();
    Ok((schemas, true))
}

/// Fetch all queryable entities (tables, views, materialized views) for a
/// specific schema using the DbService. `schema = None` lets the backend pick
/// its default (`public` for postgres).
pub async fn fetch_queryable_entities(
    db_service: &dyn DatabaseServiceTrait,
    connection_id: i64,
    database_name: &str,
    schema: Option<&str>,
) -> Result<Vec<QueryableEntity>> {
    tracing::debug!(
        "Fetching queryable entities for database '{}', schema {:?}",
        database_name,
        schema
    );
    if let Ok(connection) = db_service
        .get_or_create_connection_by_id(connection_id, Some(database_name))
        .await
    {
        let entities = connection.get_queryable_entities(schema).await?;
        tracing::debug!(
            "Found {} entities for database '{}', schema {:?}",
            entities.len(),
            database_name,
            schema
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

/// Fetch full column metadata (including foreign keys) for a specific table (in
/// an optional schema) using the DbService.
pub async fn fetch_columns(
    db_service: &dyn DatabaseServiceTrait,
    connection_id: i64,
    table_name: &str,
    database_name: &str,
    schema: Option<&str>,
) -> Result<Vec<ColumnInfo>> {
    tracing::debug!(
        "Fetching columns for table '{}', database '{}', schema {:?}",
        table_name,
        database_name,
        schema
    );
    if let Ok(connection) = db_service
        .get_or_create_connection_by_id(connection_id, Some(database_name))
        .await
    {
        let columns = connection.get_columns_for_table(table_name, schema).await?;
        tracing::debug!("Found {} columns for table '{}'", columns.len(), table_name);
        Ok(columns)
    } else {
        tracing::warn!(
            "Failed to get connection for fetching columns from table '{}', database '{}'",
            table_name,
            database_name
        );
        Ok(Vec::new())
    }
}

/// Fetch the user-defined function signatures of a schema using the DbService.
pub async fn fetch_function_signatures(
    db_service: &dyn DatabaseServiceTrait,
    connection_id: i64,
    database_name: &str,
    schema: Option<&str>,
) -> Result<Vec<FunctionSignatureInfo>> {
    tracing::debug!(
        "Fetching function signatures for database '{}', schema {:?}",
        database_name,
        schema
    );
    if let Ok(connection) = db_service
        .get_or_create_connection_by_id(connection_id, Some(database_name))
        .await
    {
        let functions = connection.list_function_signatures(schema).await?;
        tracing::debug!(
            "Found {} functions for database '{}', schema {:?}",
            functions.len(),
            database_name,
            schema
        );
        Ok(functions)
    } else {
        tracing::warn!(
            "Failed to get connection for fetching functions from database '{}'",
            database_name
        );
        Ok(Vec::new())
    }
}
