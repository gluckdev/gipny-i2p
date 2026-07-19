# gipny: Tor → i2p migration

This fork replaces the network transport of gipny from **Tor (embedded Arti)**
to **i2p**, using the pure‑Go **go‑i2p** router (via its SAMv3 bridge) bundled as
a sidecar. Everything else — X3DH + Double Ratchet crypto, SQLCipher vault,
sealed‑sender relay protocol, DB, UI, bot‑sdk — is unchanged. The node address
(historically an `.onion`) is treated as an opaque string that now carries an i2p
destination.

> Status: transport core done and the bundled router is verified to build and
> answer SAMv3 (`HELLO REPLY RESULT=OK VERSION=3.3`). Client addresses are
> **ephemeral per session** (the relay routes by the ed25519 key, not by i2p
> address), the identity card shows a `.b32.i2p` short form, and the client runs
> outbound-only (`publish: false`). Android embeds the router in-process through
> JNI; CI builds an installable debug APK on every push (artifact
> `gipny-android-debug-apk`) and remains experimental pending a broader
> real-device test matrix. A real i2p relay still has to be deployed and its
> destination baked in (see “Deploy a relay”) — meanwhile a relay address can be
> set at runtime in Settings.

---

## Why this shape

- **i2p needs a running router.** Unlike Arti (a library compiled into the app),
  i2p routing lives in a separate router process exposing a **SAMv3** TCP API.
- **go‑i2p** is a pure‑Go router; its **`go-sam-bridge`** has an *embedded router*
  library (`lib/embedding`) that starts the router in‑process and serves SAM. We
  wrap it in a tiny binary (`i2p-router/`, `gipny-i2p-router`) — one self‑contained
  Go binary providing both router and SAM, so the app stays “zero install”.
- The Rust side speaks SAMv3 with the **`yosemite`** crate (async/tokio).
- **I2CP wiring (why the wrapper is not one line).** `embedding.New` starts the
  router's I2CP *server* but never connects an I2CP *client*, so out of the box
  SAM STREAM sessions have no transport: `SESSION CREATE` returns OK, yet every
  `STREAM CONNECT`/`ACCEPT` fails (`no listener for session`, surfaced to the
  caller as a generic `CANT_REACH_PEER`). The wrapper (`i2p-router/wiring.go`)
  therefore starts the router, waits for its I2CP port, connects an `i2cp.Client`,
  and builds the SAM bridge wired to it — mirroring go-sam-bridge's own
  `cmd/sam-bridge`. Caveat: the underlying `go-i2cp` client hardcodes its dial
  target to `127.0.0.1:7654` (its TCP address is fixed at construction and
  `SetProperty` cannot move it), so **exactly one router per host** is supported.
  Running several independent routers on one machine (e.g. an all‑in‑one e2e job
  with relay + two bots) needs a shared‑router redesign — see issue #42.

```
gipny (Rust) ──SAMv3 127.0.0.1:7656──▶ gipny-i2p-router (Go)
                                         ├─ embedded go-i2p router (I2CP :7654)
                                         ├─ i2cp.Client ⇄ router (wires STREAM transport)
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
  with `CGO_ENABLED=0` for every target. For Android the same codebase is
  compiled as a JNI library instead: `android_export.go` (cgo exports) +
  `jni_shim_android.c` (`JNI_OnLoad` / `Java_…` glue), built with
  `buildmode=c-shared` into a per-ABI `libgipnyi2p.so` by the
  `buildGoRouterJniLibs` Gradle task
  (`core/gen/android/buildSrc/…/GoRouterTask.kt`).
- **`core/tauri.android.conf.json`** — Android platform config override:
  `bundle.resources = []`, so the desktop sidecar binary is not packaged into
  the APK (Android runs the router in-process; the resource glob would
  otherwise fail the android build).
- **`libcore/src/router.rs`** — `RouterHandle` lifecycle: spawns/​supervises the
  router child (desktop) or `attach`es to an in‑process one (Android), picks a
  free SAM port, and `probe_sam()` waits for `HELLO … RESULT=OK`.
- **`.github/workflows/build.yml`** — compile CI (router matrix + Rust + UI).
- **`.github/workflows/release.yml`** — full release pipeline on Actions.

### Rewritten
- **`libcore/src/net.rs`** — `TorNode` → **`I2pNode`** (same public API; a
  `pub type TorNode = I2pNode;` alias keeps callers untouched). One SAMv3 STREAM
  session bound to a fresh **ephemeral destination** (regenerated every
  session, never persisted; see “Network identity” below) with
  `publish: false` — no LeaseSet publish and no inbound tunnels, which speeds
  up cold start; outbound via detached SAM
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
- **Android** — Gradle cross-compiles the Go router as a per-ABI JNI library
  (`libgipnyi2p.so`); `GipnyService.kt` loads it, starts SAM on
  `127.0.0.1:7656` off the main thread and keeps it alive in the foreground
  service; the Rust side connects with `RouterHandle::attach` instead of
  spawning a child. In-app updates fetch `android-apk-<arch>` artifacts from
  the update-server manifest (no Play Store).

### Deliberately unchanged
`crypto.rs`, `db.rs` (schema, incl. the `onion_address`/`onion` columns —
opaque), `security.rs`/vault, `session.rs` message logic, relay wire protocol,
the whole message/attachment/group flow. The address is just a string.

---

## Network identity: ephemeral per session

Early revisions persisted the SAM keypair (`i2p/dest.key` / `dest.pub`) for a
stable address; that design was replaced. On every start the node calls SAM
`DEST GENERATE` (via `yosemite::RouterApi::generate_destination`) and uses the
destination **for that session only**: nothing touches disk, the private key
blob lives in `Zeroizing` memory and is kept in-session solely so `recreate`
can rebuild the SAM session after a router hiccup.

The stable identity is the ed25519/x25519 pair in the encrypted vault — the
relay routes by that key, not by i2p address, so regenerating the address is
free and unlinks sessions at the network layer.

The identity card shows both the full base64 destination and the short
`.b32.i2p` form — `base32(sha256(binary_destination)).b32.i2p`, computed in
`I2pNode::b32_address`.

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
All builds run on GitHub Actions:
- `build.yml` — compile check on every push/PR: router matrix, Rust workspace,
  UI typecheck, plus an **android APK** job that builds a debug APK, asserts
  `libgipnyi2p.so` is packaged, and uploads the APK as the
  `gipny-android-debug-apk` artifact.
- `release.yml` — on a `v*` tag: AppImage/deb + NSIS + signed Android arm64
  APK (router bundled on desktop, JNI-embedded on Android).
- `codeql.yml` — security scanning (rust / js-ts / actions).
- `dependabot.yml` — weekly grouped updates for every ecosystem. Breaking bumps
  for `core/relay` are pinned until the migration issue lands (bincode ≥2 is
  never taken — 3.0.0 on crates.io is a `compile_error!` stub).

Android note: the vendored OpenSSL build (SQLCipher) expects binutils-style
`<triple>-ranlib`/`-ar` names that NDK r23+ no longer ships; the workflows
symlink them to `llvm-ranlib`/`llvm-ar` before building.

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

## Done since the initial migration
- ~~Compute `.b32.i2p` short address~~ — shown in the identity card.
- ~~A relay‑address setter in Settings~~ — overrides the baked-in
  `DEFAULT_RELAY` at runtime.
- ~~Client outbound‑only mode~~ — `publish: false` is now the default.
- ~~Ephemeral per-session address~~ — replaced the persisted `dest.key`.
- ~~Android JNI embedding~~ — router runs in-process; debug APK in CI.
- ~~relay: migrate to current major deps and commit its lockfile~~ — bincode 2,
  rand 0.10, ed25519-dalek 3, thiserror 2, rusqlite 0.40.
- ~~x86_64 Android APK for emulator testing~~ — debug APK + emulator smoke
  job in CI; the main workspace (`libcore`/`core`/`bot-sdk`) crypto/serde
  stack migration is still open (issue #31).

## Follow‑ups / ideas
- Deploy the canonical relay + update server and bake in their destinations
  (`DEFAULT_RELAY`, `DEFAULT_UPDATE_ONION`).
- Expand Android validation across physical devices and additional ABIs.
- Large‑file throughput: bump outbound tunnel quantity during transfers.
