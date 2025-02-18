package main

import (
	"io/fs"
	"path/filepath"
	"sync"
	"syscall"
	"time"
)

// SizeCache holds size information with timestamp
type SizeCache struct {
	Size      int64
	Timestamp time.Time
}

// DirectorySizer handles directory size calculations with caching
type DirectorySizer struct {
	cache map[string]SizeCache
	mu    sync.RWMutex
	// Cache entries older than this will be recalculated
	maxAge time.Duration
}

// NewDirectorySizer creates a new DirectorySizer instance
func NewDirectorySizer() *DirectorySizer {
	return &DirectorySizer{
		cache:  make(map[string]SizeCache),
		maxAge: 5 * time.Minute, // Cache entries expire after 5 minutes
	}
}

// GetAllocatedSize gets the actual allocated size of a file or directory
func (ds *DirectorySizer) GetAllocatedSize(path string) int64 {
	// Convert to absolute path for consistent cache keys
	absPath, err := filepath.Abs(path)
	if err != nil {
		return 0
	}

	// Check cache first
	if size := ds.checkCache(absPath); size >= 0 {
		return size
	}

	// Calculate new size
	size := ds.calculateSize(absPath)

	// Store in cache
	ds.mu.Lock()
	ds.cache[absPath] = SizeCache{
		Size:      size,
		Timestamp: time.Now(),
	}
	ds.mu.Unlock()

	return size
}

// checkCache checks if we have a valid cached size
func (ds *DirectorySizer) checkCache(path string) int64 {
	ds.mu.RLock()
	defer ds.mu.RUnlock()

	if entry, exists := ds.cache[path]; exists {
		if time.Since(entry.Timestamp) < ds.maxAge {
			return entry.Size
		}
	}
	return -1
}

// calculateSize computes the actual size of a file or directory
func (ds *DirectorySizer) calculateSize(path string) int64 {
	var stat syscall.Stat_t
	if err := syscall.Stat(path, &stat); err != nil {
		return 0
	}

	// If it's not a directory, return its size directly
	if stat.Mode&syscall.S_IFDIR == 0 {
		return stat.Blocks * 512
	}
	cacheHits := 0

	// For directories, walk through all contents
	var totalSize int64
	filepath.WalkDir(path, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return filepath.SkipDir
		}

		// Check if we have this path in cache
		if cachedSize := ds.checkCache(p); cachedSize >= 0 {
			cacheHits += 1
			totalSize += cachedSize
			return nil // Skip this directory as we have its size
		}

		// If not in cache, get size of this item
		if err := syscall.Stat(p, &stat); err != nil {
			return nil
		}

		// For non-directories, add size and cache it
		if stat.Mode&syscall.S_IFDIR == 0 {
			size := stat.Blocks * 512
			ds.mu.Lock()
			ds.cache[p] = SizeCache{
				Size:      size,
				Timestamp: time.Now(),
			}
			ds.mu.Unlock()
			totalSize += size
		}

		return nil
	})

	//log.Printf("Total %d of file sizes using cache", cacheHits)

	return totalSize
}
