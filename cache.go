package main

import (
	"io"
	"log"
	"os"
	"path/filepath"
	"sync"
	"time"
)

type FileInfo struct {
	Name        string  `json:"name"`
	Path        string  `json:"path"`
	IsDir       bool    `json:"is_dir"`
	Size        *int64  `json:"size"` // Changed to pointer to allow null for directories
	CreatedTime float64 `json:"created_time"`
	CachedSize  int64   `json:"cached_size"`
}

type CacheProgress struct {
	CurrentSpeed   float64 `json:"current_speed"`
	TotalBytesRead int64   `json:"total_bytes_read"`
	TotalSize      int64   `json:"total_size"`
	IsComplete     bool    `json:"is_complete"`
	CachedSize     int64   `json:"cached_size"`
}

type GlobalProgress struct {
	TotalSpeed     float64 `json:"total_speed"`
	OverallPercent float64 `json:"overall_percent"`
	ActiveJobs     int     `json:"active_jobs"`
	CachedSize     int64   `json:"cached_size"`
}

// CacheManager handles active caching operations
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

// cacheFile handles caching a single file
func (cm *CacheManager) cacheFile(sourcePath, cachePath string, progress *CacheProgress) error {
	startTime := time.Now()
	lastUpdate := startTime

	sourceFile, err := os.Open(sourcePath)
	if err != nil {
		return err
	}
	defer sourceFile.Close()

	buffer := make([]byte, cm.chunkSize)
	for {
		n, err := sourceFile.Read(buffer)
		if err == io.EOF {
			break
		}
		if err != nil {
			return err
		}

		progress.TotalBytesRead += int64(n)
		currentTime := time.Now()

		if currentTime.Sub(lastUpdate) >= time.Second {
			cm.Lock()
			progress.CurrentSpeed = float64(progress.TotalBytesRead) / currentTime.Sub(startTime).Seconds()
			progress.CachedSize = cm.sizer.calculateSize(cachePath)
			cm.Unlock()
			lastUpdate = currentTime
		}
	}

	progress.CurrentSpeed = float64(progress.TotalBytesRead) / time.Since(startTime).Seconds()
	progress.CachedSize = cm.sizer.calculateSize(cachePath)
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
	}()
	return progress, nil
}

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
