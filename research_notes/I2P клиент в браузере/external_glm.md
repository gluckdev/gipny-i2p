# External AI worker notes: Catalogue of I2P router implementations and their WASM readiness

> Source: non-Claude worker `opencode/glm-5.3-flash-free` run through opencode on 2026-11-01 with a written brief. Lower confidence than the Claude researcher notes: treat its URLs and figures as unverified unless they match a Claude note or are obviously primary sources; the worker was told to mark missing web access and unverified claims itself (see its first line).

# NOTES: I2P router implementations and their WebAssembly/browser readiness (as of 2026-11-01)

## 1. Web access status

**Working web access confirmed.** The `webfetch` tool successfully returned real page content (GitHub repository and release pages, i2p.net proposal page), and the `websearch` tool returned real search results including page excerpts from crates.io, docs.rs, i2p.net docs and GitHub. A few links could not be retrieved (a 404 on a raw `ROADMAP.md` file, noted below). Claims below cite a URL only where I actually fetched the page. When I used a search-result excerpt (real returned snippet, but the full page was not fetched), the citation is marked `[S]`. Everything hinged on absence-of-evidence is listed under **Unverified**.

Date caveat: GitHub release pages show release dates without a year ("released 20 Oct"). I inferred years by release ordering and by asset timestamps (e.g. i2pd 3.0.0 assets carry `2026-10-21T01:14:31Z`). Daily-precision dates for the four most recent i2pd and the latest Java I2P/go-i2p releases are therefore safe; older ones I only describe as roughly quarterly.

## 2. Summary table

| Implementation | Language | Maturity | Transports | WASM readiness verdict |
|---|---|---|---|---|
| i2pd (PurpleI2P) | C++17/20 | Mature, very active | NTCP2, SSU2 (+ PQ variants) | Blocked (raw sockets, OpenSSL, Boost; no wasm effort found) |
| Java I2P (reference router) | Java | Mature, very active | NTCP2, SSU2 only (legacy removed) | Blocked for true wasm; "prototype possible" via CheerpJ but blocked on sockets |
| go-i2p | Go | Beta ("usable by experts", dev-only releases) | NTCP2 (full), SSU2 (experimental) | Plausible with work (pure-Go crypto helps; sockets/goroutines need replacing) |
| emissary | Rust | Young but working (NTCP2+SSU2, real-world usable) | NTCP2, SSU2 | Plausible with work — best current candidate (pure-Rust crypto, embeddable async core) |
| ire (str4d) | Rust | Pre-alpha, dormant (legacy NTCP/SSU) | NTCP, SSU (legacy) | Blocked (abandoned, obsolete transports) |
| i2p-rs | Rust | Client library only (SAMv3), not a router | n/a (SAM client) | n/a — not a router |
| tachyon-i2p / i2pd-sys | Rust FFI over libi2pd | Pre-0.1 | via embedded libi2pd | Blocked for browser (C++ FFI), but proves the *embedded router* pattern |

## 3. Per-implementation details

### i2pd (PurpleI2P/i2pd) — C++

- Repository: https://github.com/PurpleI2P/i2pd (fetched). BSD-3-clause. ~10,175 commits; open issues were being filed as recently as 2026-10-18 (issues page snippet). 4.2k stars.
- Latest release: **3.0.0**, released 20 Oct 2026 by r4sas (release assets timestamped 2026-10-21) — https://github.com/PurpleI2P/i2pd/releases (fetched). Recent cadence: 2.61.0 (20 Jul 2026), 2.60.0 (25 Apr 2026), 2.59.0 (8 Feb 2026), 2.58.0 (8 Sep 2025).
- Transports: NTCP2 and SSU2, including post-quantum NTCP2 (ML-KEM-512/768/1024) when OpenSSL ≥ 3.5, and SSU2 default crypto type ML-KEM-768 (from 2.59.0/2.61.0 release notes).
- Network layer: Boost.Asio. Dependency notes from fetched release notes: still on Boost (supports boost 1.87 in 2.55.0, boost 1.89 in 2.58.0), but `boost::filesystem` and `boost::date_time` dependencies were removed in 2.54.0 ("Removed dependency from boost::date_time and boost::filesystem"), and 2.56.0 notes mention "Exclude disk operations from SSU2 and NTCP2 threads". OpenSSL (or LibreSSL/BoringSSL family workflows) is central — including OpenSSL 4 support (2.61.0) and post-quantum calls.
- Platform support in README runs to Android (via i2pd-android, which vendors i2pd as a git submodule — verified in Android's `.gitmodules` snippet), iOS, Haiku, Solaris, Windows, macOS, Docker/Snap. No browser/Emscripten target is listed.
- WebAssembly: the README "no LLM/AI policy" exists but nothing about wasm; supported-platforms list has no browser target. I found no Emscripten build discussions in i2pd issues/releases.
- **Verdict: blocked** as a straight port — the transport layer opens raw TCP/UDP sockets (unsupported in browsers), and the crypto lives in OpenSSL. However the codebase is platform-portable C++17 and already ships for Android/iOS; a wasm port would require re-implementing transports on a socket bridge (WebSocket/WebRTC relay or PT-based shim) and linking a wasm-friendly crypto stack. That is a large but not absurd engineering effort; without an existing prototype I do not rate it beyond "possible with heavy work".

### Java I2P (i2p/i2p.i2p) — the reference router

- Repository: https://github.com/i2p/i2p.i2p (fetched). Mirror of the official git at i2pgit.org. 18,608 commits; CI via GitHub Actions.
- Latest release: **2.14.0** (https://github.com/i2p/i2p.i2p/releases/latest, fetched; the i2p.net proposal page header also advertises "2.14.0 released" with a blog path dated 2026-10-20 — https://www.i2p.net/en/proposals/109-pt-transport/, fetched). Release tagging by eyedeekay; releases roughly quarterly (2.11.0 was 2026-02-09 per the releases-page history snippet).
- Transports: NTCP2 and SSU2 only. Legacy NTCP/SSU are documented as obsolete/removed — i2p.net Transport Overview ("There are two transports currently implemented: NTCP2 ... SSU2"), https://i2p.net/en/docs/overview/transport [S], and Java I2P 2.6.0 release text: "Legacy transport protocols are being removed, simplifying the code in the UDP transports" (releases page snippet).
- Network layer: Java NIO for NTCP (per i2p.net legacy NTCP doc description [S]); heavy use of Java threads and a JVM.
- Prerequisites: Java SDK 17+ for full builds (README, fetched). Networking beyond HTTP is via JVM sockets; browser-console webapps are Java servlets (Jetty).
- CheerpJ: I searched specifically for any CheerpJ-based attempt to run the Java I2P router (queries combining CheerpJ/i2p/router). **No such attempt was found.** CheerpJ itself is real and relevant: it is "a WebAssembly-based Java Virtual Machine for the browser" with Java 8/11/17 support and Java multi-threading (https://cheerpj.com/docs/overview.html [S]); networking support is HTTP/HTTPS-centric, with a documented pattern of tunneling other traffic through a Tailscale-like network (Swing Browser tutorial) — i.e., raw TCP/UDP from the router would still need bridging.
- **Verdict: blocked** for a WebAssembly *build* (Java bytecode → wasm is not directly supported; CheerpJ runs bytecode inside a wasm-JVM instead). A CheerpJ prototype of the Java router is technically the most "drop-in" browser experiment because CheerpJ runs threads and the full OpenJDK runtime, but the router's NIO transports cannot open sockets from inside the browser sandbox — they would need a WebSocket/WebRTC bridge running at a relay, which effectively makes the browser a thin client, not a full participant.

### go-i2p (Go)

- Organization: https://github.com/go-i2p (fetched), website goi2p.net [S]. The router repo is https://github.com/go-i2p/go-i2p — described on the org page as "Go implementation of the I2P Router protocol. Usable with NTCP2, I2CP, and I2PControl" and in the org README as "**incomplete but usable by experts**" (https://github.com/go-i2p, fetched).
- Latest release: **v0.1.70001** (8 Sep 2026 per the releases page, https://github.com/go-i2p/go-i2p/releases, fetched), released by eyedeekay with the note "only recommended for developers at this time. The first general-use release will be version 0.2.0". v0.1.67 (1 Jul) introduced "Preliminary support for SSU2". Org listing shows activity into late Oct 2026.
- Transports per pkg.go.dev descriptions: transport manager + NTCP2 (Noise_XK_25519_ChaChaPoly_SHA256, session management) and SSU2 (experimental, with introspection docs `docs/transport-observability.md`) — https://pkg.go.dev/github.com/go-i2p/go-i2p [S]. Router docs state "The core router functionality is now operational" with I2CP, tunnels, NetDB, lease sets; SAMv3.3 exists via separately packaged `go-sam-bridge` [S].
- Notable: an `lib/embedded` API "for embedding I2P routers into Go applications" (pkg.go.dev package descriptions [S], and v0.1.1 release notes, fetched) — an embedding surface that matters for browser work.
- Dependencies: Go standard library + pure-Go crypto + `go-noise` (org's own Noise library, https://github.com/go-i2p, fetched). Contrary to the brief's worry about "Go → hard dependencies", there is no OpenSSL/Boost here.
- WebAssembly: no wasm effort found anywhere. In principle Go cross-compiles to wasm32 targets, but goroutines/the network layer sit on OS sockets; a browser build needs a rewritten I/O glue layer (e.g., a WebSocket NTCP2 shim) and Go 1.21-style wasm threading behind COOP/COEP headers is workable but not free.
- **Verdict: plausible with work.** Pure-Go stack, embedded-router API, active maintainers; maturity is the main constraint (first general-use release 0.2.0 not yet out). A wasm proof-of-concept of library-only features (crypto, netdb, tunnel math) is easy; a full networked router needs transport bridging.

### emissary (Rust) — altonen/emissary → eepnet/emissary

- The repository has moved: crates.io/docs.rs now link to **https://github.com/eepnet/emissary** ("Rust implementation of the I2P protocol stack", 255–260 stars, MIT/Apache-2.0) while older READMEs link to github.com/altonen/emissary (owner user `altonen`). Created 2023-12-31, ~1,032 commits (search-results data).
- Crate `emissary-core` **0.4.0 published 2026-04-12** on crates.io (docs.rs version list); previous: 0.3.1 (2026-02-04), 0.3.0 (2025-12-13), 0.2.0 (2025-04-23), 0.1.0 (2025-04-01) — docs.rs pages [S].
- Structure per README: `emissary-core` = I2P protocol as an **asynchronous library**; `emissary-util` = utilities "such as runtime implementations and reseeder"; `emissary-cli` = **tokio**-based router (cargo-installable). Latest features: **NTCP2 + SSU2 transports**, I2CP + SAMv3 clients, HTTP + SOCKSv5 proxies (older README 0.2/0.3 show NTCP2-only and no SOCKS5, showing recent growth).
- Cryptography is essentially all pure-Rust (crates.io dependency listing [S]): chacha20poly1305, x25519-dalek, sha2/siphash/subtle, zeroize — the dependency descriptions explicitly note wasm compatibility ("Uses a portable pure Rust implementation that works everywhere, even WASM!"). Optional deps include `parking_lot`/`spin` (no_std-friendly locking) and `flate2` (dev).
- WebAssembly: I found no wasm target, no wasm CI build, and no wasm discussion in the README/docs. But the design is the most wasm-shaped of all candidates: protocol logic isolated in an async library crate with runtime supplied from outside (`emissary-util` "runtime implementations"), pure-Rust crypto, no OpenSSL, no Boost.
- What a browser port would actually need: a JavaScript/WebAssembly async runtime replacing tokio (or targeting wasm32-unknown-emscripten, which upstream Rust+wasm-bindgen now supports as an experimental target), and a network socket shim — browsers have no raw TCP/UDP, so NTCP2/SSU2 frames would have to be tunneled over a WebSocket/WebRTC bridge to a relay node, or a new P2P-over-WebRTC transport would have to be defined.
- **Verdict: plausible with work.** The best starting point for a real "router in a PWA" effort.

### Other Rust projects seen in crates.io (inventory per crates.io keyword page [S], docs.rs pages [S])

- **ire** (str4d) — "I2P router implementation in Rust", pre-alpha, single version 0.0.1, legacy NTCP/SSU handshakes only. Dormant. **Verdict: blocked.**
- **i2p / i2p-rs** — SAM-based client library (0.0.1, 2017), https://github.com/i2p/i2p-rs. Not a router; presumes an external one.
- **i2p_snow** (str4d) — pure-Rust Noise with I2P extensions (v0.5.1). Building block, not a router.
- **yosemite** (v0.7.0, cyphernet.io) — SAM-based client library, not a router (keyword-listing evidence only).
- **tachyon-i2p / i2pd-sys** (hacer-bark, codeberg) — Rust FFI wrapper embedding `libi2pd` in-process ("no external i2pd process, no SAM bridge"), versions 0.0.1–0.0.5 Jul–Aug 2026, used by `tachyon-web`'s `Server::serve_i2p` to publish eepsites. Not wasm-relevant (links C++), but shows the embedded-router pattern going mainstream in 2026.

## 4. Browser/WASM experiments found

- **No implementation of I2P router network protocol runs in a browser today.** Independent confirmation: the RingsNetwork documentation (checked 22 Sep 2026 per its own sources note) explicitly marks I2P as **"Browser P2P: ❌"**, with the definition: "Browser P2P means the browser itself connects as a peer; a browser UI or gateway client does not qualify" (https://github.com/RingsNetwork/rings/blob/master/docs/src/README.md [S]). Same table: Tor ❌ browser, WebTorrent ✅.
- Searches for "i2pd emscripten", "i2p wasm", "cheerpj i2p" produced **zero I2P-specific results** — generic emscripten/CheerpJ documentation only. No community attempt to compile i2pd or a Rust/Go router to wasm could be verified (zzz.i2p threads are on the I2P network and invisible to clearnet search; the clearnet mirror i2pforum.net surface no wasm/webassembly thread in my queries).
- What *does* exist under "I2P in the browser" is browser-**bundling**, not in-browser routers: Firefox profiles and profile managers such as PurpleI2P/i2pdbrowser ("i2pd browser bundle") and i2p/i2p.firefox (org page, fetched) — the router still runs natively next to the browser.
- Adjacent prior art: WebTorrent (WebRTC data channels in browsers), libp2p's wasm stack, and wasm-native nodes in RingsNetwork and Skywire, all documenting a "host bridge" pattern for the socket problem (all [S]). A browser I2P router faces the same three hard problems: no raw TCP/UDP (needs WebRTC/WebSocket bridging), no persistent filesystem (IndexedDB/OPFS), and threading under COOP/COEP.

## 5. WebSocket / pluggable transport support (direct question)

**No.** None of the implementations shipped a WebSocket-based router transport as of this research date:

- The official transport model has exactly **two** transports (NTCP2 over TCP, SSU2 over UDP); "Additional transports may be developed, including: a TLS/SSH look-alike transport; an 'indirect' transport …; Tor-compatible pluggable transports" — i.e., a PT interface is listed as future work at https://i2p.net/en/docs/overview/transport [S].
- The concrete proposal, **Prop 109 "PT Transport"**, exists and I fetched it: an I2P transport that connects to other routers through Tor-style Pluggable Transports, layered as (1) generic PT over SOCKS/ExtORPort, (2) generic NTCP-over-PT, (3) specific NTCP-xxxx PT binaries — https://www.i2p.net/en/proposals/109-pt-transport/ (fetched). It is a proposal for experimentation, and there was no evidence anywhere of an implemented, shipped PT layer in any codebase I saw.
- Consequence for a browser PWA: there is **no pluggable-transport interface one could implement with WebSocket/WebRTC today**. The practical path is the transport-manager abstraction implementations already have (go-i2p and i2pd describe a transport manager/muxer over NTCP2+SSU2): adding a third, relay-bridged transport behind that manager is the natural, if unbuilt, extension point. SAMv3 (TCP to localhost) is also a stable client interface a wasm router could re-expose over WebSocket — but that serves clients, not peer links.

## 6. Unverified

- **CheerpJ + Java I2P:** the absence of any attempt is an absence-of-evidence conclusion from searches, not a proof that one never happened (zzz.i2p forum threads are on the I2P network and invisible to clearnet search).
- **go-i2p ROADMAP.md:** the org page links `go-i2p/ROADMAP.md`, but my raw fetches of that path returned 404 (both `master` and `main` branches). I therefore do not know what the roadmap says about wasm/network work.
- **Exact year labels** for i2pd releases 2.58.0 and older and for go-i2p v0.1.57 and older are inferred from release ordering/quarterly cadence, not explicit in the fetched pages.
- **yosemite crate nature** (SAM client library for I2P) is taken only from a crates.io keyword-page snippet.
- **ether the "altonen → eepnet" move reason and date:** the redirect is observable (old README links vs new crates.io links), but I did not find a statement of why/when.
- **Star counts and commit counts** (e.g., emissary 255–260 stars) shifted between search snapshots; treat as approximations.- Anything about actual *performance* or *memory footprint* of any router under wasm — no data exists at all; all verdicts are architectural, not measured.

## 7. Sources (URLs actually fetched with webfetch → real content)

1. https://github.com/PurpleI2P/i2pd — repo README, build platforms, no-AI policy
2. https://github.com/PurpleI2P/i2pd/releases — 3.0.0 (20 Oct 2026), 2.61.0, 2.60.0, 2.59.0, 2.58.0, 2.57.0, 2.56.0, 2.55.0, 2.54.0, 2.53.1 release notes
3. https://github.com/i2p/i2p.i2p — repo README (Java 17, Ant/Gradle), structure
4. https://github.com/i2p/i2p.i2p/releases/latest — Release 2.14.0 tag page
5. https://github.com/go-i2p — organization page: go-i2p router status, lib list, goi2p.net
6. https://github.com/go-i2p/go-i2p/releases — v0.1.70001 (Sep 2026) and history incl. v0.1.1 embedded API notes
7. https://www.i2p.net/en/proposals/109-pt-transport/ — PT Transport proposal + "2.14.0 released" header

Search-result snippets consulted `[S]` (real pages, excerpts returned by the web search tool):

- https://i2p.net/en/docs/overview/transport (two-transports statement; future-work list)
- https://pkg.go.dev/github.com/go-i2p/go-i2p (NTCP2 full / SSU2 experimental; embedded package)
- https://docs.rs/crate/emissary-core/0.4.0 (version history, features), https://docs.rs/crate/tachyon-i2p/0.0.2, https://docs.rs/tachyon-web (embedded libi2pd)
- https://crates.io/keywords/i2p (crate inventory: ire, i2p-rs, i2p_snow, i2pd-sys, tachyon-i2p, yosemite, emissary-core)
- https://cheerpj.com/docs/overview.html and related CheerpJ pages (Java-in-browser JVM, Java 8/11/17, threading, networking model)
- https://github.com/RingsNetwork/rings/blob/master/docs/src/README.md (browser-P2P comparison matrix, source-checked 22 Sep 2026)
- https://github.com/PurpleI2P/i2pd-android/…/.gitmodules snippet (i2pd vendored for Android)
- GitHub issue pages for i2pd (issue-activity dates, e.g. #2601 opened Oct 18, 2026)
- https://i2p.github.io/i2p.i2p/ (Java package overview incl. transport package layout)
