use crate::cache_manager::CacheManager;
use crate::directory_sizer::DirectorySizer;
use crate::models::{FileInfo, GlobalProgress};
use crate::storage_backend::StorageBackend;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct Server {
    cache_manager: Arc<CacheManager>,
    directory_sizer: Arc<DirectorySizer>,
    storage_backend: Arc<dyn StorageBackend>,
    cache_path: PathBuf,
    cache_threads: usize,
}

impl Server {
    pub fn new(
        storage_backend: Box<dyn StorageBackend>,
        cache_path: PathBuf,
        chunk_size: usize,
        cache_threads: usize,
    ) -> Self {
        let storage_arc = Arc::from(storage_backend);
        Self {
            cache_manager: Arc::new(CacheManager::new(chunk_size, Arc::clone(&storage_arc))),
            directory_sizer: Arc::new(DirectorySizer::new()),
            storage_backend: storage_arc,
            cache_path,
            cache_threads,
        }
    }

    pub async fn browse(&self, path: &str) -> Result<Vec<FileInfo>, std::io::Error> {
        let entries = self.storage_backend.list_dir(path).await?;
        println!("Browsing path: {}, found entries: {:?}", path, entries);
        let mut file_infos = Vec::new();
        for entry in entries {
            let cached_size = if entry.is_dir {
                // For directories, calculate cache size from cache path
                let rel_path = if path.starts_with('/') {
                    &path[1..]
                } else {
                    path
                };
                let cache_dir_path = self.cache_path.join(rel_path).join(&entry.name);
                self.directory_sizer
                    .get_allocated_size(&cache_dir_path)
                    .await
                    .abs()
            } else {
                // For files, check cache
                let rel_path = if path.starts_with('/') {
                    &path[1..]
                } else {
                    path
                };
                let cache_file_path = self.cache_path.join(rel_path).join(&entry.name);
                self.directory_sizer
                    .get_allocated_size(&cache_file_path)
                    .await
                    .abs()
            };

            file_infos.push(FileInfo {
                name: entry.name,
                path: entry.path,
                is_dir: entry.is_dir,
                size: entry.size,
                created_time: entry.modified_time,
                cached_size,
            });
        }

        Ok(file_infos)
    }

    pub async fn start_precache(&self, path: &str) -> Result<(), std::io::Error> {
        let path2 = if path.starts_with("/") {
            &path[1..]
        } else {
            path
        };
        let cache_path = self.cache_path.join(path2);

        self.cache_manager
            .start_progress(path.to_string(), cache_path, Some(self.cache_threads))
            .await?;
        Ok(())
    }

    pub async fn get_cache_progress(&self, path: &str) -> Result<GlobalProgress, std::io::Error> {
        if path == "/" || path == "" {
            return Ok(self.cache_manager.get_global_progress().await);
        }

        match self.cache_manager.get_progress(path).await {
            Some(progress) => Ok(GlobalProgress {
                total_speed: progress.read().current_speed,
                overall_percent: (progress.read().total_bytes_read as f64
                    / progress.read().total_size as f64)
                    * 100.0,
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
