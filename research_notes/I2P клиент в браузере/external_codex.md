# External AI review: independent architecture recommendation for a browser-native I2P client

> Source: OpenAI Codex CLI (model gpt-6.1-sol, reasoning xhigh) with live web search, run on 2026-11-01 with the full-question brief. Medium confidence: an independent second opinion with its own sources; where it conflicts with a Claude researcher note, prefer the note that cites the primary source and say so. The review's own 'Unverified claims' section lists what it could not confirm.

# Web access status

**Live web search worked.** Research date: **1 November 2026**. Sources below were consulted through live search or page retrieval. Project-specific facts supplied in the brief are treated as inputs, not independently audited.

Status labels distinguish **production** capabilities, **demonstrated** experiments, **theoretical** designs, and **blocked** requirements. A working compiler target does not establish a working browser router.

# Executive recommendation

**Pursue the browser client, beginning with an Emissary-based spike and retaining i2pd/Emscripten as the fallback.** Emissary’s explicit runtime abstraction is a better starting point for browser networking than emulating an operating system beneath i2pd. Its production readiness still needs validation. [Emissary embedding documentation](https://eepnet.github.io/emissary/embedding-rust.html)

The proposed architecture is:

**PWA UI → browser-owned Rust/WASM messaging core and router → WSS transport endpoint → I2P.**

Keep keys, tunnel construction, destination sessions, message encryption, and storage in the browser. Initially use an authenticated byte forwarder on the owner’s agent; subsequently evaluate native NTCP2-over-WebSocket listeners.

Two qualifications are fundamental:

- **Closed-app continuous routing is blocked as a dependable PWA feature.** Use the existing offline-mail relay and reconnect on foregrounding. Browser lifecycle management can suspend or discard execution. [Chrome lifecycle documentation](https://developer.chrome.com/docs/web-platform/page-lifecycle-api)
- **An absolute guarantee against malicious hosting is incompatible with ordinary host-delivered executable code.** Pages can receive no conversation traffic during normal operation, but compromised application updates could access unlocked conversations. [WebCrypto security considerations](https://www.w3.org/TR/webcrypto/)

# Findings

## 1. Router implementation path

**i2pd/Emscripten: theoretically viable, substantial porting work.** OpenSSL running through Emscripten has been demonstrated; zlib is an Emscripten port. These dependencies are not the decisive obstacle. [OpenSSL browser implementation](https://github.com/cryptool-org/openssl-webterm), [Emscripten settings](https://emscripten.org/docs/tools_reference/settings_reference.html)

Networking and scheduling are harder. Emscripten explicitly describes its POSIX-over-WebSocket emulation as incomplete. Its fuller socket-call proxy requires pthreads and is described as potentially slow and mainly useful for testing. An Asio-related issue documents an infinite `select()` loop consuming CPU: successful compilation must therefore be followed by idle-power testing. [Networking documentation](https://emscripten.org/docs/porting/networking.html), [Asio/Emscripten issue](https://github.com/emscripten-core/emscripten/issues/25759)

Pthreads require SharedArrayBuffer and cross-origin isolation. Given the supplied Pages hosting constraint, ordinary header configuration is unavailable. `coi-serviceworker` demonstrates injecting COOP/COEP through a service worker and reloading the first visit; treat this as a compatibility dependency requiring installed-iOS testing and integration with the PWA’s existing worker. [Emscripten pthreads](https://emscripten.org/docs/porting/pthreads), [service-worker workaround](https://github.com/gzuidhof/coi-serviceworker)

Asyncify and JSPI can suspend synchronous-looking code around asynchronous browser operations; neither provides raw sockets or removes threading dependencies. Emscripten estimates roughly **50%** Asyncify overhead in code size and execution, with considerable application dependence. JSPI support now includes **Chrome 137, Firefox 153, Safari 27**; older Safari targets need another path. [Asyncify/JSPI documentation](https://emscripten.org/docs/porting/asyncify.html), [compatibility data](https://raw.githubusercontent.com/mdn/browser-compat-data/main/webassembly/api.json), [Safari 27 release](https://webkit.org/blog/18325/webkit-features-for-safari-27-0/)

**Rust: Emissary is the strongest candidate, with demonstrated native interoperability and unverified browser operation.** Its asynchronous core supports NTCP2/SSU2; the router is generic over a `Runtime`, with Tokio and smol implementations. The abstraction covers sockets, timers, spawning, randomness, compression, and storage. However, `Send`/`Sync` bounds and address-based TCP interfaces require deliberate browser adapters. [Repository](https://github.com/eepnet/emissary), [runtime source](https://raw.githubusercontent.com/eepnet/emissary/master/emissary-core/src/runtime/mod.rs)

A browser runtime should use cooperative tasks, browser timers, WebSocket-backed streams, and browser persistence. Do not try to retain native Tokio networking: its documented WASM support excludes ordinary `net` configurations. [Tokio WASM limitations](https://docs.rs/tokio/latest/tokio/)

Maturity remains a concern. The I2P project described Emissary as a new router prototype in October 2025. Development continued in 2026: a router-shutdown fix merged **25 April 2026**. Older reports that this particular bug remains open are stale. Ire also exists, but its README explicitly says **pre-alpha**. [I2P announcement](https://www.i2p.net//en/blog/2025/10/16/new-i2p-routers/), [merged fix](https://github.com/eepnet/emissary/pull/351), [Ire](https://github.com/str4d/ire)

**Java/CheerpJ: technically plausible, unattractive first choice.** CheerpJ supplies a browser JVM/OpenJDK and socket connectivity through browser-side Tailscale over WebSockets, or a custom backend. Thus Java I2P would still need a transport endpoint; JNI dependencies need browser-compatible replacements. This adds another runtime and integration boundary without resolving PWA lifecycle limits. [CheerpJ compatibility](https://cheerpj.com/compatibility/)

**New minimal router: theoretically possible, highest protocol risk.** “Client-only” removes transit service and floodfill duties, but still requires interoperable transports, tunnel pools, netDb operations, destination encryption, and streaming. I would adapt an existing implementation before rebuilding these mechanisms. [I2P tunnel specification](https://www.i2p.net/en/docs/specs/tunnel-implementation/), [netDb specification](https://i2p.net/en/docs/overview/network-database/)

## 2. Transport and entry nodes

**WebSocket is the initial baseline.** It is widely deployed across browsers, including mobile Safari and Chrome. Use **WSS** from the HTTPS application; do not depend on mixed-content exceptions. Its standard API lacks receive backpressure, so impose queue, frame, and connection limits. [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), [security guidance](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API/Writing_WebSocket_client_applications)

**WebTransport is now a credible second transport.** Compatibility data lists **Chrome 97, Firefox 114, Safari/iOS 26.4**. Safari shipped it on **24 March 2026**. Reliable streams and datagrams offer better transport choices, but browser WebTransport is not a raw UDP socket and cannot directly dial ordinary SSU2 peers. [Compatibility data](https://raw.githubusercontent.com/mdn/browser-compat-data/main/api/WebTransport.json), [Safari announcement](https://webkit.org/blog/17862/webkit-features-for-safari-26-4/), [API documentation](https://developer.mozilla.org/en-US/docs/Web/API/WebTransport_API)

**WebRTC DataChannel is broadly deployed**, with cross-browser availability since January 2020. It supplies DTLS/SCTP channels, including reliable ordered delivery, but introduces ICE, signaling, and potentially STUN/TURN infrastructure. [DataChannel documentation](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel)

Certificate choices:

- **WSS:** use a DDNS/custom hostname and automated ACME renewal. Let’s Encrypt HTTP-01 requires port **80**; DNS-01 validates DNS TXT records without inbound HTTP access. Certificate issuance does not itself make a CGNAT-hosted endpoint reachable. [ACME challenges](https://letsencrypt.org/docs/challenge-types/)
- **WebTransport:** `serverCertificateHashes` permits authenticated self-signed certificates. Certificates must be X.509v3, valid for **less than two weeks**, and support an acceptable key type; P-256 is the documented interoperable default. Authenticate certificate-hash updates using the paired node identity. [Constructor requirements](https://developer.mozilla.org/en-US/docs/Web/API/WebTransport/WebTransport)
- **WebRTC:** DTLS fingerprints avoid public-CA requirements, but fingerprints must be authenticated. libp2p WebRTC-direct demonstrates browser-to-native connections using certificate hashes and a subsequent identity handshake. [WebRTC-direct specification](https://github.com/libp2p/specs/blob/master/webrtc/webrtc-direct.md)

For the project:

**A dumb forwarder is preferable for the spike.** Websockify already demonstrates WebSocket-to-TCP forwarding with authentication/token plugins. The browser must perform NTCP2 with the actual remote router; the forwarder sees target addresses and encrypted transport bytes. Restrict authenticated forwarding to prevent it becoming an unrestricted proxy. [Websockify](https://github.com/novnc/websockify)

**Integrated NTCP2-over-WebSocket is preferable if public deployment justifies it.** The reachable gipny node becomes an authenticated adjacent I2P router, eliminating arbitrary target forwarding. This is a proposed extension: define endpoint discovery, identity binding, limits, and transport negotiation.

A critical inference follows: **outbound-only sockets do not imply outbound-only I2P tunnels**. Receiving still needs inbound tunnels, whose final adjacent hop must deliver over an established browser connection. With only a small entry set, peer selection must respect that constraint. Demonstrate this before optimizing crypto. [Tunnel processing](https://www.i2p.net/en/docs/specs/tunnel-implementation/)

Comparable systems provide useful but limited precedents:

- **Production:** Snowflake uses WebRTC; WebTunnel deploys HTTPS-facing bridges with WebSocket upgrade handling. Neither proves a browser-hosted Tor router. [Snowflake](https://snowflake.torproject.org/), [WebTunnel deployment](https://community.torproject.org/relay/setup/webtunnel/)
- **Implemented:** libp2p documents browser/native WebRTC and WebTransport, certificate rotation, and identity handshakes. Its older “Safari unsupported” guidance must be read against Safari’s 2026 release. [Browser connectivity](https://libp2p.io/docs/browser-connectivity/)
- **Demonstrated WASM infrastructure:** Veilid has maintained WASM builds, but deprecated WSS because of deployment/privacy roadblocks; its WebTransport issue lists remaining work. Do not cite it as proof that HTTPS browser deployment is solved. [Changelog](https://gitlab.com/veilid/veilid/-/blob/main/CHANGELOG.md), [WSS decision](https://gitlab.com/veilid/veilid/-/issues/487), [WebTransport work](https://gitlab.com/veilid/veilid/-/issues/460)

Within I2P, mooZ demonstrates experimental browser WebRTC **using a native SAM router**. That is relevant community experimentation, but it does not meet this brief. [mooZ](https://eyedeekay.github.io/mooz/index.html)

## 3. Crypto and performance

Modern ECIES tunnel construction uses **X25519, HKDF, ChaCha20-Poly1305**; NTCP2 uses a Noise-based handshake and ChaChaPoly, with AES obfuscation. Tunnel data processing also retains AES operations and fixed **1,024-byte** messages. [Tunnel creation](https://i2p.net/en/docs/specs/tunnel-creation-ecies/), [NTCP2](https://www.i2p.net//en/docs/specs/ntcp2/), [tunnel implementation](https://www.i2p.net/en/docs/specs/tunnel-implementation/)

**Derived workload example:** four three-hop tunnels require twelve hop-specific build records per generation, before exploratory pools, retries, and connection handshakes. Tunnels last approximately **10 minutes**. This suggests startup/reconnection bursts deserve separate measurement from steady messaging. It does not establish phone CPU or battery cost. [Tunnel lifetime and processing](https://www.i2p.net/en/docs/specs/tunnel-implementation/)

One browser benchmark reports Ed25519 verification means of **0.173 ms** for serial Rust/WASM, **0.070 ms** batched, and **0.056 ms** WebCrypto, on an **i7-1360P, 16 GB RAM, headless Chrome 138.0.7204.158**. These are desktop experimental results, not mobile-router estimates. [2025 study, methodology and Table IV](https://www.jocm.us/2025/JCM-V20N6-681.pdf)

NetDb need not equal a complete network snapshot: implementations choose retention policies, and accepted records require signature verification. For budgeting, an **assumed** 2,000 records averaging 2 KiB occupy about **3.9 MiB serialized**; parsed objects, indexes, tunnel buffers, WASM memory, and UI are additional. Neither that record size nor total phone memory is measured here. [NetDb policies](https://i2p.net/en/docs/overview/network-database/)

WASM SIMD is available from **Chrome 91, Firefox 89, Safari 16.4**; 128-bit SIMD can help suitable implementations. It does not automatically expose native AES instructions. [SIMD documentation](https://emscripten.org/docs/porting/simd.html)

Benchmark WebCrypto offload selectively. Raw AES-ECB is absent from its standard algorithm set, and AES-CBC mandates PKCS#7 padding, complicating I2P compatibility. ChaCha20-Poly1305 appeared in Chrome’s **155 beta** announcement, so blanket claims that WebCrypto never supports it are stale—but cross-browser availability still needs detection. [WebCrypto specification](https://www.w3.org/TR/webcrypto/), [Chrome announcement](https://developer.chrome.com/blog/chrome-155-beta)

## 4. Bootstrap

Use a **fresh signed SU3 bootstrap bundle**, verified inside WASM against pinned reseed signing certificates, plus individual RouterInfo validation. Official reseed guidance describes **75–100 RouterInfos**, distinct TLS/signing certificates, freshness, and diversity requirements. Convert the existing release artifact deliberately; do not assume it already satisfies SU3 requirements. [Reseed documentation](https://i2p.net/en/docs/misc/reseed/)

Shipping this public bundle with Pages avoids cross-origin fetch requirements. Direct browser reseeding needs suitable CORS response headers; `no-cors` produces an unreadable opaque response. Actual production reseed-server CORS coverage remains unverified. [Fetch request modes](https://developer.mozilla.org/en-US/docs/Web/API/Request/mode)

For a general byte forwarder, browser-side TLS over the forwarded stream can retrieve a reseed without browser Fetch/CORS. A transport-only NTCP2 entry instead needs an initial bundled peer set or another public bootstrap channel. These are architectural proposals, not demonstrated integrations.

After joining, perform normal I2NP floodfill lookups and LeaseSet operations inside the browser router. Preserve client netDb separation and lookup protections; do not replace them with entry-node RPC. [NetDb operations and isolation](https://i2p.net/en/docs/overview/network-database/)

## 5. Storage, keys, and code integrity

**Prefer encrypted IndexedDB records initially; evaluate OPFS SQLite for schema reuse.** wa-sqlite supports IndexedDB and OPFS VFS implementations. Its AccessHandlePoolVFS can avoid SharedArrayBuffer and Asyncify, with a single-instance restriction. Thus OPFS does **not inherently require** COOP/COEP. [wa-sqlite](https://github.com/rhashimoto/wa-sqlite), [VFS comparison](https://github.com/rhashimoto/wa-sqlite/blob/master/src/examples/README.md)

By contrast, sql.js holds its database in memory and requires explicit import/export for persistence. This is convenient for small prototypes but entails whole-database memory and persistence management. [sql.js](https://github.com/sql-js/sql.js/)

SQLCipher is not theoretically excluded: a **May 2023** experiment built it for WASM with OpenSSL after removing `mlock`. That is prototype evidence, not a supported production browser package. [SQLCipher experiment](https://discuss.zetetic.net/t/is-there-interest-in-the-wasm-build/5922)

My recommendation is a random vault key wrapping authenticated encrypted records, protected by either an Argon2-derived password key or WebAuthn PRF-derived wrapping key. Include recovery export, authenticated metadata, and crash-safe transactions. This is a proposed replacement design requiring review, not a drop-in SQLCipher equivalent.

A non-extractable `CryptoKey` prevents API export; it does not prevent authorized operations by hostile same-origin code, guarantee hardware storage, or guarantee encrypted key persistence. [CryptoKey semantics](https://developer.mozilla.org/en-US/docs/Web/API/CryptoKey), [WebCrypto security model](https://www.w3.org/TR/webcrypto/)

**PRF is usable but authenticator-dependent.** Safari announced PRF in **18.0** and expanded security-key support in **26.4**. Compatibility data records Chrome **116** and desktop Firefox **139**, with differences between creation/assertion paths. Detect actual extension results and maintain recovery: PRF cannot be invoked silently. [Safari 18](https://webkit.org/blog/15865/webkit-features-in-safari-18-0/), [Safari 26.4](https://webkit.org/blog/17862/webkit-features-for-safari-26-4/), [compatibility data](https://raw.githubusercontent.com/mdn/browser-compat-data/main/api/CredentialsContainer.json), [PRF explainer](https://github.com/w3c/webauthn/blob/main/explainers/prf-extension.md)

Safari 17 introduced full persistent-storage API support; quota can reach **60% of disk**, but that is an upper bound, not reserved capacity. Request `persist()`, inspect its result, and provide backups. Home Screen applications are exempt from ITP’s **seven-day** script-storage cap, while storage pressure and user deletion remain relevant. [Storage policy](https://webkit.org/blog/14403/updates-to-storage-policy/), [tracking-prevention rules](https://webkit.org/tracking-prevention/)

For updates, use versioned caches, migration checks, and deliberate activation. Automatically calling `skipWaiting()` can mix old page code with a new worker. [Service-worker lifecycle](https://developer.chrome.com/docs/workbox/service-worker-lifecycle?hl=en)

For supply-chain integrity, use pinned dependencies, reproducible release artifacts, SRI, a restrictive meta CSP, and an authenticated WASM manifest. These reduce attack opportunities; hashes delivered by compromised HTML cannot authenticate that HTML. Code Verify adds an independently installed verifier and external hash authority. IWAs provide signed bundles but currently target enterprise ChromeOS, not universal iOS/Android deployment. [SRI](https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Subresource_Integrity), [CSP](https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/CSP), [Code Verify](https://engineering.fb.com/2022/03/10/security/code-verify/), [IWA restrictions](https://developer.chrome.com/docs/iwa/introduction)

## 6. PWA platform limits

**Installed does not mean continuously executable.** Chrome can freeze/discard pages; its guidance explicitly discusses closing WebSockets and WebRTC connections. WebKit also implements background suspension. No reliable cross-platform WebSocket lifetime can be promised. [Chrome lifecycle](https://developer.chrome.com/docs/web-platform/page-lifecycle-api), [WebKit suspension implementation](https://bugs.webkit.org/show_bug.cgi?id=198488)

iOS/iPadOS **16.4+** supports Web Push for Home Screen apps after user interaction and permission. WebKit requires user-visible notifications and disallows silent push; push therefore cannot serve as a hidden router keepalive. Android Chrome also supports push-driven service-worker execution, but page lifecycle management still prevents treating it as a permanent router process. [iOS Web Push](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/), [silent-push restrictions](https://webkit.org/blog/16535/), [worker lifecycle](https://developer.chrome.com/docs/workbox/service-worker-lifecycle?hl=en)

Product behavior should be: **relay retains encrypted mail while closed; opening the PWA unlocks storage, rebuilds tunnels, and retrieves mail through I2P**. Optional push should contain only a generic wake notification. Push necessarily introduces a separate notification service and metadata relationship; accepting that is an owner decision.

## 7. Threat model and honest promises

By architectural inference, a forwarder learns client IP, session timing, volume, and forwarded destinations. An integrated NTCP2 entry additionally authenticates the browser router and sees its adjacent-hop protocol traffic. Neither should possess destination/application private keys. Stable pairing and router identities permit correlation across sessions. These conclusions follow from forwarding and NTCP2’s authenticated transport design. [Websockify](https://github.com/novnc/websockify), [NTCP2](https://www.i2p.net//en/docs/specs/ntcp2/)

WASM does not make browser fingerprinting or timing attacks disappear. Research demonstrates verified WASM crypto and discusses compilation-induced timing leaks; those guarantees do not automatically extend to this Rust/C++ router or its browser JIT. [Verified WASM cryptography research](https://beurdouche.github.io/publications/2019-SignalWASM.pdf)

Modern answers to “JavaScript crypto” objections include browser-native cryptography, reviewed WASM implementations, secure randomness, isolation, and independent code verification. The enduring objection is executable-code delivery: injected script can access plaintext when the app is unlocked. [WebCrypto security considerations](https://www.w3.org/TR/webcrypto/)

An honest promise is:

> Keys and message processing remain on your device. Static hosting receives no conversation traffic. Entry nodes observe connection metadata and encrypted transport traffic. Delivery resumes when the app runs; protection assumes an uncompromised browser and authentic application code.

Do not promise unobservability, perpetual background delivery, or immunity to malicious application updates.

## 8. Phased recommendation and decisions

These are **planning estimates**, not verified schedules; assume experienced Rust/I2P/browser engineers.

| Phase | Rough effort | Concrete exit condition | Single biggest risk |
|---|---:|---|---|
| Spike | 2–4 engineer-weeks | Browser builds inbound/outbound tunnels, exchanges messages, renews tunnels, reconnects on iOS and Android | Restricted transport adjacency breaks inbound delivery |
| Prototype | 8–16 engineer-weeks | Encrypted persistence, recovery export, relay retrieval, updates, measured phone resource use | Suspension/recovery causes state loss |
| Product | 6–12 engineer-months, plus external review | Interoperability soak, hostile-entry testing, recovery validation, reproducible releases, supported-device matrix | Compromised executable updates |

Measure cold/warm startup, tunnel success, foreground idle power, memory, verification bursts, and recovery after network changes. Set numerical acceptance budgets after collecting device measurements.

The owner must decide minimum browser versions; owner-operated versus shared entries; whether push metadata is acceptable; whether entry discovery becomes an I2P transport extension; and whether “hosting never sees data” assumes authentic code or must withstand an actively malicious host.

# Options comparison

| Option | Established evidence | Browser status | Assessment |
|---|---|---|---|
| i2pd/Emscripten | Mature native implementation; browser dependencies demonstrated | Whole router unverified | Best fallback; preserves existing fork |
| Emissary/WASM | Native router, explicit runtime abstraction | Browser integration unverified | **Preferred spike** |
| Java/CheerpJ | Browser JVM and socket-backend demonstrations | I2P integration unverified | Extra runtime and networking complexity |
| New minimal router | Published interoperable protocol specifications | Theoretical | Highest implementation/security burden |
| WSS forwarder | Existing browser-to-TCP implementations | Production mechanism | Fastest transport experiment |
| Integrated NTCP2/WSS | NTCP2 and WebSocket separately established | Proposed integration | Cleaner bounded entry service |
| WebTransport/WebRTC | Shipped APIs and libp2p implementations | Established building blocks | Later transport options |

Evidence and citations for each row appear in Findings 1–2.

# Risks and unknowns

The principal engineering uncertainty is bidirectional I2P reachability through browser-compatible adjacencies. The principal product constraint is suspension. The principal security constraint is trusting delivered application code.

Other release gates include entry diversity, bootstrap freshness, storage-loss recovery, hostile-input resource limits, cross-browser crypto compatibility, and sustained phone power use. These require project measurements and review; this research does not establish their outcome.

# Unverified claims

- No production deployment of the complete requested browser-only I2P architecture was verified.
- A community announcement claims Emissary compiles to WASM; this does not verify browser networking or phone operation. [Announcement](https://www.reddit.com/r/rust/comments/1joqdz0)
- No completed i2pd/Emscripten browser port or Java-I2P/CheerpJ integration was verified.
- No specific adopted I2P WebSocket/WebRTC router-transport proposal was verified.
- Production reseed CORS coverage, whole-router mobile benchmarks, and service-worker isolation compatibility across the supported iOS matrix remain unverified.
- Effort estimates, memory-budget examples, and proposed integrations above are engineering judgments, not measured results.

# Sources

Deduplicated sources consulted and used:

- Emscripten: [networking](https://emscripten.org/docs/porting/networking.html), [pthreads](https://emscripten.org/docs/porting/pthreads), [Asyncify/JSPI](https://emscripten.org/docs/porting/asyncify.html), [SIMD](https://emscripten.org/docs/porting/simd.html), [settings](https://emscripten.org/docs/tools_reference/settings_reference.html).
- Porting evidence: [OpenSSL browser build](https://github.com/cryptool-org/openssl-webterm), [Asio polling issue](https://github.com/emscripten-core/emscripten/issues/25759), [COOP/COEP worker](https://github.com/gzuidhof/coi-serviceworker).
- Rust routers: [Emissary](https://github.com/eepnet/emissary), [embedding](https://eepnet.github.io/emissary/embedding-rust.html), [runtime source](https://raw.githubusercontent.com/eepnet/emissary/master/emissary-core/src/runtime/mod.rs), [shutdown fix](https://github.com/eepnet/emissary/pull/351), [WASM announcement](https://www.reddit.com/r/rust/comments/1joqdz0), [Ire](https://github.com/str4d/ire), [Tokio](https://docs.rs/tokio/latest/tokio/).
- Alternative runtime: [CheerpJ compatibility](https://cheerpj.com/compatibility/).
- Browser compatibility: [WebAssembly](https://raw.githubusercontent.com/mdn/browser-compat-data/main/webassembly/api.json), [WebTransport](https://raw.githubusercontent.com/mdn/browser-compat-data/main/api/WebTransport.json), [WebAuthn](https://raw.githubusercontent.com/mdn/browser-compat-data/main/api/CredentialsContainer.json).
- Networking APIs: [WebSocket](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), [WebSocket security](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API/Writing_WebSocket_client_applications), [WebTransport API](https://developer.mozilla.org/en-US/docs/Web/API/WebTransport_API), [certificate requirements](https://developer.mozilla.org/en-US/docs/Web/API/WebTransport/WebTransport), [DataChannel](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel).
- Entry precedents: [ACME challenges](https://letsencrypt.org/docs/challenge-types/), [Websockify](https://github.com/novnc/websockify), [libp2p connectivity](https://libp2p.io/docs/browser-connectivity/), [WebRTC-direct](https://github.com/libp2p/specs/blob/master/webrtc/webrtc-direct.md), [Snowflake](https://snowflake.torproject.org/), [WebTunnel](https://community.torproject.org/relay/setup/webtunnel/), [mooZ](https://eyedeekay.github.io/mooz/index.html).
- Veilid: [changelog](https://gitlab.com/veilid/veilid/-/blob/main/CHANGELOG.md), [WSS deprecation](https://gitlab.com/veilid/veilid/-/issues/487), [WebTransport work](https://gitlab.com/veilid/veilid/-/issues/460).
- I2P: [new routers](https://www.i2p.net//en/blog/2025/10/16/new-i2p-routers/), [tunnel creation](https://i2p.net/en/docs/specs/tunnel-creation-ecies/), [tunnel implementation](https://www.i2p.net/en/docs/specs/tunnel-implementation/), [NTCP2](https://www.i2p.net//en/docs/specs/ntcp2/), [netDb](https://i2p.net/en/docs/overview/network-database/), [reseeding](https://i2p.net/en/docs/misc/reseed/).
- Cryptography: [2025 browser benchmarks](https://www.jocm.us/2025/JCM-V20N6-681.pdf), [verified WASM research](https://beurdouche.github.io/publications/2019-SignalWASM.pdf), [WebCrypto specification](https://www.w3.org/TR/webcrypto/), [Chrome 155](https://developer.chrome.com/blog/chrome-155-beta), [CryptoKey](https://developer.mozilla.org/en-US/docs/Web/API/CryptoKey), [PRF explainer](https://github.com/w3c/webauthn/blob/main/explainers/prf-extension.md).
- Storage: [wa-sqlite](https://github.com/rhashimoto/wa-sqlite), [VFS comparison](https://github.com/rhashimoto/wa-sqlite/blob/master/src/examples/README.md), [sql.js](https://github.com/sql-js/sql.js/), [SQLCipher experiment](https://discuss.zetetic.net/t/is-there-interest-in-the-wasm-build/5922), [Fetch modes](https://developer.mozilla.org/en-US/docs/Web/API/Request/mode).
- WebKit: [Safari 18](https://webkit.org/blog/15865/webkit-features-in-safari-18-0/), [Safari 26.4](https://webkit.org/blog/17862/webkit-features-for-safari-26-4/), [Safari 27](https://webkit.org/blog/18325/webkit-features-for-safari-27-0/), [storage policy](https://webkit.org/blog/14403/updates-to-storage-policy/), [tracking prevention](https://webkit.org/tracking-prevention/), [suspension](https://bugs.webkit.org/show_bug.cgi?id=198488), [iOS push](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/), [silent-push restrictions](https://webkit.org/blog/16535/).
- Lifecycle and integrity: [Chrome page lifecycle](https://developer.chrome.com/docs/web-platform/page-lifecycle-api), [service-worker lifecycle](https://developer.chrome.com/docs/workbox/service-worker-lifecycle?hl=en), [SRI](https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Subresource_Integrity), [CSP](https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/CSP), [Code Verify](https://engineering.fb.com/2022/03/10/security/code-verify/), [IWAs](https://developer.chrome.com/docs/iwa/introduction).