use sqlx::Row;

use crate::{AppDatabase, EnvironmentType};
use blanco_core::EnvironmentTag;

impl AppDatabase {
    /// The tags the user defined. The built-in ones are not stored.
    pub async fn load_environment_tags(&self) -> Result<Vec<EnvironmentTag>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                "SELECT id, name, color, confirm_writes FROM environment_tags ORDER BY name",
            )
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(|row| EnvironmentTag {
                    id: EnvironmentType::from_i32(row.get::<i64, _>(0) as i32),
                    name: row.get(1),
                    color: row.get(2),
                    confirms_writes: row.get::<i64, _>(3) != 0,
                })
                .collect())
        })
        .await
    }

    /// Save a user-defined tag. A tag with a built-in id is new and gets the
    /// next free id from [`EnvironmentType::FIRST_CUSTOM_ID`] on.
    pub async fn save_environment_tag(
        &self,
        tag: &EnvironmentTag,
    ) -> Result<EnvironmentType, sqlx::Error> {
        let pool = self.pool();
        let tag = tag.clone();
        self.run(async move {
            let id = if tag.id.is_built_in() {
                let row = sqlx::query("SELECT MAX(id) FROM environment_tags")
                    .fetch_one(&pool)
                    .await?;
                let highest = row.get::<Option<i64>, _>(0).unwrap_or(0) as i32;
                highest.max(EnvironmentType::FIRST_CUSTOM_ID - 1) + 1
            } else {
                tag.id.to_i32()
            };
            sqlx::query(
                "INSERT OR REPLACE INTO environment_tags (id, name, color, confirm_writes) VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(&tag.name)
            .bind(&tag.color)
            .bind(tag.confirms_writes as i64)
            .execute(&pool)
            .await?;
            Ok(EnvironmentType::from_i32(id))
        })
        .await
    }

    /// Delete a user-defined tag. Connections that carried it fall back to
    /// DEV.
    pub async fn delete_environment_tag(&self, id: EnvironmentType) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let mut transaction = pool.begin().await?;
            sqlx::query("UPDATE connections SET environment_type = ? WHERE environment_type = ?")
                .bind(EnvironmentType::DEV.to_i32())
                .bind(id.to_i32())
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM environment_tags WHERE id = ?")
                .bind(id.to_i32())
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConnectionData;

    #[tokio::test]
    async fn test_custom_tags_round_trip_and_deleting_one_resets_its_connections() {
        let db = AppDatabase::new_in_memory(tokio::runtime::Handle::current())
            .await
            .expect("in-memory database");
        let new_tag = |name: &str| EnvironmentTag {
            id: EnvironmentType::DEV,
            name: name.to_string(),
            color: "magenta".to_string(),
            confirms_writes: true,
        };
        let qa = db.save_environment_tag(&new_tag("QA")).await.unwrap();
        let demo = db.save_environment_tag(&new_tag("DEMO")).await.unwrap();
        assert_eq!(qa.to_i32(), EnvironmentType::FIRST_CUSTOM_ID);
        assert_eq!(demo.to_i32(), EnvironmentType::FIRST_CUSTOM_ID + 1);

        let mut connection = ConnectionData::new_sqlite("qa".into(), "/tmp/qa.db".into());
        connection.environment_type = qa;
        db.save_connection(&connection).await.unwrap();

        let tags = db.load_environment_tags().await.unwrap();
        assert_eq!(
            tags.iter().map(|tag| tag.name.as_str()).collect::<Vec<_>>(),
            ["DEMO", "QA"]
        );
        assert!(tags.iter().all(|tag| tag.confirms_writes));

        db.delete_environment_tag(qa).await.unwrap();
        assert_eq!(db.load_environment_tags().await.unwrap().len(), 1);
        let connections = db.load_connections().await.unwrap();
        assert_eq!(connections[0].environment_type, EnvironmentType::DEV);
    }
}
