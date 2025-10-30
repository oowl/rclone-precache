use crate::storage_backend::{FileEntry, StorageBackend};
use async_trait::async_trait;
use std::io;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncRead;

#[derive(Clone)]
pub struct LocalFileSystem {
    base_path: PathBuf,
}

impl LocalFileSystem {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let path = if path.starts_with('/') {
            &path[1..]
        } else {
            path
        };
        self.base_path.join(path)
    }
}

#[async_trait]
impl StorageBackend for LocalFileSystem {
    async fn list_dir(&self, path: &str) -> io::Result<Vec<FileEntry>> {
        let full_path = self.resolve_path(path);
        let mut entries = Vec::new();

        let mut dir = tokio::fs::read_dir(&full_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy().into_owned();

            let size = if metadata.is_file() {
                Some(metadata.len() as i64)
            } else {
                None
            };
            
            let entry_path = if path.is_empty() || path == "/" {
                format!("/{}", file_name)
            } else {
                format!("{}/{}", path, file_name)
            };

            entries.push(FileEntry {
                name: file_name,
                path: entry_path,
                is_dir: metadata.is_dir(),
                size,
                modified_time: metadata
                    .accessed()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                    .as_secs_f64(),
            });
        }

        Ok(entries)
    }

    async fn metadata(&self, path: &str) -> io::Result<FileEntry> {
        let full_path = self.resolve_path(path);
        let metadata = tokio::fs::metadata(&full_path).await?;
        
        let file_name = full_path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Invalid path"))?
            .to_string_lossy()
            .into_owned();

        let size = if metadata.is_file() {
            Some(metadata.len() as i64)
        } else {
            None
        };

        Ok(FileEntry {
            name: file_name,
            path: path.to_string(),
            is_dir: metadata.is_dir(),
            size,
            modified_time: metadata
                .accessed()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                .as_secs_f64(),
        })
    }

    async fn open_file(&self, path: &str) -> io::Result<Box<dyn AsyncRead + Unpin + Send>> {
        let full_path = self.resolve_path(path);
        let file = File::open(full_path).await?;
        Ok(Box::new(file))
    }

    async fn file_size(&self, path: &str) -> io::Result<i64> {
        let full_path = self.resolve_path(path);
        let metadata = tokio::fs::metadata(full_path).await?;
        Ok(metadata.len() as i64)
    }
}
