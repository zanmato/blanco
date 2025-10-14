use std::path::{Path, PathBuf};
use anyhow::Result;
use tokio::fs;

/// Manages query files stored on disk for LSP integration
pub struct QueryFileManager {
    queries_dir: PathBuf,
}

impl QueryFileManager {
    /// Create a new query file manager
    pub fn new() -> Result<Self> {
        let queries_dir = Self::queries_dir()?;
        std::fs::create_dir_all(&queries_dir)?;

        Ok(Self { queries_dir })
    }

    /// Get the queries directory path
    fn queries_dir() -> Result<PathBuf> {
        let mut path = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find data directory"))?;
        path.push("blanco");
        path.push("queries");
        Ok(path)
    }

    /// Create a new query file and return its path
    pub async fn create_query_file(&self, tab_id: i64, content: &str) -> Result<PathBuf> {
        let file_path = self.query_file_path(tab_id);
        fs::write(&file_path, content).await?;
        Ok(file_path)
    }

    /// Read query file content
    pub async fn read_query_file(&self, tab_id: i64) -> Result<String> {
        let file_path = self.query_file_path(tab_id);
        let content = fs::read_to_string(&file_path).await?;
        Ok(content)
    }

    /// Update query file content
    pub async fn update_query_file(&self, tab_id: i64, content: &str) -> Result<()> {
        let file_path = self.query_file_path(tab_id);
        fs::write(&file_path, content).await?;
        Ok(())
    }

    /// Delete query file
    pub async fn delete_query_file(&self, tab_id: i64) -> Result<()> {
        let file_path = self.query_file_path(tab_id);
        if file_path.exists() {
            fs::remove_file(&file_path).await?;
        }
        Ok(())
    }

    /// Get the file path for a query tab
    fn query_file_path(&self, tab_id: i64) -> PathBuf {
        self.queries_dir.join(format!("query_{}.sql", tab_id))
    }

    /// Get the file URI for a query tab (for LSP)
    pub fn query_file_uri(&self, tab_id: i64) -> String {
        let path = self.query_file_path(tab_id);
        format!("file://{}", path.display())
    }

    /// Check if a query file exists
    pub fn query_file_exists(&self, tab_id: i64) -> bool {
        self.query_file_path(tab_id).exists()
    }

    /// Get the queries directory path
    pub fn queries_directory(&self) -> &Path {
        &self.queries_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_query_file_creation() {
        let manager = QueryFileManager::new().unwrap();
        let content = "SELECT * FROM users;";

        let file_path = manager.create_query_file(999, content).await.unwrap();
        assert!(file_path.exists());

        let read_content = manager.read_query_file(999).await.unwrap();
        assert_eq!(read_content, content);

        // Cleanup
        manager.delete_query_file(999).await.unwrap();
    }

    #[tokio::test]
    async fn test_query_file_update() {
        let manager = QueryFileManager::new().unwrap();
        let initial_content = "SELECT 1;";
        let updated_content = "SELECT 2;";

        manager.create_query_file(998, initial_content).await.unwrap();
        manager.update_query_file(998, updated_content).await.unwrap();

        let read_content = manager.read_query_file(998).await.unwrap();
        assert_eq!(read_content, updated_content);

        // Cleanup
        manager.delete_query_file(998).await.unwrap();
    }
}
