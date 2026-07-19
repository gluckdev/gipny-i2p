// gipny-i2p-router is the bundled i2p transport helper for gipny.
//
// It is a thin wrapper around go-i2p's SAM bridge embedding library
// (github.com/go-i2p/go-sam-bridge/lib/embedding). It starts an embedded,
// pure-Go i2p router together with a SAMv3 bridge on a local TCP port; the
// gipny client (Rust) then speaks SAMv3 to it via the `yosemite` crate.
//
// This replaces the previously embedded Tor (Arti) transport. Unlike Arti,
// i2p needs a running router, and this single self-contained Go binary
// provides both the router and the SAM API, so the app stays "zero install".
//
// Usage:
//
//	gipny-i2p-router --sam-listen 127.0.0.1:7656 --data /path/to/profile/i2p/router
//
// The process runs until it receives SIGINT/SIGTERM (the parent gipny process
// kills it on lock/exit). It exits non-zero if the bridge fails to start.
package main

import (
	"context"
	"flag"
	"log"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
)

func main() {
	samListen := flag.String("sam-listen", "127.0.0.1:7656", "SAM v3 TCP listen address")
	dataDir := flag.String("data", "", "router data directory (i2p netdb/state)")
	debug := flag.Bool("debug", false, "enable debug logging")
	mock := flag.Bool("mock", false, "run mock SAM bridge for local testing")
	flag.Parse()

	if *mock {
		log.Printf("gipny-i2p-router: running in MOCK mode (pure-local SAMv3 mock server on %s)", *samListen)
		server := NewMockSAMServer(*samListen)
		if err := server.Start(); err != nil {
			log.Fatalf("gipny-i2p-router: start mock server: %v", err)
		}
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
		<-sig
		log.Printf("gipny-i2p-router: shutting down mock server")
		server.Stop()
		return
	}

	// Isolate the embedded router's on-disk state under the profile directory.
	// The embedding API has no explicit data-dir option yet, so we chdir into
	// the requested directory and let go-i2p write its state relative to CWD.
	if *dataDir != "" {
		abs, err := filepath.Abs(*dataDir)
		if err != nil {
			log.Fatalf("gipny-i2p-router: resolve data dir: %v", err)
		}
		if err := os.MkdirAll(abs, 0o700); err != nil {
			log.Fatalf("gipny-i2p-router: create data dir: %v", err)
		}
		if err := os.Chdir(abs); err != nil {
			log.Fatalf("gipny-i2p-router: chdir data dir: %v", err)
		}
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start the embedded router + SAM bridge with I2CP transport wired in, so
	// STREAM sessions can actually carry data (see wiring.go).
	wb, err := startWiredBridge(ctx, *samListen, *debug)
	if err != nil {
		log.Fatalf("gipny-i2p-router: %v", err)
	}
	log.Printf("gipny-i2p-router: SAMv3 bridge listening on %s (embedded go-i2p router, I2CP wired)", *samListen)

	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
	<-sig

	log.Printf("gipny-i2p-router: shutting down")
	wb.Stop(context.Background())
}
