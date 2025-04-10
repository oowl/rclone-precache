package main

import (
	_ "embed"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"time"

	"github.com/gin-contrib/cors"
	"github.com/gin-gonic/gin"
)

//go:embed frontend/index.html
var indexHTML string

//go:embed frontend/js/tailwindcss.js
var tailwindcssJS string

//go:embed frontend/js/react.production.min.js
var reactJS string

//go:embed frontend/js/react-dom.production.min.js
var reactDomJS string

//go:embed frontend/js/babel.min.js
var babelJS string

type Server struct {
	cacheManager *CacheManager
	sizer        *DirectorySizer
	mountPath    string
	cachePath    string
	threadCount  int
}

func NewServer(mountPath string, cachePath string, chunkSize int, threadCount int) *Server {
	return &Server{
		cacheManager: NewCacheManager(chunkSize),
		sizer:        NewDirectorySizer(),
		mountPath:    mountPath,
		cachePath:    cachePath,
		threadCount:  threadCount,
	}
}

// handleBrowse handles directory browsing requests
func (s *Server) handleBrowse(c *gin.Context) {
	reqPath := c.Param("path")
	fullPath := filepath.Join(s.mountPath, reqPath)
	cacheBase := filepath.Join(s.cachePath, reqPath)

	entries, err := os.ReadDir(fullPath)
	if err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "Path not found"})
		return
	}

	var fileInfos []FileInfo
	for _, entry := range entries {
		info, err := entry.Info()
		if err != nil {
			continue
		}

		cachePath := filepath.Join(cacheBase, entry.Name())
		var size *int64
		if !entry.IsDir() {
			s := info.Size()
			size = &s
		}

		fileInfo := FileInfo{
			Name:        entry.Name(),
			Path:        filepath.Join(reqPath, entry.Name()),
			IsDir:       entry.IsDir(),
			Size:        size,
			CreatedTime: float64(info.ModTime().Unix()),
			CachedSize:  s.sizer.calculateSize(cachePath),
		}
		fileInfos = append(fileInfos, fileInfo)
	}

	sort.Slice(fileInfos, func(i, j int) bool {
		return fileInfos[i].CreatedTime > fileInfos[j].CreatedTime
	})

	c.JSON(http.StatusOK, fileInfos)
}

// handlePrecache handles precaching requests
func (s *Server) handlePrecache(c *gin.Context) {
	reqPath := c.Param("path")
	sourcePath := filepath.Join(s.mountPath, reqPath)
	cachePath := filepath.Join(s.cachePath, reqPath)

	if _, exists := s.cacheManager.GetProgress(sourcePath); exists {
		c.JSON(http.StatusBadRequest, gin.H{"message": fmt.Sprintf("Precache already in progress for %s", reqPath)})
		return
	}

	_, err := s.cacheManager.StartProgress(sourcePath, cachePath, s.threadCount)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": err.Error()})
		return
	}

	c.JSON(http.StatusOK, gin.H{"message": fmt.Sprintf("Started caching directory: %s", reqPath)})
}

// handleCacheProgress handles progress monitoring requests
func (s *Server) handleCacheProgress(c *gin.Context) {
	reqPath := c.Param("path")
	if reqPath == "/" {
		// Return global progress
		progress := s.cacheManager.GetGlobalProgress()
		// Add cache size to global progress
		progress.CachedSize = s.sizer.calculateSize(s.cachePath)
		c.JSON(http.StatusOK, progress)
		return
	}

	time.Sleep(1 * time.Second)
	sourcePath := filepath.Join(s.mountPath, reqPath)
	progress, exists := s.cacheManager.GetProgress(sourcePath)
	if !exists {
		c.JSON(http.StatusNotFound, gin.H{"error": "No active cache operation found"})
		return
	}
	c.JSON(http.StatusOK, progress)
}

func (s *Server) SetupRouter() *gin.Engine {
	router := gin.Default()

	// Configure CORS
	router.Use(cors.New(cors.Config{
		AllowOrigins:     []string{"*"},
		AllowMethods:     []string{"GET", "POST", "OPTIONS"},
		AllowHeaders:     []string{"Origin", "Content-Type"},
		ExposeHeaders:    []string{"Content-Length"},
		AllowCredentials: true,
	}))

	// API routes
	api := router.Group("/api")
	{
		api.GET("/browse/*path", s.handleBrowse)
		api.POST("/precache/*path", s.handlePrecache)
		api.GET("/cache-progress/*path", s.handleCacheProgress)
	}

	// Serve JS
	router.GET("/js/tailwindcss.js", func(c *gin.Context) {
		c.Header("Content-Type", "application/javascript")
		c.String(http.StatusOK, tailwindcssJS)
	})
	router.GET("/js/react.production.min.js", func(c *gin.Context) {
		c.Header("Content-Type", "application/javascript")
		c.String(http.StatusOK, reactJS)
	})
	router.GET("/js/react-dom.production.min.js", func(c *gin.Context) {
		c.Header("Content-Type", "application/javascript")
		c.String(http.StatusOK, reactDomJS)
	})
	router.GET("/js/babel.min.js", func(c *gin.Context) {
		c.Header("Content-Type", "application/javascript")
		c.String(http.StatusOK, babelJS)
	})

	// Serve index.html for all other routes
	router.NoRoute(func(c *gin.Context) {
		c.Header("Content-Type", "text/html")
		c.String(http.StatusOK, indexHTML)
	})

	return router
}
