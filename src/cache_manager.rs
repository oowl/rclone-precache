use crate::models::{CacheProgress, GlobalProgress};
use crate::storage_backend::StorageBackend;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

pub struct CacheManager {
    chunk_size: usize,
    storage_backend: Arc<dyn StorageBackend>,
    active: Arc<RwLock<HashMap<String, Arc<RwLock<CacheProgress>>>>>,
}

impl CacheManager {
    pub fn new(chunk_size: usize, storage_backend: Arc<dyn StorageBackend>) -> Self {
        Self {
            chunk_size,
            storage_backend,
            active: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start_progress(
        &self,
        source_path: String,
        cache_path: PathBuf,
        threads: Option<usize>,
    ) -> Result<(), std::io::Error> {
        let thread_count = threads.unwrap_or(1);

        // Get file size from storage backend
        let total_size = if let Ok(size) = self.storage_backend.file_size(&source_path).await {
            size
        } else {
            // It's a directory, calculate size recursively
            self.calculate_directory_size(&source_path).await
        };

        let progress = Arc::new(RwLock::new(CacheProgress {
            current_speed: 0.0,
            total_bytes_read: 0,
            total_size,
            is_complete: false,
            cached_size: 0,
            speed_windows: Vec::new(),
        }));

        self.active
            .write()
            .insert(source_path.clone(), Arc::clone(&progress));

        let progress_clone = Arc::clone(&progress);
        let source_path_clone = source_path.clone();
        let chunk_size = self.chunk_size;
        let active_clone = Arc::clone(&self.active);
        let storage_backend = Arc::clone(&self.storage_backend);

        tokio::spawn(async move {
            if let Err(e) = Self::cache_file(
                &storage_backend,
                &source_path_clone,
                cache_path,
                &progress_clone,
                chunk_size,
                thread_count,
            )
            .await
            {
                tracing::error!("Error caching file {:?}: {}", source_path_clone, e);
            }

            if let Some(progress) = active_clone.write().remove(&source_path_clone) {
                progress.write().is_complete = true;
            }
        });

        Ok(())
    }

    pub async fn get_progress(&self, path: &str) -> Option<Arc<RwLock<CacheProgress>>> {
        self.active.read().get(path).cloned()
    }

    pub async fn get_global_progress(&self) -> GlobalProgress {
        let active = self.active.read();
        let mut total_speed = 0.0;
        let mut total_bytes = 0;
        let mut total_size = 0;
        let active_jobs = active.len() as i32;

        for progress in active.values() {
            let progress = progress.read();
            total_speed += progress.current_speed;
            total_bytes += progress.total_bytes_read;
            total_size += progress.total_size;
        }

        GlobalProgress {
            total_speed,
            overall_percent: if total_size > 0 {
                (total_bytes as f64 / total_size as f64) * 100.0
            } else {
                0.0
            },
            active_jobs,
            cached_size: total_bytes,
        }
    }

    fn calculate_directory_size<'a>(
        &'a self,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = i64> + Send + 'a>> {
        Box::pin(async move {
            let mut total_size = 0i64;

            if let Ok(entries) = self.storage_backend.list_dir(path).await {
                for entry in entries {
                    if entry.is_dir {
                        total_size += self.calculate_directory_size(&entry.path).await;
                    } else if let Some(size) = entry.size {
                        total_size += size;
                    }
                }
            }

            total_size
        })
    }

    async fn cache_file(
        storage_backend: &Arc<dyn StorageBackend>,
        source_path: &str,
        cache_path: PathBuf,
        progress: &Arc<RwLock<CacheProgress>>,
        chunk_size: usize,
        threads: usize,
    ) -> Result<(), std::io::Error> {
        Self::cache_file_inner(storage_backend, source_path, cache_path, progress, chunk_size, threads).await
    }

    fn cache_file_inner<'a>(
        storage_backend: &'a Arc<dyn StorageBackend>,
        source_path: &'a str,
        cache_path: PathBuf,
        progress: &'a Arc<RwLock<CacheProgress>>,
        chunk_size: usize,
        in_threads: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), std::io::Error>> + Send + 'a>> {
        Box::pin(async move {
            // Check if it's a directory
            let entry = storage_backend.metadata(source_path).await?;
            
            if entry.is_dir {
                // Create cache directory if it doesn't exist
                tokio::fs::create_dir_all(&cache_path).await?;
                
                // Recursively cache all files in directory
                let entries = storage_backend.list_dir(source_path).await?;
                for entry in entries {
                    let sub_cache_path = cache_path.join(&entry.name);
                    Self::cache_file(storage_backend, &entry.path, sub_cache_path, progress, chunk_size, in_threads).await?;
                }
                return Ok(());
            }

            // For regular files, read from storage backend and write to cache
            // Create parent directory if it doesn't exist
            if let Some(parent) = cache_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            
            let mut reader = storage_backend.open_file(source_path).await?;
            let mut writer = tokio::fs::File::create(&cache_path).await?;
            
            let mut buffer = vec![0u8; chunk_size];
            let mut total_bytes_read = 0i64;
            let mut last_update = std::time::SystemTime::now();

            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        // Write to cache file
                        tokio::io::AsyncWriteExt::write_all(&mut writer, &buffer[..n]).await?;

                        total_bytes_read += n as i64;
                        let now = std::time::SystemTime::now();

                        let time_passed = now
                            .duration_since(last_update)
                            .unwrap_or(std::time::Duration::from_secs(0));
                        if time_passed >= std::time::Duration::from_secs(1) {
                            let mut progress_guard = progress.write();
                            progress_guard.total_bytes_read += total_bytes_read;
                            progress_guard.update_speed(total_bytes_read, now);
                            progress_guard.cached_size += total_bytes_read;
                            total_bytes_read = 0;
                            last_update = now;
                        }
                    }
                    Err(e) => return Err(e),
                }
            }

            // Flush and sync the file to disk
            tokio::io::AsyncWriteExt::flush(&mut writer).await?;
            writer.sync_all().await?;

            if total_bytes_read > 0 {
                let mut progress_guard = progress.write();
                progress_guard.total_bytes_read += total_bytes_read;
                progress_guard.update_speed(total_bytes_read, std::time::SystemTime::now());
                progress_guard.cached_size += total_bytes_read;
            }

            Ok(())
        })
    }
}
