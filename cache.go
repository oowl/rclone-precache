package main

import (
	"io"
	"log"
	"os"
	"path/filepath"
	"sync"
	"time"
)

type SpeedWindow struct {
	bytesRead int64
	timestamp time.Time
}

type FileInfo struct {
	Name        string  `json:"name"`
	Path        string  `json:"path"`
	IsDir       bool    `json:"is_dir"`
	Size        *int64  `json:"size"`
	CreatedTime float64 `json:"created_time"`
	CachedSize  int64   `json:"cached_size"`
}

type CacheProgress struct {
	CurrentSpeed   float64       `json:"current_speed"`
	TotalBytesRead int64         `json:"total_bytes_read"`
	TotalSize      int64         `json:"total_size"`
	IsComplete     bool          `json:"is_complete"`
	CachedSize     int64         `json:"cached_size"`
	buffer         []byte        // Buffer for reading file data
	speedWindows   []SpeedWindow // Track speed history
}

type GlobalProgress struct {
	TotalSpeed     float64 `json:"total_speed"`
	OverallPercent float64 `json:"overall_percent"`
	ActiveJobs     int     `json:"active_jobs"`
	CachedSize     int64   `json:"cached_size"`
}

type CacheManager struct {
	sync.RWMutex
	chunkSize int
	active    map[string]*CacheProgress
	sizer     *DirectorySizer
}

func NewCacheManager(chunkSize int) *CacheManager {
	return &CacheManager{
		active:    make(map[string]*CacheProgress),
		sizer:     NewDirectorySizer(),
		chunkSize: chunkSize,
	}
}

// updateSpeed calculates the average speed over the last 5 seconds
func (cp *CacheProgress) updateSpeed(bytesRead int64, currentTime time.Time) {
	// Add new window
	cp.speedWindows = append(cp.speedWindows, SpeedWindow{
		bytesRead: bytesRead,
		timestamp: currentTime,
	})

	// Remove windows older than 5 seconds
	cutoffTime := currentTime.Add(-5 * time.Second)
	var validWindows []SpeedWindow
	var totalBytes int64

	for _, window := range cp.speedWindows {
		if window.timestamp.After(cutoffTime) {
			validWindows = append(validWindows, window)
			totalBytes += window.bytesRead
		}
	}

	cp.speedWindows = validWindows

	// Calculate average speed over the valid windows
	if len(validWindows) > 0 {
		timeRange := currentTime.Sub(validWindows[0].timestamp).Seconds()
		if timeRange > 0 {
			cp.CurrentSpeed = float64(totalBytes) / timeRange
		}
	}
}

func (cm *CacheManager) cacheFile(sourcePath, _ string, progress *CacheProgress) error {
	sourceFile, err := os.Open(sourcePath)
	if err != nil {
		return err
	}
	defer sourceFile.Close()

	nn := 0
	lastUpdate := time.Now()

	for {
		n, err := sourceFile.Read(progress.buffer)
		if err == io.EOF {
			break
		}
		if err != nil {
			return err
		}

		currentTime := time.Now()
		nn += n

		if currentTime.Sub(lastUpdate) >= time.Second {
			cm.Lock()
			progress.TotalBytesRead += int64(nn)
			progress.updateSpeed(int64(nn), currentTime)
			progress.CachedSize += int64(nn)
			cm.Unlock()
			nn = 0
			lastUpdate = currentTime
		}
	}

	// Handle any remaining bytes
	if nn > 0 {
		cm.Lock()
		progress.TotalBytesRead += int64(nn)
		progress.updateSpeed(int64(nn), time.Now())
		progress.CachedSize += int64(nn)
		cm.Unlock()
	}

	return nil
}

func (cm *CacheManager) StartProgress(sourcePath, cachePath string) (*CacheProgress, error) {
	cm.Lock()
	defer cm.Unlock()

	info, err := os.Stat(sourcePath)
	if err != nil {
		return nil, err
	}

	progress := &CacheProgress{
		CurrentSpeed:   0,
		TotalBytesRead: 0,
		TotalSize:      cm.sizer.GetAllocatedSize(sourcePath),
		IsComplete:     false,
		speedWindows:   make([]SpeedWindow, 0),
		buffer:         make([]byte, cm.chunkSize),
	}
	cm.active[sourcePath] = progress

	go func() {
		if !info.IsDir() {
			if err := cm.cacheFile(sourcePath, cachePath, progress); err != nil {
				log.Printf("Error caching file %s: %v", sourcePath, err)
			}
		} else {
			err := filepath.Walk(sourcePath, func(path string, info os.FileInfo, err error) error {
				if err != nil {
					return err
				}
				if !info.IsDir() {
					relPath, err := filepath.Rel(sourcePath, path)
					if err != nil {
						return err
					}
					cachePathNew := filepath.Join(cachePath, relPath)
					if err := cm.cacheFile(path, cachePathNew, progress); err != nil {
						log.Printf("Error caching file %s: %v", relPath, err)
					}
				}
				return nil
			})
			if err != nil {
				log.Printf("Error walking directory %s: %v", sourcePath, err)
			}
		}
		cm.CompleteProgress(sourcePath)
	}()
	return progress, nil
}

// Other methods remain unchanged
func (cm *CacheManager) GetProgress(path string) (*CacheProgress, bool) {
	cm.RLock()
	defer cm.RUnlock()
	progress, exists := cm.active[path]
	return progress, exists
}

func (cm *CacheManager) CompleteProgress(path string) {
	cm.Lock()
	defer cm.Unlock()
	if progress, exists := cm.active[path]; exists {
		progress.IsComplete = true
	}
	go func() {
		time.Sleep(1 * time.Second)
		cm.Lock()
		delete(cm.active, path)
		cm.Unlock()
	}()
}

func (cm *CacheManager) GetGlobalProgress() GlobalProgress {
	cm.RLock()
	defer cm.RUnlock()

	var totalSpeed float64
	var totalRead, totalSize int64
	activeJobs := 0

	for _, progress := range cm.active {
		if !progress.IsComplete {
			totalSpeed += progress.CurrentSpeed
			totalRead += progress.TotalBytesRead
			totalSize += progress.TotalSize
			activeJobs++
		}
	}

	overallPercent := 0.0
	if totalSize > 0 {
		overallPercent = float64(totalRead) / float64(totalSize) * 100
	}

	return GlobalProgress{
		TotalSpeed:     totalSpeed,
		OverallPercent: overallPercent,
		ActiveJobs:     activeJobs,
	}
}
