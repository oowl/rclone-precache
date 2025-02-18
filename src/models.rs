use serde::Serialize;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Serialize)]
pub struct SpeedWindow {
    pub bytes_read: i64,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<i64>,
    pub created_time: f64,
    pub cached_size: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheProgress {
    pub current_speed: f64,
    pub total_bytes_read: i64,
    pub total_size: i64,
    pub is_complete: bool,
    pub cached_size: i64,
    #[serde(skip)]
    pub speed_windows: Vec<SpeedWindow>,
}

impl CacheProgress {
    pub fn update_speed(&mut self, bytes_read: i64, current_time: SystemTime) {
        self.speed_windows.push(SpeedWindow {
            bytes_read,
            timestamp: current_time,
        });

        // Remove windows older than 5 seconds
        let cutoff = current_time - Duration::from_secs(5);
        self.speed_windows.retain(|window| window.timestamp > cutoff);

        // Calculate average speed
        let total_bytes: i64 = self.speed_windows.iter().map(|w| w.bytes_read).sum();
        if let Some(first_window) = self.speed_windows.first() {
            if let Ok(duration) = current_time.duration_since(first_window.timestamp) {
                let seconds = duration.as_secs_f64();
                if seconds > 0.0 {
                    self.current_speed = total_bytes as f64 / seconds;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobalProgress {
    pub total_speed: f64,
    pub overall_percent: f64,
    pub active_jobs: i32,
    pub cached_size: i64,
}