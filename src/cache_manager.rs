use crate::directory_sizer::DirectorySizer;
use crate::models::{CacheProgress, GlobalProgress};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub struct CacheManager {
    chunk_size: usize,
    directory_sizer: Arc<DirectorySizer>,
    active: Arc<RwLock<HashMap<PathBuf, Arc<RwLock<CacheProgress>>>>>,
}

impl CacheManager {
    pub fn new(chunk_size: usize) -> Self {
        Self {
            chunk_size,
            directory_sizer: Arc::new(DirectorySizer::new()),
            active: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start_progress(
        &self,
        source_path: PathBuf,
        _: PathBuf,
        threads: Option<usize>,
    ) -> Result<(), std::io::Error> {
        let thread_count = threads.unwrap_or(1);

        let progress = Arc::new(RwLock::new(CacheProgress {
            current_speed: 0.0,
            total_bytes_read: 0,
            total_size: self.directory_sizer.get_allocated_size(&source_path).await,
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

        tokio::spawn(async move {
            if let Err(e) = Self::cache_file(
                &source_path_clone,
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

    pub async fn get_progress(&self, path: &PathBuf) -> Option<Arc<RwLock<CacheProgress>>> {
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

    async fn cache_file(
        source_path: &PathBuf,
        progress: &Arc<RwLock<CacheProgress>>,
        chunk_size: usize,
        threads: usize,
    ) -> Result<(), std::io::Error> {
        Self::cache_file_inner(source_path, progress, chunk_size, threads).await
    }

    fn cache_file_inner<'a>(
        source_path: &'a PathBuf,
        progress: &'a Arc<RwLock<CacheProgress>>,
        chunk_size: usize,
        in_threads: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), std::io::Error>> + Send + 'a>> {
        Box::pin(async move {
            // If file is directory, recursively cache all files in directory
            if tokio::fs::metadata(source_path).await?.is_dir() {
                let mut dir = tokio::fs::read_dir(source_path).await?;
                while let Some(entry) = dir.next_entry().await? {
                    let entry_path = entry.path();
                    Self::cache_file(&entry_path, progress, chunk_size, in_threads).await?;
                }
                return Ok(());
            }

            let mut threads = in_threads;

            // For regular files, use multi-threaded approach
            let file_size = tokio::fs::metadata(source_path).await?.len();

            // If file is small, don't use multiple threads
            if file_size < (chunk_size * threads) as u64 {
                threads = 1;
            }

            // Define overlap size - 5% of the chunk or 1MB, whichever is smaller
            let overlap_size = std::cmp::min((file_size / threads as u64) / 20, 1024 * 1024);

            // Calculate segment size for each thread
            let segment_size = file_size / threads as u64;

            // Create a vector to store all thread futures
            let mut handles = Vec::with_capacity(threads);

            // Create and spawn threads
            for thread_idx in 0..threads {
                let source_path_clone = source_path.clone();
                let progress_clone = Arc::clone(progress);

                // Calculate start and end positions for this thread
                let start_pos = if thread_idx == 0 {
                    0
                } else {
                    thread_idx as u64 * segment_size - overlap_size
                };

                let end_pos = if thread_idx == threads - 1 {
                    file_size
                } else {
                    (thread_idx + 1) as u64 * segment_size
                };

                let handle = tokio::spawn(async move {
                    Self::read_file_segment(
                        &source_path_clone,
                        &progress_clone,
                        chunk_size,
                        start_pos,
                        end_pos,
                    )
                    .await
                });

                handles.push(handle);
            }

            // Wait for all threads to complete
            for handle in handles {
                if let Err(e) = handle.await? {
                    tracing::error!("Error in thread reading file segment: {}", e);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Thread error",
                    ));
                }
            }

            Ok(())
        })
    }

    async fn read_file_segment(
        source_path: &PathBuf,
        progress: &Arc<RwLock<CacheProgress>>,
        chunk_size: usize,
        start_pos: u64,
        end_pos: u64,
    ) -> Result<(), std::io::Error> {
        let mut file = File::open(source_path).await?;

        // Seek to the starting position
        file.seek(SeekFrom::Start(start_pos)).await?;

        let mut buffer = vec![0u8; chunk_size];
        let mut bytes_read = 0;
        let mut last_update = std::time::SystemTime::now();
        let mut current_pos = start_pos;

        while current_pos < end_pos {
            // Calculate how much to read in this iteration
            let bytes_to_read = std::cmp::min(chunk_size, (end_pos - current_pos) as usize);

            // Read the chunk
            let n = file.read(&mut buffer[0..bytes_to_read]).await?;
            if n == 0 {
                break; // End of file
            }

            current_pos += n as u64;
            bytes_read += n as i64;
            let now = std::time::SystemTime::now();

            let time_passed = now
                .duration_since(last_update)
                .unwrap_or(std::time::Duration::from_secs(0));
            if time_passed >= std::time::Duration::from_secs(1) {
                let mut progress_guard = progress.write();
                progress_guard.total_bytes_read += bytes_read;
                progress_guard.update_speed(bytes_read, now);
                progress_guard.cached_size += bytes_read;
                bytes_read = 0;
                last_update = now;
            }
        }

        if bytes_read > 0 {
            let mut progress_guard = progress.write();
            progress_guard.total_bytes_read += bytes_read;
            progress_guard.update_speed(bytes_read, std::time::SystemTime::now());
            progress_guard.cached_size += bytes_read;
        }

        Ok(())
    }
}
