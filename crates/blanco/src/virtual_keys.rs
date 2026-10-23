//! Virtual foreign keys, declared in Blanco for tables and views the server
//! knows no keys for. They reach the grid and the schema graph as the
//! column's foreign key, as a declared one would.

use app_database::{AppDatabase, VirtualForeignKey};
use blanco_core::{ColumnInfo, ForeignKeyInfo};

/// The virtual keys of one schema, or none when they cannot be read. A
/// failure to read them must not fail what they decorate.
pub async fn load(
    app_database: &AppDatabase,
    connection_id: i64,
    database_name: &str,
    schema_name: Option<&str>,
) -> Vec<VirtualForeignKey> {
    match app_database
        .load_virtual_foreign_keys(connection_id, database_name, schema_name)
        .await
    {
        Ok(keys) => keys,
        Err(error) => {
            tracing::warn!("Reading virtual foreign keys failed: {error:#}");
            Vec::new()
        }
    }
}

/// The table a key references, as a column's foreign key names it.
pub fn referenced_table(key: &VirtualForeignKey) -> String {
    match &key.referenced_schema {
        Some(schema) => format!("{schema}.{}", key.referenced_table),
        None => key.referenced_table.clone(),
    }
}

/// Give the columns of `table` the foreign keys its virtual keys declare.
/// A key the server declares for a column wins.
pub fn merge_into_columns(columns: &mut [ColumnInfo], table: &str, keys: &[VirtualForeignKey]) {
    for key in keys.iter().filter(|key| key.table_name == table) {
        for (name, referenced) in key.columns.iter().zip(&key.referenced_columns) {
            if let Some(column) = columns
                .iter_mut()
                .find(|column| &column.name == name && column.foreign_key.is_none())
            {
                column.foreign_key = Some(ForeignKeyInfo {
                    foreign_table_name: referenced_table(key),
                    foreign_column_name: referenced.clone(),
                    constraint_name: Some(key.name.clone()),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "integer".to_string(),
            is_nullable: true,
            is_primary_key: false,
            default_value: None,
            character_maximum_length: None,
            foreign_key: None,
            comment: None,
            numeric_precision: None,
            numeric_scale: None,
            enum_labels: Vec::new(),
        }
    }

    #[test]
    fn virtual_keys_fill_columns_without_a_declared_key() {
        let key = VirtualForeignKey {
            id: 1,
            connection_id: 1,
            database_name: "shop".to_string(),
            schema_name: Some("sales".to_string()),
            table_name: "order_report".to_string(),
            name: "customer".to_string(),
            columns: vec!["customer_id".to_string(), "region".to_string()],
            referenced_schema: Some("crm".to_string()),
            referenced_table: "customers".to_string(),
            referenced_columns: vec!["id".to_string(), "region".to_string()],
        };
        let mut columns = vec![column("id"), column("customer_id"), column("region")];
        let declared = ForeignKeyInfo {
            foreign_table_name: "regions".to_string(),
            foreign_column_name: "code".to_string(),
            constraint_name: Some("order_report_region_fkey".to_string()),
        };
        columns[2].foreign_key = Some(declared.clone());

        merge_into_columns(&mut columns, "order_report", std::slice::from_ref(&key));
        assert_eq!(columns[0].foreign_key, None);
        assert_eq!(
            columns[1].foreign_key,
            Some(ForeignKeyInfo {
                foreign_table_name: "crm.customers".to_string(),
                foreign_column_name: "id".to_string(),
                constraint_name: Some("customer".to_string()),
            })
        );
        assert_eq!(columns[2].foreign_key, Some(declared));

        // Another table's keys leave the columns alone.
        let mut other = vec![column("customer_id")];
        merge_into_columns(&mut other, "orders", &[key]);
        assert_eq!(other[0].foreign_key, None);
    }
}
