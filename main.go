package main

import (
	"flag"
	"log"
)

func main() {
	MountPath := flag.String("mount", "", "Source path")
	CachePath := flag.String("cache", "", "Cache path")
	ChunkSize := flag.Int("chunk", 1, "Chunk size in MB for caching")
	flag.Parse()

	if *MountPath == "" || *CachePath == "" {
		log.Fatal("Mount and cache paths are required")
	}

	// Create server instance
	server := NewServer(*MountPath, *CachePath, *ChunkSize*1024*1024)
	r := server.SetupRouter()
	if err := r.Run(":8000"); err != nil {
		log.Fatal(err)
	}
}
