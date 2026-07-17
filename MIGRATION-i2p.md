# gipny: Tor → i2p migration

This fork replaces the network transport of gipny from **Tor (embedded Arti)**
to **i2p**, using the pure‑Go **go‑i2p** router (via its SAMv3 bridge) bundled as
a sidecar. Everything else — X3DH + Double Ratchet crypto, SQLCipher vault,
sealed‑sender relay protocol, DB, UI, bot‑sdk — is unchanged. The node address
(historically an `.onion`) is treated as an opaque string that now carries an i2p
destination.

> Status: transport core done and the bundled router is verified to build and
> answer SAMv3 (`HELLO REPLY RESULT=OK VERSION=3.3`). A real i2p relay still has
> to be deployed and its destination baked in (see “Deploy a relay”). Android
> in‑process router embedding (JNI) is pending.

---

## Why this shape

- **i2p needs a running router.** Unlike Arti (a library compiled into the app),
  i2p routing lives in a separate router process exposing a **SAMv3** TCP API.
- **go‑i2p** is a pure‑Go router; its **`go-sam-bridge`** has an *embedded router*
  library (`lib/embedding`) that starts the router in‑process and serves SAM. We
  wrap it in a tiny binary (`i2p-router/`, `gipny-i2p-router`) — one self‑contained
  Go binary providing both router and SAM, so the app stays “zero install”.
- The Rust side speaks SAMv3 with the **`yosemite`** crate (async/tokio).

```
gipny (Rust) ──SAMv3 127.0.0.1:7656──▶ gipny-i2p-router (Go)
                                         ├─ embedded go-i2p router
                                         └─ SAMv3 bridge
```

**Risks (accepted):** go‑i2p is early‑stage (“probably not safe yet”); its
streaming is a *prototype* — the exact thing our long‑lived relay stream relies
on. First connection is slower than Tor (reseed + tunnel build). The design stays
router‑agnostic (plain SAMv3), so **i2pd can be dropped in** on the same port if
go‑i2p proves unstable.

---

## What changed, file by file

### New
- **`i2p-router/`** — the bundled Go router wrapper (`main.go`, `go.mod`,
  `go.sum`). Flags: `--sam-listen 127.0.0.1:7656 --data <dir>`. Pure Go, builds
  with `CGO_ENABLED=0` for every target.
- **`libcore/src/router.rs`** — `RouterHandle` lifecycle: spawns/​supervises the
  router child (desktop) or `attach`es to an in‑process one (Android), picks a
  free SAM port, and `probe_sam()` waits for `HELLO … RESULT=OK`.
- **`.github/workflows/build.yml`** — compile CI (router matrix + Rust + UI).
- **`.github/workflows/release.yml`** — full release pipeline on Actions.

### Rewritten
- **`libcore/src/net.rs`** — `TorNode` → **`I2pNode`** (same public API; a
  `pub type TorNode = I2pNode;` alias keeps callers untouched). One SAMv3 STREAM
  session bound to a **persistent destination**; outbound via detached SAM
  streams (concurrent dials); optional inbound via `STREAM FORWARD`
  (`GIPNY_I2P_ACCEPT=1`, off by default since the app is relay‑mediated).
  Deleted: SOCKS5 provider, `ProxyConfig`/`ProxyKind`, Arti bootstrap, onion
  keystore handling. `NetError::Tor` → `NetError::I2p`.
- **`core/relay/src/main.rs`** — relay server now opens a persistent yosemite
  `Session` (publish=true) and accepts SAM streams; `handle_client`/`client_loop`
  are byte‑stream generic and **unchanged**. Prints its i2p destination on start.

### Edited (mechanical / wiring)
- **`core/src/lib.rs`** — boot() starts `I2pNode` directly; resolves the bundled
  router via the Tauri resource dir (`GIPNY_I2P_BIN`); **all proxy plumbing
  removed** (DTO, `From` impls, `SETTING_PROXY`, `read_proxy_config`,
  `get/set_proxy_config` commands + their `invoke_handler` entries,
  `start_tor_with_proxy_fallback`).
- **`bot-sdk/src/lib.rs`** — `TorNode::start(dir, None)` → `TorNode::start(dir)`.
- **`libcore/src/{relay,update}.rs`** — `DEFAULT_RELAY` / `DEFAULT_UPDATE_ONION`
  are now **empty placeholders** (old `.onion`s are invalid on i2p) — fill after
  deploying (see below).
- **`libcore/src/session.rs`, `core/src/core.rs`** — relay loops skip quietly
  when no relay is configured (empty address) instead of hammering.
- **Cargo**: removed `arti-client`/`tor-*`/`async-trait`/(libcore)`futures`; added
  `yosemite` (workspace + relay). `core` no longer depends on transport crates.
- **UI** (`ui/src/*`): removed the outer‑proxy settings + `ProxyConfig` types and
  `get/set_proxy_config` calls; relaxed the add‑contact address validation from
  `.endsWith('.onion')` to “`.i2p` or a full base64 destination”; boot log/stage
  copy Tor → i2p (`BootStage` `'tor'` → `'i2p'`). Address fields/labels are
  opaque and otherwise unchanged.
- **`core/tauri.conf.json`** — bundles `resources/gipny-i2p-router*`.
- **Android** `GipnyService.kt` — notification copy “tor” → “i2p”.

### Deliberately unchanged
`crypto.rs`, `db.rs` (schema, incl. the `onion_address`/`onion` columns —
opaque), `security.rs`/vault, `session.rs` message logic, relay wire protocol,
the whole message/attachment/group flow. The address is just a string.

---

## Persistent identity

On first run the node calls SAM `DEST GENERATE` (via
`yosemite::RouterApi::generate_destination`) and stores:
- `…/i2p/dest.key` — private key blob (`0600`), reused across restarts →
  **stable address**.
- `…/i2p/dest.pub` — the shareable public destination (this is the “onion”
  string put in contact cards / shown as “my address”).

The address is currently the **full base64 destination** (long but correct with
`connect()`). A `.b32.i2p` short form is a nice‑to‑have (not yet computed).

---

## Build & run

### Router (standalone, for testing)
```bash
cd i2p-router
CGO_ENABLED=0 go build -o gipny-i2p-router .
./gipny-i2p-router --sam-listen 127.0.0.1:7656 --data ./router-data
# verify:  printf 'HELLO VERSION MIN=3.0 MAX=3.3\n' | nc 127.0.0.1 7656  → RESULT=OK
```

### App
The Rust app auto‑spawns the bundled router. Override its path with
`GIPNY_I2P_BIN=/path/to/gipny-i2p-router`. Two local profiles each spawn their own
router on a free port and share one relay.

### CI / releases
All builds run on GitHub Actions — `build.yml` (compile check) and `release.yml`
(AppImage/deb + NSIS, router bundled). Android is built experimental‑only.

---

## Deploy a relay (required before it works end‑to‑end)

1. On a server, run go‑i2p (or i2pd) exposing SAMv3 on `127.0.0.1:7656`.
2. Run `gipny-relay` (`GIPNY_RELAY_DATA=/var/lib/gipny-relay`). It prints
   `I2P DESTINATION: <base64>`.
3. Paste that into `libcore/src/relay.rs::DEFAULT_RELAY` and rebuild. (Same for
   an update server → `libcore/src/update.rs::DEFAULT_UPDATE_ONION`.)
4. Wire the relay’s router as a companion systemd unit (`After=/Wants=`) in
   `core/relay/gipny-relay.service`.

Until a relay destination is baked in (or set at runtime), clients start and get
their i2p address but have no relay to reach — messaging is idle by design.

---

## Applied optimizations
- **SAM `gzip: false`** on client and relay sessions — payloads are already
  E2E‑encrypted and padded to fixed buckets; SAM gzip only burns CPU and blurs
  the uniform size classes.
- **Detached concurrent dials** (`yosemite` `async-extra`) — outbound streams
  don’t serialize behind one session lock.

## Follow‑ups / ideas
- Compute `.b32.i2p` short address for nicer cards/QR.
- A relay‑address setter in Settings (since `DEFAULT_RELAY` ships empty).
- Client outbound‑only mode (`publish:false`, fewer inbound tunnels) — faster
  cold start, since the client never accepts (relay‑mediated). Behavior change,
  left off for parity.
- Android: embed the go router via JNI/gomobile started from `GipnyService`.
- Large‑file throughput: bump outbound tunnel quantity during transfers.
