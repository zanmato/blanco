//! Binary downloader for PostgreSQL language server
//! 
//! This module handles downloading the PostgreSQL language server binary from GitHub releases,
//! with support for different architectures and caching.

use anyhow::Result;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::Archive;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tracing::{debug, error, info, warn};

/// Supported architectures for the PostgreSQL language server
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Architecture {
    #[serde(rename = "x86_64-unknown-linux-gnu")]
    X86_64UnknownLinuxGnu,
    #[serde(rename = "x86_64-apple-darwin")]
    X86_64AppleDarwin,
    #[serde(rename = "aarch64-apple-darwin")]
    Aarch64AppleDarwin,
    #[serde(rename = "x86_64-pc-windows-msvc")]
    X86_64PcWindowsMsvc,
}

impl std::fmt::Display for Architecture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Architecture::X86_64UnknownLinuxGnu => write!(f, "x86_64-unknown-linux-gnu"),
            Architecture::X86_64AppleDarwin => write!(f, "x86_64-apple-darwin"),
            Architecture::Aarch64AppleDarwin => write!(f, "aarch64-apple-darwin"),
            Architecture::X86_64PcWindowsMsvc => write!(f, "x86_64-pc-windows-msvc"),
        }
    }
}

impl Architecture {
    /// Detect the current system architecture
    pub fn current() -> Result<Self> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        match (os, arch) {
            ("linux", "x86_64") => Ok(Architecture::X86_64UnknownLinuxGnu),
            ("macos", "x86_64") => Ok(Architecture::X86_64AppleDarwin),
            ("macos", "aarch64") => Ok(Architecture::Aarch64AppleDarwin),
            ("windows", "x86_64") => Ok(Architecture::X86_64PcWindowsMsvc),
            _ => Err(anyhow::anyhow!("Unsupported architecture: {}-{}", os, arch)),
        }
    }

    /// Get the binary name for this architecture
    pub fn binary_name(&self) -> &'static str {
        match self {
            Architecture::X86_64UnknownLinuxGnu => "postgrestools_x86_64-unknown-linux-gnu",
            Architecture::X86_64AppleDarwin => "postgrestools_x86_64-apple-darwin",
            Architecture::Aarch64AppleDarwin => "postgrestools_aarch64-apple-darwin",
            Architecture::X86_64PcWindowsMsvc => "postgrestools_x86_64-pc-windows-msvc.exe",
        }
    }

    /// Get the file name for the release asset
    pub fn asset_name(&self) -> &'static str {
        match self {
            Architecture::X86_64UnknownLinuxGnu => "postgrestools-x86_64-unknown-linux-gnu.tar.gz",
            Architecture::X86_64AppleDarwin => "postgrestools-x86_64-apple-darwin.tar.gz",
            Architecture::Aarch64AppleDarwin => "postgrestools-aarch64-apple-darwin.tar.gz",
            Architecture::X86_64PcWindowsMsvc => "postgrestools-x86_64-pc-windows-msvc.tar.gz",
        }
    }
}

/// Download errors
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported architecture: {0}")]
    UnsupportedArchitecture(String),
    #[error("Release not found for architecture: {0}")]
    ReleaseNotFound(Architecture),
    #[error("Asset not found in release: {0}")]
    AssetNotFound(String),
    #[error("Cache directory error: {0}")]
    CacheDir(String),
}

/// GitHub release information
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

/// GitHub release asset
#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Binary downloader for PostgreSQL language server
pub struct BinaryDownloader {
    client: reqwest::Client,
    cache_dir: PathBuf,
    architecture: Architecture,
}

impl BinaryDownloader {
    /// Create a new binary downloader
    pub fn new() -> Result<Self, DownloadError> {
        let architecture = Architecture::current()
            .map_err(|e| DownloadError::UnsupportedArchitecture(e.to_string()))?;

        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| DownloadError::CacheDir("Could not find cache directory".to_string()))?
            .join("blanco")
            .join("postgres-lsp");

        Ok(Self {
            client: reqwest::Client::new(),
            cache_dir,
            architecture,
        })
    }

    /// Ensure the binary is available, downloading if necessary
    pub async fn ensure_binary(&self) -> Result<PathBuf, DownloadError> {
        // Check if binary already exists locally
        if let Some(binary_path) = self.check_local_binary().await? {
            info!("Using existing PostgreSQL LSP binary: {:?}", binary_path);
            return Ok(binary_path);
        }

        // Download from GitHub releases
        self.download_binary().await
    }

    /// Check if binary exists locally (in cache or project directory)
    async fn check_local_binary(&self) -> Result<Option<PathBuf>, DownloadError> {
        // First check cache directory
        let cached_binary = self.cache_dir.join(self.architecture.binary_name());
        if cached_binary.exists() && self.is_binary_executable(&cached_binary).await? {
            return Ok(Some(cached_binary));
        }

        // Check project directory (for development)
        let project_binary = PathBuf::from("postgrestools_x86_64-unknown-linux-gnu");
        if project_binary.exists() && self.is_binary_executable(&project_binary).await? {
            return Ok(Some(project_binary));
        }

        Ok(None)
    }

    /// Check if a binary is executable
    async fn is_binary_executable(&self, path: &Path) -> Result<bool, DownloadError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(path).await?;
            Ok(metadata.permissions().mode() & 0o111 != 0)
        }
        
        #[cfg(windows)]
        {
            // On Windows, just check if the file exists and has .exe extension
            Ok(path.extension().map_or(false, |ext| ext == "exe"))
        }
    }

    /// Download the binary from GitHub releases
    async fn download_binary(&self) -> Result<PathBuf, DownloadError> {
        info!("Downloading PostgreSQL LSP binary for architecture: {:?}", self.architecture);

        // Get latest release information
        let release = self.get_latest_release().await?;
        debug!("Found latest release: {}", release.tag_name);

        // Find the appropriate asset
        let asset = release.assets.into_iter()
            .find(|a| a.name == self.architecture.asset_name())
            .ok_or_else(|| DownloadError::AssetNotFound(self.architecture.asset_name().to_string()))?;

        info!("Downloading asset: {}", asset.name);

        // Download the asset
        let response = self.client.get(&asset.browser_download_url).send().await?;
        if !response.status().is_success() {
            return Err(DownloadError::Http(reqwest::Error::from(
                response.error_for_status().unwrap_err()
            )));
        }

        let bytes = response.bytes().await?;

        // Extract the tar.gz archive
        self.extract_archive(&bytes).await
    }

    /// Get the latest release from GitHub
    async fn get_latest_release(&self) -> Result<Release, DownloadError> {
        let url = "https://api.github.com/repos/supabase-community/postgres-language-server/releases/latest";
        
        let response = self.client
            .get(url)
            .header("User-Agent", "blanco-sql-editor")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DownloadError::Http(reqwest::Error::from(
                response.error_for_status().unwrap_err()
            )));
        }

        let release: Release = response.json().await?;
        Ok(release)
    }

    /// Extract the tar.gz archive and return the binary path
    async fn extract_archive(&self, bytes: &[u8]) -> Result<PathBuf, DownloadError> {
        // Create cache directory if it doesn't exist
        fs::create_dir_all(&self.cache_dir).await?;

        // Decode gzip
        let decoder = GzDecoder::new(bytes);
        let mut archive = Archive::new(decoder);

        // Extract to cache directory
        archive.unpack(&self.cache_dir)?;

        let binary_path = self.cache_dir.join(self.architecture.binary_name());

        // Make executable on Unix systems
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&binary_path).await?.permissions();
            perms.set_mode(perms.mode() | 0o755);
            fs::set_permissions(&binary_path, perms).await?;
        }

        info!("Successfully extracted PostgreSQL LSP binary to: {:?}", binary_path);
        Ok(binary_path)
    }

    /// Get the cache directory
    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache_dir
    }

    /// Get the current architecture
    pub fn architecture(&self) -> &Architecture {
        &self.architecture
    }

    /// Clean up cached binaries
    pub async fn cleanup_cache(&self) -> Result<(), DownloadError> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir).await?;
            info!("Cleaned up PostgreSQL LSP cache");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_architecture_detection() {
        // This test will pass on any supported architecture
        let arch = Architecture::current();
        assert!(arch.is_ok());
    }

    #[test]
    fn test_architecture_properties() {
        let arch = Architecture::current().unwrap();
        
        // Check that binary name is not empty
        assert!(!arch.binary_name().is_empty());
        
        // Check that asset name is not empty
        assert!(!arch.asset_name().is_empty());
        
        // Check that asset name ends with .tar.gz
        assert!(arch.asset_name().ends_with(".tar.gz"));
    }

    #[tokio::test]
    async fn test_downloader_creation() {
        let downloader = BinaryDownloader::new();
        assert!(downloader.is_ok());
    }
}