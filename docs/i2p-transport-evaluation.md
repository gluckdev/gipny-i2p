# go-i2p as a transport: what works, what does not, and what it costs

Written 2026-07-19 after a full day of debugging why no message ever reached its
destination. Records what was measured, so none of it has to be rediscovered.

**Verdict: go-i2p cannot currently carry gipny traffic, and the rest of the
stack is fine.** Four defects sat on top of each other. Three were ours and are
fixed. The fourth is upstream, is not a single bug, and blocks everything:
client tunnels never finish building, so there is no anonymous transport at all.

This is no longer an inference. Swapping only the router — same relay, same
bots, same harness, same code — the end-to-end test **delivers 5 of 5 messages
over live i2p in 49 seconds** (run 29693889188, `.github/workflows/e2e-i2pd.yml`):

```
| messages sent   | 5       |
| echoes received | 5       |
| RTT median      | 5798 ms |
| total elapsed   | 49108 ms |
[e2e] SUCCESS — all 5 messages delivered and echoed
```

Against go-i2p the same harness has never delivered a single message.

## The four layers

### 1. The SAM bridge had no I2CP transport — ours, fixed

`embedding.New` starts the embedded router's I2CP *server* but never connects an
I2CP *client*, so `createStreamManagerCallback` bails and no StreamManager is
registered per session. `SESSION CREATE` returned OK while every
`STREAM CONNECT`/`ACCEPT` failed with a generic `CANT_REACH_PEER`.

Fixed in `i2p-router/wiring.go` (`97788e6`) by mirroring go-sam-bridge's own
reference wiring: start router → wait for I2CP port → connect client → build the
bridge with `WithI2CPProvider` and a registrar that registers a StreamManager per
STREAM session.

### 2. The router kills idle I2CP connections after 30 s — upstream, worked around

The I2CP server arms a 30 s read deadline *before* the header read
(`go-i2p lib/i2cp/protocol.go:175,400`), so it also bounds the idle gap between
messages, and treats the timeout as a fatal read error. go-i2cp sends no
keepalives. The client connected at startup is therefore always dead by the time
the first SAM session is created — which is every real case.

Worse, the failure is invisible: the `CreateSession` write fails, the SAM handler
returns an error, and go-sam-bridge closes the SAM control socket **without a
`SESSION STATUS`**. The Rust side sees a bare EOF, reported by `yosemite` as
`invalid message from router` — a message that says nothing about the cause and
cost hours to trace.

Reported as [go-i2p#54](https://github.com/go-i2p/go-i2p/issues/54). Worked
around in `e0a3826`, then made obsolete by layer 3: per-session clients connect
milliseconds before use, so the deadline cannot fire in between.

Note `IsConnected()` cannot be used to detect this. The client's read loop never
observes the close and keeps reporting the link as up while every write fails.

### 3. One I2CP client can only ever have one working session — ours, fixed

go-sam-bridge's reference wiring shares one I2CP client across all sessions.
Against this router that caps the bridge at a single working SAM session. Run
29687964736 caught it exactly: the router wrote `SessionStatus` for both sessions
to the same socket and go-i2cp logged reading only the first — and the line it
omits is emitted *before* dispatch, so the message was never read at all.

Without that status the second session's `tunnelReady` is never signalled,
`WaitForTunnels` blocks, and the bridge's 60 s command deadline closes the SAM
socket before any reply. The relay always worked; every bot always failed.

Fixed in `18edc66`: each SAM session gets its own I2CP client. All dial
127.0.0.1:7654 (go-i2cp permits nothing else), so this stays inside the
one-router-per-host constraint. Verified locally (2/2 sessions) and in CI (7
sessions, relay + both bots).

Known gap: session clients are closed on shutdown via `CloseAll`, but not on
`SESSION REMOVE` — the bridge exposes no hook. A long-lived router churning
sessions will accumulate them.

### 4. Client tunnels never build — upstream, unresolved, blocks everything

Reported as [go-i2p#55](https://github.com/go-i2p/go-i2p/issues/55).

Over ~13 minutes with `DEBUG_I2P=debug`:

| | |
|---|---|
| NTCP2 Noise handshakes completed | 124 |
| netdb peers available after filtering | 63 |
| tunnel builds failing `no transports available` | 29 |
| `Tunnel build retry failed` | 8 |
| `timeout_waiting_for_tunnels` | one per session |
| **LeaseSets published** | **0** |

The router is otherwise healthy — it reseeds, keeps a populated netdb, and talks
to dozens of peers. But peer selection picks tunnel hops purely from netdb
RouterInfo (`lib/netdb/std_peer_selection.go` — advertised addresses, caps,
staleness, PeerTracker score) and never asks the transport layer whether a
session exists. The gateway is dialled only when the build request is sent
(`lib/i2np/tunnel_manager_build.go:800,853`); when that fails,
`lib/tunnel/pool.go:903` string-matches the error, marks the peers failed, and
retries with the same blind criteria.

i2pd avoids this by taking the first hop from its transport peer set
(`TunnelPool.cpp:598-620`, including an explicit "Can't select first hop for a
tunnel. Trying already connected" fallback). `Transports::GetRandomPeer`
(`Transports.cpp:1215`) draws from `m_Peers` — routers with an established
session.

## Control experiment: i2pd, same machine, same minute

i2pd 2.60.0, SAM on the same port, driven by the same script (`tools/`):

```
20:38:59 Tunnel: Outbound tunnel 61550957 has been created     # ~4 s after start
20:39:00 Tunnel: Inbound tunnel 3107694909 has been created

>>> SESSION CREATE STYLE=STREAM ID=eeptest DESTINATION=<priv>
<<< [6.0s] SESSION STATUS RESULT=OK
>>> STREAM CONNECT ID=eeptest DESTINATION=<i2p-projekt.i2p>
<<< [3.5s] STREAM STATUS RESULT=OK
    HTTP/1.1 302 Moved Temporarily
    Location: /en/
    nginx/1.24.0 (Ubuntu)
```

20 tunnels in its first two minutes. Same uplink, same script. The go-i2p
behaviour also reproduces on a GitHub Actions runner, so it is not specific to
one network.

## Control experiment 2: the whole product over i2pd

`.github/workflows/e2e-i2pd.yml` is `e2e.yml` with the router swapped and
nothing else changed — same relay binary, same bots, same harness, same
`GIPNY_SAM_PORT=7656` shared-router arrangement.

| | go-i2p | i2pd 2.60.0 |
|---|---|---|
| tunnels | never ready | seconds |
| real eepsite | unreachable | fetched |
| **e2e messages delivered** | **0 of 5, ever** | **5 of 5** |

Bot startup ~15 s, relay connect 13–19 s, RTT ~5.8 s median. Those are ordinary
i2p numbers.

This also confirms the three fixes above were necessary rather than incidental:
both bots attach to one shared router here too, which only works because each
SAM session gets its own I2CP client.

Setup notes worth keeping: install the `.deb` with `apt-get install -y ./file`
rather than unpacking it — the runtime dependency list (boost, miniupnpc, …) is
whatever that build links against and does not converge if fetched by hand. Pick
the build matching the runner's Ubuntu codename so apt can satisfy it; the
binary is at `usr/bin/i2pd`.

## The first-hop patch experiment (not shipped)

`docs/patches/go-i2p-first-hop-selection.patch` applies i2pd's idea to go-i2p:
transports gain `ConnectedPeers()`, the muxer aggregates it, and
`StdNetDB.SelectPeers` promotes or substitutes an already-connected peer into the
gateway slot.

Measured against v0.1.59999:

| | before | after |
|---|---|---|
| `timeout_waiting_for_tunnels` | 2 of 2 sessions | 0 |
| `Tunnel build retry failed` | 8 | 0 |
| `no transports available` | 29 | 2 |

But the eepsite still did not load, and the layers underneath surfaced:

- `i2p-projekt.i2p` → `CANT_REACH_PEER "invalid destination format"`. go-i2p
  supports ECIES destinations only; ElGamal is unimplemented, and that
  destination is of the older form (516 chars vs 524).
- `stats.i2p` → `TIMEOUT "wait for SYN-ACK"` after 60 s. Real progress — the SYN
  went out through tunnels — but nothing came back.
- All six client tunnel builds still expired (`Cleaned up expired tunnel build
  via timeout`, 90 s each). The only tunnels that completed were **zero-hop
  exploratory** ones (`is_client_tunnel=false`), which is normal at startup.

**Do not ship this patch as written.** i2pd gates the same behaviour on having
more than 100 connected peers (25 for inbound). This version has no threshold,
and in the test run only 4 peers were connected — so every tunnel was pinned to
the same 4 gateways, chosen by accident of connection order. See the security
section.

## Security implications

Stated plainly, because this is a messenger that promises privacy.

**Unverified but likely: the published LeaseSet may expose our IP.** With the
patch, `timeout_waiting_for_tunnels` stopped firing and `Publishing all
LeaseSets` appears in the log — while the only completed tunnels were zero-hop.
A LeaseSet lists the gateways of inbound tunnels; for a zero-hop tunnel that
gateway is *us*. netdb is public and floodfills serve LeaseSets to anyone. This
was inferred from logs, not confirmed by inspecting a published LeaseSet —
**worth confirming before anyone runs this against the real network.**

**The first-hop patch collapses gateway diversity.** Pinning the first hop is
not wrong in itself — I2P deliberately limits how many routers learn your IP —
but it must be a deliberate set of adequate size, which is why i2pd gates it on
peer count. With 4 connected peers, a single hostile one among them sees
essentially all our traffic, indefinitely.

**go-i2p is fingerprintable by its own admission.** The project README says it
"is probably very distinct on the network". In an anonymity network, being
distinguishable is itself a deanonymisation risk, and no fix of ours changes it —
anonymity is a property of the crowd you blend into.

**Unimplemented crypto narrows the network.** No ElGamal (ECIES only), and
`messageReliability=Guaranteed` silently degrades to BestEffort.

None of gipny's own fixes from today touch anonymity: I2CP wiring, reconnect and
per-session clients are plumbing about whether bytes arrive, not about who can
see them.

## Reproduction

`tools/sam-probe.py` — two SAM sessions on one router; catches layer 3.
`tools/sam-eepsite.py` — fetch a real eepsite by base64 destination; catches
layer 4 and is the decisive test, because it removes gipny entirely from the
question.

Both need a router with SAM on 127.0.0.1:7656 and, for the eepsite test, an
`hosts.txt` (the official one ships in the I2P source tree at
`installer/resources/hosts.txt`).

Run the router with `DEBUG_I2P=debug` — the `--debug` flag only reaches
go-sam-bridge's embedding options and leaves go-i2p's logger at `io.Discard`.
Expect ~300 MB of log for a few minutes of runtime.

`NAMING LOOKUP` for a `.b32.i2p` address returns `KEY_NOT_FOUND` in 0.0 s — no
netdb lookup is attempted — which is why the reproduction scripts carry base64
destinations instead of names.

## Open decisions

1. Confirm or rule out the LeaseSet exposure above.
2. Evaluate i2pd as the shipped transport. Desktop is a binary swap — the Rust
   side speaks plain SAMv3 and does not change. Android is the real cost: i2pd is
   C++ with boost and OpenSSL across four ABIs, against a Go router that
   cross-compiles trivially today.
3. Until a transport actually delivers, README and release notes should not imply
   working anonymous messaging.
