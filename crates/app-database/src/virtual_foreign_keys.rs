//! Foreign keys the user declared in Blanco for tables and views that have
//! none on the server. They are never sent to the server, and go with their
//! connection when it is deleted.

use sqlx::Row;

use crate::AppDatabase;

/// One virtual foreign key of a table or view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualForeignKey {
    /// The row id, 0 for a key not saved yet.
    pub id: i64,
    pub connection_id: i64,
    pub database_name: String,
    /// The schema of the table, `None` on backends without schemas.
    pub schema_name: Option<String>,
    pub table_name: String,
    pub name: String,
    /// Referencing columns, paired by position with `referenced_columns`.
    pub columns: Vec<String>,
    /// The referenced table's schema, `None` for the table's own.
    pub referenced_schema: Option<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}

/// Schemas and schema-less tables share one column, where NULL would not
/// take part in the unique key.
fn stored_schema(schema: Option<&str>) -> &str {
    schema.unwrap_or_default()
}

fn loaded_schema(schema: String) -> Option<String> {
    (!schema.is_empty()).then_some(schema)
}

fn columns_json(columns: &[String]) -> Result<String, sqlx::Error> {
    serde_json::to_string(columns).map_err(|error| sqlx::Error::Encode(Box::new(error)))
}

fn columns_from_json(text: &str) -> Result<Vec<String>, sqlx::Error> {
    serde_json::from_str(text).map_err(|error| sqlx::Error::Decode(Box::new(error)))
}

impl AppDatabase {
    /// The virtual foreign keys of every table in one schema of a
    /// connection's database, by table and name.
    pub async fn load_virtual_foreign_keys(
        &self,
        connection_id: i64,
        database_name: &str,
        schema_name: Option<&str>,
    ) -> Result<Vec<VirtualForeignKey>, sqlx::Error> {
        let pool = self.pool();
        let database_name = database_name.to_string();
        let schema_name = stored_schema(schema_name).to_string();
        self.run(async move {
            let rows = sqlx::query(
                "SELECT id, table_name, name, columns, referenced_schema, referenced_table, referenced_columns
                 FROM virtual_foreign_keys
                 WHERE connection_id = ? AND database_name = ? AND schema_name = ?
                 ORDER BY table_name, name",
            )
            .bind(connection_id)
            .bind(&database_name)
            .bind(&schema_name)
            .fetch_all(&pool)
            .await?;
            rows.into_iter()
                .map(|row| {
                    Ok(VirtualForeignKey {
                        id: row.get(0),
                        connection_id,
                        database_name: database_name.clone(),
                        schema_name: loaded_schema(schema_name.clone()),
                        table_name: row.get(1),
                        name: row.get(2),
                        columns: columns_from_json(&row.get::<String, _>(3))?,
                        referenced_schema: loaded_schema(row.get(4)),
                        referenced_table: row.get(5),
                        referenced_columns: columns_from_json(&row.get::<String, _>(6))?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Save a virtual foreign key, new when its id is 0, and return its id.
    /// A table holds one key of each name.
    pub async fn save_virtual_foreign_key(
        &self,
        key: &VirtualForeignKey,
    ) -> Result<i64, sqlx::Error> {
        let pool = self.pool();
        let key = key.clone();
        self.run(async move {
            let columns = columns_json(&key.columns)?;
            let referenced_columns = columns_json(&key.referenced_columns)?;
            let schema_name = stored_schema(key.schema_name.as_deref());
            let referenced_schema = stored_schema(key.referenced_schema.as_deref());
            if key.id == 0 {
                let result = sqlx::query(
                    "INSERT INTO virtual_foreign_keys
                     (connection_id, database_name, schema_name, table_name, name, columns,
                      referenced_schema, referenced_table, referenced_columns, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(key.connection_id)
                .bind(&key.database_name)
                .bind(schema_name)
                .bind(&key.table_name)
                .bind(&key.name)
                .bind(&columns)
                .bind(referenced_schema)
                .bind(&key.referenced_table)
                .bind(&referenced_columns)
                .bind(chrono::Utc::now().timestamp())
                .execute(&pool)
                .await?;
                Ok(result.last_insert_rowid())
            } else {
                sqlx::query(
                    "UPDATE virtual_foreign_keys
                     SET name = ?, columns = ?, referenced_schema = ?, referenced_table = ?,
                         referenced_columns = ?
                     WHERE id = ?",
                )
                .bind(&key.name)
                .bind(&columns)
                .bind(referenced_schema)
                .bind(&key.referenced_table)
                .bind(&referenced_columns)
                .bind(key.id)
                .execute(&pool)
                .await?;
                Ok(key.id)
            }
        })
        .await
    }

    pub async fn delete_virtual_foreign_key(&self, id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM virtual_foreign_keys WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConnectionData;

    #[tokio::test]
    async fn virtual_keys_round_trip_and_go_with_their_connection() {
        let db = AppDatabase::new_in_memory(tokio::runtime::Handle::current())
            .await
            .expect("in-memory database");
        let connection_id = db
            .save_connection(&ConnectionData::new_sqlite(
                "shop".into(),
                "/tmp/shop.db".into(),
            ))
            .await
            .expect("connection");
        let mut key = VirtualForeignKey {
            id: 0,
            connection_id,
            database_name: "main".to_string(),
            schema_name: None,
            table_name: "order_report".to_string(),
            name: "customer".to_string(),
            columns: vec!["customer_id".to_string()],
            referenced_schema: None,
            referenced_table: "customers".to_string(),
            referenced_columns: vec!["id".to_string()],
        };
        key.id = db.save_virtual_foreign_key(&key).await.expect("insert");
        assert!(key.id > 0);
        assert_eq!(
            db.load_virtual_foreign_keys(connection_id, "main", None)
                .await
                .expect("load"),
            [key.clone()]
        );
        // Another schema or database sees none of them.
        assert!(
            db.load_virtual_foreign_keys(connection_id, "main", Some("other"))
                .await
                .expect("load")
                .is_empty()
        );

        key.referenced_columns = vec!["customer_id".to_string()];
        db.save_virtual_foreign_key(&key).await.expect("update");
        let loaded = db
            .load_virtual_foreign_keys(connection_id, "main", None)
            .await
            .expect("load");
        assert_eq!(loaded[0].referenced_columns, ["customer_id"]);

        // A second key of the same name on the table is refused.
        let duplicate = VirtualForeignKey {
            id: 0,
            ..key.clone()
        };
        assert!(db.save_virtual_foreign_key(&duplicate).await.is_err());

        db.delete_connection(connection_id).await.expect("delete");
        assert!(
            db.load_virtual_foreign_keys(connection_id, "main", None)
                .await
                .expect("load")
                .is_empty()
        );
    }
}
