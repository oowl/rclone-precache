use parking_lot::RwLock;
use std::collections::HashMap;
use std::os::linux::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use std::pin::Pin;
use std::future::Future;

struct SizeCache {
    size: i64,
    timestamp: SystemTime,
}

pub struct DirectorySizer {
    cache: RwLock<HashMap<String, SizeCache>>,
    max_age: Duration,
}

impl DirectorySizer {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            max_age: Duration::from_secs(300), // 5 minutes
        }
    }
    
    pub async fn get_allocated_size(&self, path: &PathBuf) -> i64 {
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        let path_str = abs_path.to_string_lossy().to_string();
        
        // Check cache first
        if let Some(size) = self.check_cache(&path_str) {
            return size;
        }
        
        // Calculate new size
        let size = self.calculate_size(&abs_path).await;
        
        // Store in cache
        self.cache.write().insert(
            path_str,
            SizeCache {
                size,
                timestamp: SystemTime::now(),
            },
        );
        
        size
    }
    
    fn check_cache(&self, path: &str) -> Option<i64> {
        let cache = self.cache.read();
        
        cache.get(path).and_then(|entry| {
            if SystemTime::now()
                .duration_since(entry.timestamp)
                .unwrap_or(Duration::from_secs(0))
                < self.max_age
            {
                Some(entry.size)
            } else {
                None
            }
        })
    }
    
    async fn calculate_size(&self, path: &PathBuf) -> i64 {
        self.calculate_size_inner(path).await
    }

    fn calculate_size_inner<'a>(&'a self, path: &'a PathBuf) -> Pin<Box<dyn Future<Output = i64> + 'a>> {
        Box::pin(async move {
            let metadata = match tokio::fs::metadata(path).await {
                Ok(m) => m,
                Err(_) => return 0,
            };

            if !metadata.is_dir() {
                return metadata.len() as i64;
            }

            let mut total_size = 0;
            let mut _cache_hits = 0;

            let mut read_dir = match tokio::fs::read_dir(path).await {
                Ok(rd) => rd,
                Err(_) => return 0,
            };

            while let Ok(Some(entry)) = read_dir.next_entry().await {
                let path = entry.path();
                let path_str = path.to_string_lossy().to_string();

                // Check cache first
                if let Some(cached_size) = self.check_cache(&path_str) {
                    total_size += cached_size;
                    _cache_hits += 1;
                    continue;
                }

                if let Ok(metadata) = entry.metadata().await {
                    let size = if metadata.is_file() {
                        metadata.st_blocks() as i64 * 512 as i64
                    } else {
                        self.calculate_size_inner(&path).await
                    };

                    self.cache.write().insert(
                        path_str,
                        SizeCache {
                            size,
                            timestamp: SystemTime::now(),
                        },
                    );

                    total_size += size;
                }
            }

            total_size
        })
    }
}