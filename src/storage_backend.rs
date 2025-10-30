use async_trait::async_trait;
use serde::Serialize;
use std::io;
use tokio::io::AsyncRead;

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<i64>,
    pub modified_time: f64,
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// List files and directories in the given path
    async fn list_dir(&self, path: &str) -> io::Result<Vec<FileEntry>>;
    
    /// Get metadata for a specific file or directory
    async fn metadata(&self, path: &str) -> io::Result<FileEntry>;
    
    /// Open a file for reading, returns a stream
    async fn open_file(&self, path: &str) -> io::Result<Box<dyn AsyncRead + Unpin + Send>>;
    
    /// Get the size of a file
    async fn file_size(&self, path: &str) -> io::Result<i64>;
}
