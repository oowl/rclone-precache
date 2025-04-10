package main

import (
	"io"
	"log"
	"math"
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
	mu             sync.Mutex    // Mutex for thread-safe updates
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

// Thread-safe update of progress
func (cp *CacheProgress) safeUpdate(bytesRead int64, currentTime time.Time) {
	cp.mu.Lock()
	defer cp.mu.Unlock()

	cp.TotalBytesRead += bytesRead
	cp.updateSpeed(bytesRead, currentTime)
	cp.CachedSize += bytesRead
}

func (cm *CacheManager) readFileSegment(file *os.File, startPos, endPos int64, progress *CacheProgress) error {
	// Seek to the start position
	_, err := file.Seek(startPos, io.SeekStart)
	if err != nil {
		return err
	}

	// Create a buffer for this segment
	buffer := make([]byte, cm.chunkSize)
	currentPos := startPos
	bytesRead := int64(0)
	lastUpdate := time.Now()

	for currentPos < endPos {
		// Calculate how much to read in this iteration
		bytesToRead := cm.chunkSize
		if int64(bytesToRead) > (endPos - currentPos) {
			bytesToRead = int(endPos - currentPos)
		}

		n, err := file.Read(buffer[:bytesToRead])
		if err == io.EOF {
			break
		}
		if err != nil {
			return err
		}

		currentPos += int64(n)
		bytesRead += int64(n)
		currentTime := time.Now()

		if currentTime.Sub(lastUpdate) >= time.Second {
			progress.safeUpdate(bytesRead, currentTime)
			bytesRead = 0
			lastUpdate = currentTime
		}
	}

	// Handle any remaining bytes
	if bytesRead > 0 {
		progress.safeUpdate(bytesRead, time.Now())
	}

	return nil
}

func (cm *CacheManager) cacheFile(sourcePath string, progress *CacheProgress, threads int) error {
	// Open the file once to get its size
	sourceFile, err := os.Open(sourcePath)
	if err != nil {
		return err
	}

	fileInfo, err := sourceFile.Stat()
	if err != nil {
		sourceFile.Close()
		return err
	}

	fileSize := fileInfo.Size()
	sourceFile.Close()

	// If file is small, use single thread approach
	if fileSize < int64(cm.chunkSize*threads) {
		threads = 1
	}

	// Calculate segment size and overlap
	segmentSize := fileSize / int64(threads)
	// Overlap is 5% of segment size or 1MB, whichever is smaller
	overlapSize := int64(math.Min(float64(segmentSize)/20, 1024*1024))

	var wg sync.WaitGroup
	errors := make(chan error, threads)

	for i := 0; i < threads; i++ {
		wg.Add(1)

		go func(threadIndex int) {
			defer wg.Done()

			// Calculate start and end positions for this thread
			startPos := int64(0)
			if threadIndex > 0 {
				startPos = int64(threadIndex)*segmentSize - overlapSize
				if startPos < 0 {
					startPos = 0
				}
			}

			endPos := fileSize
			if threadIndex < threads-1 {
				endPos = int64(threadIndex+1) * segmentSize
			}

			// Open a separate file handle for each thread
			file, err := os.Open(sourcePath)
			if err != nil {
				errors <- err
				return
			}
			defer file.Close()

			if err := cm.readFileSegment(file, startPos, endPos, progress); err != nil {
				errors <- err
			}
		}(i)
	}

	// Wait for all threads to complete
	wg.Wait()
	close(errors)

	// Check for errors
	for err := range errors {
		if err != nil {
			return err
		}
	}

	return nil
}

func (cm *CacheManager) StartProgress(sourcePath, cachePath string, threadCount int) (*CacheProgress, error) {
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
			if err := cm.cacheFile(sourcePath, progress, threadCount); err != nil {
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
					// cachePathNew := filepath.Join(cachePath, relPath)
					if err := cm.cacheFile(path, progress, threadCount); err != nil {
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
	var totalRead, totalSize, cachedSize int64
	activeJobs := 0

	for _, progress := range cm.active {
		if !progress.IsComplete {
			totalSpeed += progress.CurrentSpeed
			totalRead += progress.TotalBytesRead
			totalSize += progress.TotalSize
			cachedSize += progress.CachedSize
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
		CachedSize:     cachedSize,
	}
}
