use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use crate::models::{FileInfo, GlobalProgress};
use crate::cache_manager::CacheManager;
use crate::directory_sizer::DirectorySizer;

#[derive(Clone)]
pub struct Server {
    cache_manager: Arc<CacheManager>,
    directory_sizer: Arc<DirectorySizer>,
    mount_path: PathBuf,
    cache_path: PathBuf,
}

impl Server {
    pub fn new(mount_path: PathBuf, cache_path: PathBuf, chunk_size: usize) -> Self {
        Self {
            cache_manager: Arc::new(CacheManager::new(chunk_size)),
            directory_sizer: Arc::new(DirectorySizer::new()),
            mount_path,
            cache_path,
        }
    }

    pub async fn browse(&self, path: &str) -> Result<Vec<FileInfo>, std::io::Error> {
        // if path starts with /, add a .
        let path2 = if path.starts_with("/") {
            &path[1..]
        } else {
            path
        };
        let full_path = self.mount_path.join(path2);
        let mut entries = Vec::new();
        
        let mut dir = tokio::fs::read_dir(full_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy().into_owned();
            
            let size = if metadata.is_file() {
                Some(metadata.len() as i64)
            } else {
                None
            };

            // get the rel path of entry.path to mount_path
            let entry_path = entry.path();
            let rel_path = entry_path.strip_prefix(&self.mount_path).unwrap();
            
            entries.push(FileInfo {
                name: file_name.clone(),
                path: format!("{}/{}", path, file_name.clone()),
                is_dir: metadata.is_dir(),
                size,
                created_time: metadata.accessed()?.duration_since(std::time::UNIX_EPOCH).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?.as_secs_f64(),
                cached_size: self.directory_sizer.get_allocated_size(&self.cache_path.join(rel_path)).await.abs(),
            });
        }
        
        Ok(entries)
    }

    pub async fn start_precache(&self, path: &str) -> Result<(), std::io::Error> {
        let path2 = if path.starts_with("/") {
            &path[1..]
        } else {
            path
        };
        let source_path = self.mount_path.join(path2);
        let cache_path = self.cache_path.join(path2);
        
        self.cache_manager.start_progress(source_path, cache_path).await?;
        Ok(())
    }

    pub async fn get_cache_progress(&self, path: &str) -> Result<GlobalProgress, std::io::Error> {
        if path == "/" || path == "" {
            return Ok(self.cache_manager.get_global_progress().await);
        }

        let path2 = if path.starts_with("/") {
            &path[1..]
        } else {
            path
        };
        
        let source_path = self.mount_path.join(path2);
        match self.cache_manager.get_progress(&source_path).await {
            Some(progress) => Ok(GlobalProgress {
                total_speed: progress.read().current_speed,
                overall_percent: (progress.read().total_bytes_read as f64 / progress.read().total_size as f64) * 100.0,
                active_jobs: 1,
                cached_size: progress.read().cached_size,
            }),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No active cache operation found",
            )),
        }
    }
}