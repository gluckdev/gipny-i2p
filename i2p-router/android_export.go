//go:build android

// Android in-process embedding: exports StartSam/StopSam as C ABI functions
// (via cgo //export) so the JNI shim in jni_shim_android.c can call them.
// This is built as a cgo c-shared library (see .github/workflows/release.yml,
// android job) instead of the standalone CLI binary used on desktop.
package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"unsafe"

	"github.com/go-i2p/go-sam-bridge/lib/embedding"
)

var (
	mu     sync.Mutex
	bridge *embedding.Bridge
	cancel context.CancelFunc
)

// StartSam starts the embedded go-i2p router + SAMv3 bridge in-process.
// dataDir is the router's on-disk state directory (created if missing).
// samListen is the SAM TCP listen address, e.g. "127.0.0.1:7656".
// Returns nil on success, or a heap-allocated C string describing the error
// (caller must free it, e.g. via C.free, once done reading it).
//
//export StartSam
func StartSam(dataDir *C.char, samListen *C.char) *C.char {
	mu.Lock()
	defer mu.Unlock()

	if bridge != nil {
		// Already running; treat as success (idempotent for service restarts).
		return nil
	}

	dir := C.GoString(dataDir)
	listen := C.GoString(samListen)

	if dir != "" {
		abs, err := filepath.Abs(dir)
		if err != nil {
			return C.CString("resolve data dir: " + err.Error())
		}
		if err := os.MkdirAll(abs, 0o700); err != nil {
			return C.CString("create data dir: " + err.Error())
		}
		if err := os.Chdir(abs); err != nil {
			return C.CString("chdir data dir: " + err.Error())
		}
	}

	b, err := embedding.New(
		embedding.WithListenAddr(listen),
	)
	if err != nil {
		return C.CString("init bridge: " + err.Error())
	}

	ctx, cancelFn := context.WithCancel(context.Background())
	if err := b.Start(ctx); err != nil {
		cancelFn()
		return C.CString("start bridge: " + err.Error())
	}

	bridge = b
	cancel = cancelFn
	return nil
}

// StopSam stops the embedded router + SAM bridge started by StartSam.
// Safe to call even if StartSam was never called or already stopped.
//
//export StopSam
func StopSam() {
	mu.Lock()
	defer mu.Unlock()

	if bridge == nil {
		return
	}
	bridge.Stop(context.Background())
	if cancel != nil {
		cancel()
	}
	bridge = nil
	cancel = nil
}

// freeCString frees a *C.char previously returned by StartSam. Exposed so the
// JNI shim can release the error string after copying it into a jstring.
//
//export FreeCString
func FreeCString(s *C.char) {
	C.free(unsafe.Pointer(s))
}
