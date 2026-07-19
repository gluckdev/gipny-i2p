// I2CP wiring for the embedded SAM bridge.
//
// embedding.New() starts an embedded go-i2p router with an I2CP *server*, but
// it never connects an I2CP *client* to it, so DefaultHandlerRegistrar's
// per-session callback bails out ("Session created without I2CP transport")
// and no StreamManager is ever registered. The result: SESSION CREATE returns
// OK, but every STREAM CONNECT/ACCEPT fails (the peer sees "no listener for
// session", the caller sees a generic CANT_REACH_PEER). Messaging is dead.
//
// startWiredBridge fixes that by mirroring go-sam-bridge's own reference wiring
// (cmd/sam-bridge/main.go, integration/integration_test.go): start the router
// first, wait for its I2CP port, connect an I2CP client, then build the SAM
// bridge with that client so STREAM sessions get a real transport.
package main

import (
	"context"
	"fmt"
	"log"
	"net"
	"time"

	"github.com/go-i2p/go-i2p/lib/config"
	"github.com/go-i2p/go-i2p/lib/embedded"

	"github.com/go-i2p/go-sam-bridge/lib/embedding"
	"github.com/go-i2p/go-sam-bridge/lib/handler"
	"github.com/go-i2p/go-sam-bridge/lib/i2cp"
	"github.com/go-i2p/go-sam-bridge/lib/session"
	samstreaming "github.com/go-i2p/go-sam-bridge/lib/streaming"
	gostreaming "github.com/go-i2p/go-streaming"
)

// wiredBridge bundles the embedded go-i2p router, the I2CP client that binds
// the SAM handlers to it, and the SAM bridge. All three are torn down together
// (in reverse order of startup) by Stop.
type wiredBridge struct {
	router *embedded.StandardEmbeddedRouter
	client *i2cp.Client
	bridge *embedding.Bridge
}

// i2cpAddr is the loopback I2CP endpoint the embedded router listens on and the
// I2CP client dials. It is fixed at the standard 7654: the go-i2cp client hard-
// codes its dial target to 127.0.0.1:7654 at construction (Tcp.Init) and ignores
// any address override via SetProperty, so this is the only value that actually
// works. One router per host therefore owns 7654; running several independent
// routers on one host is not supported by this dependency (see MIGRATION-i2p).
const i2cpAddr = "127.0.0.1:7654"

// startWiredBridge starts an embedded go-i2p router and a SAMv3 bridge whose
// STREAM handlers are wired to that router over I2CP.
//
// Ordering matters: the router must be listening on I2CP before the client can
// connect, and the client must exist before embedding.New() runs so the handler
// registrar can register a StreamManager per STREAM session.
func startWiredBridge(ctx context.Context, samListen string, debug bool) (*wiredBridge, error) {
	// 1. Start the embedded router with its I2CP server on the standard port.
	routercfg := config.DefaultRouterConfig()
	routercfg.I2CP.Address = i2cpAddr
	router, err := embedded.NewStandardEmbeddedRouter(routercfg)
	if err != nil {
		return nil, fmt.Errorf("create router: %w", err)
	}
	if err := router.Configure(routercfg); err != nil {
		return nil, fmt.Errorf("configure router: %w", err)
	}
	if err := router.Start(); err != nil {
		return nil, fmt.Errorf("start router: %w", err)
	}

	// 2. Wait for the I2CP port to accept connections, then give the router a
	//    moment to finish wiring the server internals.
	if err := waitForTCP(i2cpAddr, 60*time.Second); err != nil {
		_ = router.Stop()
		return nil, fmt.Errorf("i2cp port never came up: %w", err)
	}
	time.Sleep(2 * time.Second)

	// 3. Connect the I2CP client to the embedded router.
	client := i2cp.NewClient(&i2cp.ClientConfig{
		RouterAddr:     i2cpAddr,
		ConnectTimeout: 60 * time.Second,
		SessionTimeout: 120 * time.Second,
	})
	if err := client.Connect(ctx); err != nil {
		_ = router.Stop()
		return nil, fmt.Errorf("connect i2cp client: %w", err)
	}

	// 4. Build the SAM bridge wired to that client. Because the I2CP port is
	//    already bound by our router, embedding.New() will not start a second
	//    one — it takes the "external router" path and wires StreamManagers.
	b, err := embedding.New(
		embedding.WithListenAddr(samListen),
		embedding.WithI2CPAddr(i2cpAddr),
		embedding.WithI2CPProvider(newI2CPProviderAdapter(client)),
		embedding.WithHandlerRegistrar(wiredHandlerRegistrar(client, debug)),
		embedding.WithDebug(debug),
	)
	if err != nil {
		_ = client.Close()
		_ = router.Stop()
		return nil, fmt.Errorf("init bridge: %w", err)
	}
	if err := b.Start(ctx); err != nil {
		_ = client.Close()
		_ = router.Stop()
		return nil, fmt.Errorf("start bridge: %w", err)
	}

	return &wiredBridge{router: router, client: client, bridge: b}, nil
}

// Stop tears down the bridge, I2CP client, and router in reverse order.
func (w *wiredBridge) Stop(ctx context.Context) {
	if w.bridge != nil {
		_ = w.bridge.Stop(ctx)
	}
	if w.client != nil {
		_ = w.client.Close()
	}
	if w.router != nil {
		_ = w.router.Stop()
	}
}

// waitForTCP blocks until addr accepts a TCP connection or timeout elapses.
func waitForTCP(addr string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("tcp", addr, 2*time.Second)
		if err == nil {
			_ = conn.Close()
			return nil
		}
		time.Sleep(500 * time.Millisecond)
	}
	return fmt.Errorf("timeout after %s", timeout)
}

// wiredHandlerRegistrar registers the default SAM handlers plus STREAM handlers
// that are backed by a StreamManager per session, built from the connected
// I2CP client. Mirrors go-sam-bridge/cmd/sam-bridge/main.go.
func wiredHandlerRegistrar(client *i2cp.Client, debug bool) embedding.HandlerRegistrarFunc {
	// Per-session trace of the registration path. Only useful when diagnosing a
	// dead STREAM transport, and it fires for every session, so it is gated on
	// --debug; the failure branches below log unconditionally.
	debugf := func(format string, args ...any) {
		if debug {
			log.Printf("[wired-bridge] "+format, args...)
		}
	}

	return func(router *handler.Router, deps *embedding.Dependencies) {
		// Base handlers first (HELLO, DEST, PING, etc.); the STREAM/SESSION
		// handlers below are re-registered on top with I2CP wiring.
		embedding.DefaultHandlerRegistrar()(router, deps)

		streamConnector := handler.NewStreamingConnector()
		streamAcceptor := handler.NewStreamingAcceptor()
		streamForwarder := handler.NewStreamingForwarder()

		sessionHandler := handler.NewSessionHandler(deps.DestManager)
		sessionHandler.SetI2CPProvider(deps.I2CPProvider)

		sessionHandler.SetSessionCreatedCallback(func(sess session.Session, i2cpHandle session.I2CPSessionHandle) {
			debugf("session created: id=%s style=%s", sess.ID(), sess.Style())

			if sess.Style() != session.StyleStream {
				debugf("session %s: not STREAM, nothing to wire", sess.ID())
				return
			}
			if i2cpHandle == nil {
				log.Printf("[wired-bridge] session %s: no I2CP handle, STREAM will not work", sess.ID())
				return
			}

			i2cpSess, ok := i2cpHandle.(*i2cp.I2CPSession)
			if !ok {
				log.Printf("[wired-bridge] session %s: unexpected I2CP handle type %T, STREAM will not work", sess.ID(), i2cpHandle)
				return
			}

			underlyingSession := i2cpSess.Session()
			underlyingClient := client.I2CPClient()
			if underlyingSession == nil || underlyingClient == nil {
				log.Printf("[wired-bridge] session %s: I2CP session/client missing, STREAM will not work", sess.ID())
				return
			}

			streamManager, err := gostreaming.NewStreamManagerFromSession(underlyingClient, underlyingSession)
			if err != nil {
				log.Printf("[wired-bridge] session %s: NewStreamManagerFromSession: %v", sess.ID(), err)
				return
			}
			adapter, err := samstreaming.NewAdapter(streamManager)
			if err != nil {
				log.Printf("[wired-bridge] session %s: NewAdapter: %v", sess.ID(), err)
				return
			}
			streamConnector.RegisterManager(sess.ID(), adapter)
			if err := streamAcceptor.RegisterManager(sess.ID(), adapter); err != nil {
				log.Printf("[wired-bridge] session %s: acceptor RegisterManager: %v", sess.ID(), err)
			}
			streamForwarder.RegisterManager(sess.ID(), adapter)
			debugf("session %s: stream manager registered", sess.ID())
		})

		router.Register("SESSION CREATE", sessionHandler)
		router.Register("SESSION ADD", sessionHandler)
		router.Register("SESSION REMOVE", sessionHandler)

		streamHandler := handler.NewStreamHandler(streamConnector, streamAcceptor, streamForwarder)
		router.Register("STREAM CONNECT", streamHandler)
		router.Register("STREAM ACCEPT", streamHandler)
		router.Register("STREAM FORWARD", streamHandler)

		if destResolver, err := i2cp.NewClientDestinationResolverAdapter(client, 30*time.Second); err == nil {
			namingHandler := handler.NewNamingHandler(deps.DestManager)
			namingHandler.SetDestinationResolver(destResolver)
			router.Register("NAMING LOOKUP", namingHandler)
		}
	}
}

// i2cpProviderAdapter wraps i2cp.Client to implement session.I2CPSessionProvider.
// Mirrors go-sam-bridge/cmd/sam-bridge/main.go.
type i2cpProviderAdapter struct {
	client *i2cp.Client
}

func newI2CPProviderAdapter(client *i2cp.Client) *i2cpProviderAdapter {
	return &i2cpProviderAdapter{client: client}
}

func (a *i2cpProviderAdapter) CreateSessionForSAM(ctx context.Context, samSessionID string, cfg *session.SessionConfig) (session.I2CPSessionHandle, error) {
	i2cpConfig := &i2cp.SessionConfigFromSession{
		SignatureType:          cfg.SignatureType,
		EncryptionTypes:        cfg.EncryptionTypes,
		InboundQuantity:        cfg.InboundQuantity,
		OutboundQuantity:       cfg.OutboundQuantity,
		InboundLength:          cfg.InboundLength,
		OutboundLength:         cfg.OutboundLength,
		InboundBackupQuantity:  cfg.InboundBackupQuantity,
		OutboundBackupQuantity: cfg.OutboundBackupQuantity,
		FastReceive:            cfg.FastReceive,
		ReduceIdleTime:         cfg.ReduceIdleTime,
		CloseIdleTime:          cfg.CloseIdleTime,
	}
	// Return the handle as-is. Do not wrap it: handler/session.go blocks on
	// handle.WaitForTunnels before answering SESSION STATUS, which is what keeps
	// a STREAM CONNECT from firing into a session whose tunnels are not built
	// yet. A wrapper that stubs WaitForTunnels/IsTunnelReady reintroduces the
	// CANT_REACH_PEER failure #42 fixed, only nondeterministically. It also
	// breaks embedding/handlers.go, which type-asserts the handle to
	// *i2cp.I2CPSession for the datagram/raw paths.
	return a.client.CreateSessionForSAM(ctx, samSessionID, i2cpConfig)
}

func (a *i2cpProviderAdapter) IsConnected() bool {
	return a.client.IsConnected()
}

var _ session.I2CPSessionProvider = (*i2cpProviderAdapter)(nil)
