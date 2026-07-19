//go:build mocksam

// Mock SAMv3 server for the local development loop only.
//
// It speaks just enough of SAMv3 to let the relay and two bots hand streams to
// each other over loopback TCP, with no i2p router, no reseed, and no tunnels.
// That makes the e2e harness runnable in seconds while iterating on the Rust
// side (see run-e2e.sh, which passes --mock).
//
// It is behind the `mocksam` build tag on purpose:
//
//   - the shipped binary must never be able to serve fake destinations;
//   - a CI job running against this mock would be green without exercising i2p
//     at all, which is worse than a flaky job that tells the truth. Do not wire
//     --mock into .github/workflows/e2e.yml.
//
// Build with: go build -tags mocksam -o gipny-i2p-router .
package main

import (
	"bufio"
	"crypto/rand"
	"encoding/base64"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"
	"time"
)

var mockSAMFlag = flag.Bool("mock", false, "run a mock SAMv3 server instead of a real router (local testing only)")

// mockSAMRequested reports whether --mock was passed. Always false in builds
// without the mocksam tag (see mock_sam_stub.go).
func mockSAMRequested() bool { return *mockSAMFlag }

// runMockSAM serves the mock bridge on addr until SIGINT/SIGTERM.
func runMockSAM(addr string) {
	log.Printf("gipny-i2p-router: running in MOCK mode (pure-local SAMv3 mock server on %s) — NOT a real i2p transport", addr)
	server := NewMockSAMServer(addr)
	if err := server.Start(); err != nil {
		log.Fatalf("gipny-i2p-router: start mock server: %v", err)
	}
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
	<-sig
	log.Printf("gipny-i2p-router: shutting down mock server")
	server.Stop()
}

type mockSession struct {
	ID          string
	Destination string
}

type MockSAMServer struct {
	listenAddr string
	listener   net.Listener
	mu         sync.Mutex
	keys       map[string]string // privateKey -> publicKey
	sessions   map[string]*mockSession
	acceptors  map[string]chan net.Conn // sessionID -> channel of pending acceptors
}

func NewMockSAMServer(listenAddr string) *MockSAMServer {
	return &MockSAMServer{
		listenAddr: listenAddr,
		keys:       make(map[string]string),
		sessions:   make(map[string]*mockSession),
		acceptors:  make(map[string]chan net.Conn),
	}
}

func (s *MockSAMServer) Start() error {
	l, err := net.Listen("tcp", s.listenAddr)
	if err != nil {
		return err
	}
	s.listener = l
	go s.acceptLoop()
	return nil
}

func (s *MockSAMServer) Stop() {
	if s.listener != nil {
		s.listener.Close()
	}
}

func (s *MockSAMServer) acceptLoop() {
	for {
		conn, err := s.listener.Accept()
		if err != nil {
			return
		}
		go s.handleConnection(conn)
	}
}

func i2pBase64Encode(b []byte) string {
	s := base64.StdEncoding.EncodeToString(b)
	s = strings.ReplaceAll(s, "+", "-")
	s = strings.ReplaceAll(s, "/", "~")
	return s
}

func (s *MockSAMServer) handleConnection(conn net.Conn) {
	reader := bufio.NewReader(conn)
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			conn.Close()
			return
		}
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		log.Printf("[mock-sam] received: %s", line)
		parts := strings.Fields(line)
		if len(parts) < 2 {
			fmt.Fprint(conn, "REPLY RESULT=ERROR\n")
			conn.Close()
			return
		}

		cmd := parts[0]
		subCmd := parts[1]

		if cmd == "HELLO" {
			fmt.Fprint(conn, "HELLO REPLY RESULT=OK VERSION=3.1\n")
			continue
		}

		if cmd == "DEST" && subCmd == "GENERATE" {
			pubBytes := make([]byte, 384)
			rand.Read(pubBytes)
			pub := i2pBase64Encode(pubBytes) + "cABA=="

			privBytes := make([]byte, 256)
			rand.Read(privBytes)
			priv := i2pBase64Encode(privBytes)

			s.mu.Lock()
			s.keys[priv] = pub
			s.mu.Unlock()

			fmt.Fprintf(conn, "DEST REPLY PUB=%s PRIV=%s\n", pub, priv)
			continue
		}

		if cmd == "SESSION" && subCmd == "CREATE" {
			id := ""
			dest := ""
			for _, part := range parts[2:] {
				kv := strings.SplitN(part, "=", 2)
				if len(kv) == 2 {
					if kv[0] == "ID" {
						id = kv[1]
					} else if kv[0] == "DESTINATION" {
						dest = kv[1]
					}
				}
			}

			s.mu.Lock()
			pub, exists := s.keys[dest]
			if !exists {
				pubBytes := make([]byte, 384)
				rand.Read(pubBytes)
				pub = i2pBase64Encode(pubBytes) + "cABA=="
			}
			s.sessions[id] = &mockSession{
				ID:          id,
				Destination: pub,
			}
			if _, exists := s.acceptors[id]; !exists {
				s.acceptors[id] = make(chan net.Conn, 100)
			}
			s.mu.Unlock()

			fmt.Fprintf(conn, "SESSION STATUS RESULT=OK DESTINATION=%s\n", dest)
			continue
		}

		if cmd == "STREAM" && subCmd == "ACCEPT" {
			id := ""
			for _, part := range parts[2:] {
				kv := strings.SplitN(part, "=", 2)
				if len(kv) == 2 && kv[0] == "ID" {
					id = kv[1]
				}
			}

			s.mu.Lock()
			ch, exists := s.acceptors[id]
			if !exists {
				ch = make(chan net.Conn, 100)
				s.acceptors[id] = ch
			}
			s.mu.Unlock()

			ch <- conn
			log.Printf("[mock-sam] registered acceptor for session %s", id)
			// Return here WITHOUT closing conn. The pairing logic will take care of it.
			return
		}

		if cmd == "STREAM" && subCmd == "CONNECT" {
			id := ""
			targetDest := ""
			for _, part := range parts[2:] {
				kv := strings.SplitN(part, "=", 2)
				if len(kv) == 2 {
					if kv[0] == "ID" {
						id = kv[1]
					} else if kv[0] == "DESTINATION" {
						targetDest = kv[1]
					}
				}
			}

			s.mu.Lock()
			var targetSessionID string
			for sessID, sess := range s.sessions {
				if sess.Destination == targetDest {
					targetSessionID = sessID
					break
				}
			}
			connectorSess, hasConnector := s.sessions[id]
			s.mu.Unlock()

			if targetSessionID == "" || !hasConnector {
				log.Printf("[mock-sam] target destination %s not found", targetDest)
				fmt.Fprint(conn, "STREAM STATUS RESULT=CANT_REACH_PEER\n")
				conn.Close()
				return
			}

			s.mu.Lock()
			ch := s.acceptors[targetSessionID]
			s.mu.Unlock()

			select {
			case acceptorConn := <-ch:
				fmt.Fprint(conn, "STREAM STATUS RESULT=OK\n")
				fmt.Fprintf(acceptorConn, "STREAM STATUS RESULT=OK\n%s FROM_PORT=0 TO_PORT=0\n", connectorSess.Destination)

				log.Printf("[mock-sam] paired connector %s with acceptor %s", id, targetSessionID)
				s.copyData(conn, acceptorConn)
				return
			case <-time.After(10 * time.Second):
				log.Printf("[mock-sam] timeout waiting for acceptor %s", targetSessionID)
				fmt.Fprint(conn, "STREAM STATUS RESULT=TIMEOUT\n")
				conn.Close()
				return
			}
		}

		fmt.Fprint(conn, "REPLY RESULT=ERROR\n")
		conn.Close()
		return
	}
}

func (s *MockSAMServer) copyData(conn1, conn2 net.Conn) {
	defer conn1.Close()
	defer conn2.Close()

	errChan := make(chan error, 2)
	go func() {
		_, err := io.Copy(conn1, conn2)
		errChan <- err
	}()
	go func() {
		_, err := io.Copy(conn2, conn1)
		errChan <- err
	}()
	<-errChan
}
