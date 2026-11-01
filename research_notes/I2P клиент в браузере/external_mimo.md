# External AI worker notes: Emscripten (Boost.Asio, OpenSSL, threads, COOP/COEP) and CheerpJ toolchain facts

> Source: non-Claude worker `opencode/mimo-v2.6-flash-free` run through opencode on 2026-11-01 with a written brief. Lower confidence than the Claude researcher notes: treat its URLs and figures as unverified unless they match a Claude note or are obviously primary sources; the worker was told to mark missing web access and unverified claims itself (see its first line).

# Research notes: browser toolchains for an I2P-class daemon (Emscripten / CheerpJ)

## 1. Web access status

Yes — web access worked. Both a web-search tool and a web-fetch tool returned real, current pages (Emscripten docs and release notes, caniuse tables, GitHub repos/issues, Leaning Technologies blog, Chromium tracker). All claims below are backed by URLs that were actually retrieved; anything not directly confirmed is collected under **Unverified**.

## 2. Emscripten sockets and Asio

**Default socket model: WebSocket, not TCP.** Emscripten's networking docs state that POSIX TCP sockets are emulated over WebSocket in the browser: the client-side `connect()` is mapped to a WebSocket connection, and on the *server* side you need a WebSockify-style proxy that terminates real TCP and forwards over WebSocket ([networking docs](https://emscripten.org/docs/porting/networking.html)). Configuration knobs verified in the docs and in `src/settings.js`: `-sWEBSOCKET_URL`, `-sWEBSOCKET_SUBPROTOCOL`, linking with `-lwebsocket.js`. The docs themselves call the emulation "not very complete".

**`connect()`/`accept()`/UDP.** Outbound `connect()` works (via WebSocket, so TLS is whatever `ws://`/`wss://` gives you at the browser level). `accept()`/listening is meaningless in a tab: a browser cannot accept inbound TCP. UDP "is not available in the browser" per the networking docs (WebRTC was the historical alternative; the `SOCKET_WEBRTC` setting was removed in Emscripten 6.0.8, Aug 2026 — [release notes](https://newreleases.io/project/github/emscripten-core/emscripten/release/6.0.8)). Blocking socket calls don't block: they return `EAGAIN`, which is exactly what breaks naïve blocking Asio code.

**Full POSIX proxy.** For apps that insist on real sockets there is `-sPROXY_POSIX_SOCKETS` (present in `settings.js`), which proxies syscalls to Emscripten's own proxy server process — but it requires pthreads, `-sPROXY_TO_PTHREAD`, and running a dedicated proxy server; it is not something GitHub Pages can host ([networking docs](https://emscripten.org/docs/porting/networking.html)).

**Can Boost.Asio be built under Emscripten in 2026?** Yes, with caveats:

- There is a live Emscripten issue about building Boost with `b2 toolset=emscripten` failing ([#22767](https://github.com/emscripten-core/emscripten/issues/22767), open since Oct 2024). The `boost_headers` port exists in Emscripten; compiled Boost libs are the friction point. vcpkg's `boost-asio` port explicitly excludes features on emscripten (`ssl` requires openssl `platform: !emscripten`; the `spawn` feature is `!emscripten`).
- People do compile Asio programs: a 2019 StackOverflow answer shows Boost 1.69 + Asio + SSL compiled with emscripten, but the program **hangs after the WebSocket handshake** because blocking calls don't block ([SO 55795238](https://stackoverflow.com/questions/55795238)). A 2023 write-up demonstrates multithreaded Asio under Emscripten with `-pthread`.
- Reactor backend: Boost.Asio gates epoll on `__linux__` (`BOOST_ASIO_HAS_EPOLL` is described as "Linux: epoll, eventfd, timerfd…" in the [Asio docs](https://www.boost.org/doc/libs/1_87_0/doc/html/boost_asio/using.html)). Emscripten's documented preprocessor defines (`__EMSCRIPTEN__`, `unix`/`__unix`/`__unix__`, clang/gcc macros; 32-bit `size_t`) do **not** include `__linux__` ([Building Projects](https://emscripten.org/docs/compiling/Building-Projects.html)), so Asio should fall back to its `select`-based reactor. Notably, Emscripten 6.0.8 *did* add working `epoll_*` syscalls to the JS filesystem (with blocking waits under `PROXY_TO_PTHREAD`/`ASYNCIFY`/`JSPI`), and 6.0.12 added experimental `emscripten_epoll_listener_add` ([6.0.8](https://newreleases.io/project/github/emscripten-core/emscripten/release/6.0.8), [6.0.12](https://newreleases.io/project/github/emscripten-core/emscripten/release/6.0.12)) — so an epoll path is becoming possible, but it is brand new.
- Build integration: use `emcmake cmake` / `emconfigure ./configure` + `emmake make` per the official [Building Projects](https://emscripten.org/docs/compiling/Building-Projects.html) docs.

**Practical consequence for i2pd:** i2pd's Asio TCP acceptors can never work in-browser; all traffic must be dialed out over WebSocket(s) to gateways/bridges. Asio's `ssl::stream` over such a connection is doubly awkward: TLS is already done by the browser under `wss://`, and the 2019 experiment above shows raw `asio::ssl` handshakes hanging under the WebSocket emulation.

## 3. OpenSSL and zlib

- **zlib: first-class port.** `USE_ZLIB` is a real Emscripten port setting (verified in `settings.js`); enable with `--use-port=zlib` or `-sUSE_ZLIB=1`.
- **OpenSSL: no port.** The `tools/ports` directory was listed via the GitHub API at tags 1.38.31, 1.39.0, 2.0.0, 3.1.55, 3.1.74, 4.0.0, 5.0.0 and `main`: there is no `openssl` or `curl` port, and `settings.js` contains no `USE_OPENSSL`/`USE_CURL`. `boost_headers` exists; OpenSSL does not.
- **Build from source.** The working recipe is `emconfigure ./configure` + `emmake make`. This is exactly what the `openssl-wasm` npm package does: a Docker image sets up emsdk and "builds OpenSSL using `emmake make`", shipping `openssl.wasm` + loader JS ([npm](https://www.npmjs.com/package/openssl-wasm)). A maintained WASI variant exists too (`jedisct1/openssl-wasm`, built with Zig, precompiled static libs linkable from C/Rust/Zig — [repo](https://github.com/jedisct1/openssl-wasm)).
- **Size.** No verified byte figure for an OpenSSL-for-Emscripten build was found (see Unverified). The closest published comparable: Arti-to-wasm ("tor-js") ships "31 kB + 1.9 MB gzipped wasm" for a full Tor client ([ethereum/tor-js](https://github.com/ethereum/tor-js)). For order-of-magnitude context, a gzip-only wasm module is ~121 kB (wasm-gzip README).

## 4. Threads, COOP/COEP on static hosts, single-threaded alternatives

**Why isolation is needed.** Emscripten pthreads are Web Workers + `SharedArrayBuffer`, and browsers only expose SAB to cross-origin-isolated pages (COOP: `same-origin` + COEP: `require-corp` or `credentialless`) — [pthreads docs](https://emscripten.org/docs/porting/pthreads.html).

**GitHub Pages cannot set headers** (no custom COOP/COEP response headers; confirmed via GitHub's community discussion). The standard workaround is **coi-serviceworker** ([gzuidhof/coi-serviceworker](https://github.com/gzuidhof/coi-serviceworker)): a service worker that intercepts document requests and injects COOP/COEP headers. Verified rules and behavior:

- It must be a **separate file on your own origin** — cannot be bundled, cannot be loaded from a CDN; page must be HTTPS or localhost (README).
- **First load reloads the page** once so the SW controls it; failure states are tracked in `sessionStorage` (`coepCoepHasFailed`, degrade-to-`credentialless` logic in the shipped JS).
- `COEP: require-corp` makes every cross-origin subresource fail unless it carries CORP/CORS headers — this breaks CDNs/Firebase unless you switch to `credentialless` (a project decision record explicitly rejected the trade-off; ImHex ships an *edited* copy of coi-serviceworker for GitHub Pages that flips `credentialless` on).
- Chromium bug [#40830070](https://issues.chromium.org/issues/40830070) documents that the service-worker header-injection hack interacts badly with `COEP: credentialless` (cross-origin images proxied through the SW get blocked). A 2025 comment on the Chromium issue reports the workaround stopped working in a Chromium release (regression around credentialless/COEP interplay); it was still reported working with caveats in a 2026 cookbook. Firefox private browsing may lack service workers entirely (coi-serviceworker README/source).

**Single-threaded alternative: ASYNCIFY / JSPI.** For an event-driven Asio program, running one thread and letting blocking calls suspend is now the pragmatic path:

- `ASYNCIFY` (with mode 2 deprecated in favor of `-sJSPI`) and `JSPI`/`JSPI_EXPORTS` are real settings in `settings.js`.
- **JSPI is no longer experimental** as of Emscripten 6.0.8 (the warning was removed) — [release notes](https://newreleases.io/project/github/emscripten-core/emscripten/release/6.0.8).
- Browser support per wasm-bindgen's JSPI runtime table: **Chrome 137+, Firefox 153+, Safari Technology Preview 238, Node 25** (desktop Safari stable status per that table: not yet). Safari iOS therefore remains the weak point for JSPI.
- JSPI lets a blocking Wasm call suspend the stack and return control to the event loop — precisely the semantic Asio blocking I/O expects, replacing the `EAGAIN` failure mode.

**Recommendation implied by the facts:** pthreads+COOP/COEP only if you control headers or accept coi-serviceworker's warts; otherwise a single-threaded JSPI build is the realistic 2026 target for an Asio-style reactor, at the cost of losing i2pd's multi-threaded design.

## 5. Browser WASM limits (mobile)

Support tables (caniuse, StatCounter Sept 2026 data on the pages):

- **WebAssembly baseline**: ~97.2% global ([caniuse/wasm](https://caniuse.com/wasm)).
- **Wasm SIMD**: Chrome/Edge 91+, Firefox 89+, **Safari & iOS Safari 16.4+**, Samsung 16+; ~96.5% global ([caniuse/wasm-simd](https://caniuse.com/wasm-simd)).
- **Memory64**: Chrome/Edge 133+, Firefox 134+, Samsung 29+; **Safari: not supported** (through 27.x); ~73.5% global ([caniuse/wf-wasm-memory64](https://caniuse.com/wf-wasm-memory64)). So on iPhones you are stuck with wasm32's theoretical 4 GiB cap (V8 lifted 2 GiB→4 GiB in 2020 — [v8.dev/blog/4gb-wasm-memory](https://v8.dev/blog/4gb-wasm-memory)), far above what iOS actually grants.
- **JSPI**: shipped in Chromium 137+/Firefox 153+; Safari stable pending (see §4).

**iOS Safari practical memory:** WebKit bug [269777](https://bugs.webkit.org/show_bug.cgi?id=269777) states that on iOS 17 only ~300–500 MB of linear memory is reliably allocatable (against a 4 GB max), and the allocatable amount *shrinks* after page reloads. Community reports (Unity forum) claim iOS 18.2 raised the practical cap (~500 MB); a three.js maintainer notes iOS 26/Safari assigns ~2 GB only after a reload. These are the numbers a router with big netDB/leases data must plan around.

**Android Chrome practical memory:** the verified data is older: Chromium issue [40747423](https://issues.chromium.org/issues/40747423) (2021, Chrome 88, then 32-bit) reports hard caps of ~256–640 MB on 8 GB phones; Unity's WebAssembly/design issue [#1397](https://github.com/WebAssembly/design/issues/1397) (2021) concludes ">~300 MB is not reliable on Chrome on Android". Whether 2026's 64-bit Chrome for Android does better was not verified (see Unverified).

## 6. CheerpJ

- **What it is now:** CheerpJ 3.0 (Feb 2024) replaced the old JVM-in-JS with a **WebAssembly JVM** — Java classes compile to Wasm; demos ran IntelliJ and Minecraft in the tab ([labs.leaningtech.com/blog/cheerpj-3.0](https://labs.leaningtech.com/blog/cheerpj-3.0), [InfoWorld coverage](https://www.infoworld.com/article/3713063/run-java-apps-in-the-browser-with-cheerpj-3-0.html)). CheerpJ 4.x targets **Java 11** with JNI support ([cheerpj-4.0 blog](https://labs.leaningtech.com/blog/cheerpj-4.0)).
- **Networking:** outbound TCP works in tutorials via `java.net.Socket`/`ServerSocket`; the deep-dive blog describes a **Tailscale-based networking layer** establishing peer-to-peer connections from the browser (relayed when a direct path fails). Java multi-threading is supported (mapped onto Web Workers, per the CheerpJ overview docs).
- **Licensing/cost:** CheerpJ 4.0 is **commercial software, free for open-source, personal use, and single-person companies** ([cheerpj-4.0 blog](https://labs.leaningtech.com/blog/cheerpj-4.0)).
- **Anonymity/P2P evidence:** no published example of an anonymity network (I2P/Tor) running through CheerpJ was found; the Tailscale layer proves P2P-capable networking is *possible* but is not anonymity software. NIO (`Selector`/`ServerSocketChannel`) behavior under CheerpJ was not confirmed (see Unverified).

## 7. Comparable ports and their lessons

- **tor-js (Arti → WASM, browser)** — [ethereum/tor-js](https://github.com/ethereum/tor-js): Rust Arti Tor client compiled to Wasm; sizes "31 kB + 1.9 MB gzipped wasm". Key admission: browsers have no raw TCP, so the client needs a **"gateway"** component that bridges TCP to WebSocket.
- **libtorrent-wasm** — [Banou26/libtorrent-wasm](https://github.com/Banou26/libtorrent-wasm): libtorrent built with Emscripten; **single-threaded**, runs in a Web Worker; socket syscalls are implemented in a custom Emscripten JS library (`library_fkn.js`) that tunnels them over a WebVPN-style WebSocket; storage moved to OPFS; a `patches/` directory of source changes ships with the build.
- **veilid-wasm** — veilid's WASM build: **WebSocket-only transport** ("no DNS, no inbound ports"), inbound connections require selecting relays, bootstrap happens over `wss://` to a public bootstrap node, with explicit connection-count caps because browser connection limits are real.

**Lessons:** every comparable port (1) replaces raw TCP with WebSocket bridges/gateways and relays for inbound, (2) replaces DNS with hard-coded/DoH-style resolution, (3) moves persistence from the filesystem to OPFS/IndexedDB, (4) goes single-threaded or Web-Worker-based, and (5) accepts that "listening" as a server is impossible — the browser is always a client. None of them kept OpenSSL in the data path where the transport was replaced by `wss://` (TLS handled by the browser).

## 8. Unverified

- Exact `.wasm` size of OpenSSL+zlib (or full i2pd) built with emsdk — no emsdk in this environment, no published figure found; only the tor-js 1.9 MB gz comparable exists.
- Whether current Boost (1.8x/1.9x) + emsdk `b2` builds cleanly end-to-end — issue #22767 is still open; no confirmed patch recipe found.
- Whether `asio::ssl` works over Emscripten's WebSocket sockets today (the 2019 report showed a hang; nothing newer found).
- Practical wasm memory limits of **2025–2026 64-bit Chrome on Android** (only 2021-era 32-bit figures verified).
- CheerpJ support for `java.nio` selectors / `ServerSocketChannel`, and any CheerpJ deployment of anonymity software.
- Any existing **i2pd-to-wasm** attempt — searches found none (closest: veilid, tor-js, libtorrent-wasm).
- Current (2026) coi-serviceworker behavior specifically on iOS Safari; the 2025 breakage report was Chromium-specific.
- The concrete set of i2pd source patches needed (config/TUI, filesystem paths, transports) — out of scope of toolchain research.

## 9. Sources (URLs actually fetched)

- https://emscripten.org/docs/porting/networking.html
- https://emscripten.org/docs/porting/pthreads.html
- https://emscripten.org/docs/compiling/Building-Projects.html
- https://raw.githubusercontent.com/emscripten-core/emscripten/main/src/settings.js (and GitHub API listings of tools/ports at multiple tags)
- https://newreleases.io/project/github/emscripten-core/emscripten/release/6.0.8
- https://newreleases.io/project/github/emscripten-core/emscripten/release/6.0.12
- https://github.com/emscripten-core/emscripten/issues/22767
- https://github.com/gzuidhof/coi-serviceworker (README, coi-serviceworker.js)
- https://issues.chromium.org/issues/40830070
- https://issues.chromium.org/issues/40747423
- https://github.com/WebAssembly/design/issues/1397
- https://v8.dev/blog/4gb-wasm-memory
- https://www.webkit.org/bugs.webkit.org/show_bug.cgi?id=269777
- https://caniuse.com/wasm
- https://caniuse.com/wasm-simd
- https://caniuse.com/wf-wasm-memory64
- https://wasm-bindgen.github.io/wasm-bindgen/reference/async.html (JSPI browser support table)
- https://www.boost.org/doc/libs/1_87_0/doc/html/boost_asio/using.html
- https://stackoverflow.com/questions/55795238
- https://github.com/ethereum/tor-js
- https://github.com/Banou26/libtorrent-wasm
- https://forge.hackers.town (veilid/veilid-wasm project page)
- https://labs.leaningtech.com/blog/cheerpj-3.0
- https://labs.leaningtech.com/blog/cheerpj-4.0
- https://www.infoworld.com/article/3713063/run-java-apps-in-the-browser-with-cheerpj-3-0.html
- https://www.npmjs.com/package/openssl-wasm
- https://github.com/jedisct1/openssl-wasm
- vcpkg registry portfile for boost-asio (emscripten platform exclusions)
