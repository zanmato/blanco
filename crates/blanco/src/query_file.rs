#![allow(dead_code)]

use anyhow::Result;
use async_std::fs;
use std::path::{Path, PathBuf};

/// Manages query files stored on disk for LSP integration
pub struct QueryFileManager {
    base_dir: PathBuf,
}

impl QueryFileManager {
    /// Create a new query file manager
    pub fn new() -> Result<Self> {
        let base_dir = Self::base_dir()?;
        std::fs::create_dir_all(&base_dir)?;

        Ok(Self { base_dir })
    }

    /// Get the base blanco directory path
    fn base_dir() -> Result<PathBuf> {
        let mut path =
            dirs::data_dir().ok_or_else(|| anyhow::anyhow!("Could not find data directory"))?;
        path.push("blanco");
        Ok(path)
    }

    /// Sanitize connection name to create a safe directory name
    /// Converts to lowercase, replaces spaces and special chars with hyphens
    pub fn sanitize_connection_name(name: &str) -> String {
        name.to_lowercase()
            .chars()
            .map(|c| match c {
                'a'..='z' | '0'..='9' => c,
                _ => '-',
            })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<&str>>()
            .join("-")
    }

    /// Get the queries directory path for a specific connection
    pub fn queries_dir_for_connection(&self, connection_name: &str) -> Result<PathBuf> {
        let sanitized_name = Self::sanitize_connection_name(connection_name);
        let mut path = self.base_dir.clone();
        path.push(sanitized_name);
        path.push("queries");
        Ok(path)
    }

    /// Get the legacy queries directory (for backward compatibility)
    pub fn legacy_queries_dir(&self) -> PathBuf {
        let mut path = self.base_dir.clone();
        path.push("queries");
        path
    }

    /// Ensure queries directory exists for a connection
    pub fn ensure_connection_dir(&self, connection_name: &str) -> Result<PathBuf> {
        let queries_dir = self.queries_dir_for_connection(connection_name)?;
        std::fs::create_dir_all(&queries_dir)?;
        Ok(queries_dir)
    }

    /// Create a new query file and return its path
    pub async fn create_query_file(
        &self,
        tab_id: i64,
        connection_name: &str,
        content: &str,
    ) -> Result<PathBuf> {
        let file_path = self.query_file_path(tab_id, connection_name);
        // Ensure the connection directory exists
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&file_path, content).await?;
        Ok(file_path)
    }

    /// Read query file content
    pub async fn read_query_file(&self, tab_id: i64, connection_name: &str) -> Result<String> {
        let file_path = self.query_file_path(tab_id, connection_name);
        let content = fs::read_to_string(&file_path).await?;
        Ok(content)
    }

    /// Update query file content
    pub async fn update_query_file(
        &self,
        tab_id: i64,
        connection_name: &str,
        content: &str,
    ) -> Result<()> {
        let file_path = self.query_file_path(tab_id, connection_name);
        // Ensure the connection directory exists
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&file_path, content).await?;
        Ok(())
    }

    /// Delete query file
    pub async fn delete_query_file(&self, tab_id: i64, connection_name: &str) -> Result<()> {
        let file_path = self.query_file_path(tab_id, connection_name);
        if file_path.exists() {
            fs::remove_file(&file_path).await?;
        }
        Ok(())
    }

    /// Get the file path for a query tab
    pub fn query_file_path(&self, tab_id: i64, connection_name: &str) -> PathBuf {
        let queries_dir = self
            .queries_dir_for_connection(connection_name)
            .unwrap_or_else(|_| self.legacy_queries_dir());
        queries_dir.join(format!("query_{}.sql", tab_id))
    }

    /// Get the legacy file path for a query tab (for backward compatibility)
    pub fn legacy_query_file_path(&self, tab_id: i64) -> PathBuf {
        self.legacy_queries_dir()
            .join(format!("query_{}.sql", tab_id))
    }

    /// Migrate a query file from legacy location to connection-specific location
    pub async fn migrate_query_file(&self, tab_id: i64, connection_name: &str) -> Result<bool> {
        let legacy_path = self.legacy_query_file_path(tab_id);
        let new_path = self.query_file_path(tab_id, connection_name);

        if legacy_path.exists() && !new_path.exists() {
            // Ensure the new directory exists
            if let Some(parent) = new_path.parent() {
                fs::create_dir_all(parent).await?;
            }

            // Copy the file to new location
            let content = fs::read_to_string(&legacy_path).await?;
            fs::write(&new_path, content).await?;

            // Optionally remove the old file (commented out for safety)
            // fs::remove_file(&legacy_path).await?;

            return Ok(true);
        }

        Ok(false)
    }

    /// Get the file URI for a query tab (for LSP)
    pub fn query_file_uri(&self, tab_id: i64, connection_name: &str) -> String {
        let path = self.query_file_path(tab_id, connection_name);
        format!("file://{}", path.display())
    }

    /// Check if a query file exists in the new location
    pub fn query_file_exists(&self, tab_id: i64, connection_name: &str) -> bool {
        self.query_file_path(tab_id, connection_name).exists()
    }

    /// Check if a query file exists in the legacy location
    pub fn legacy_query_file_exists(&self, tab_id: i64) -> bool {
        self.legacy_query_file_path(tab_id).exists()
    }

    /// Get the queries directory path for a connection
    pub fn queries_directory(&self, connection_name: &str) -> Result<PathBuf> {
        self.queries_dir_for_connection(connection_name)
    }

    /// Get the base blanco directory path
    pub fn base_directory(&self) -> &Path {
        &self.base_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_name_sanitization() {
        assert_eq!(
            QueryFileManager::sanitize_connection_name("Local PostgreSQL"),
            "local-postgresql"
        );
        assert_eq!(
            QueryFileManager::sanitize_connection_name("Production DB"),
            "production-db"
        );
        assert_eq!(
            QueryFileManager::sanitize_connection_name("Test@Database#123"),
            "test-database-123"
        );
        assert_eq!(
            QueryFileManager::sanitize_connection_name("  spaces  "),
            "spaces"
        );
        assert_eq!(
            QueryFileManager::sanitize_connection_name("Multiple---Dashes"),
            "multiple-dashes"
        );
    }
}
