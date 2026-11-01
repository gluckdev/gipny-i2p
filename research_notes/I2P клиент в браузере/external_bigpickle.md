# External AI worker notes: Browser-to-native transports in anonymity/P2P networks, certificates for home entry nodes, I2P community discussions

> Source: non-Claude worker `opencode/big-pickle` run through opencode on 2026-11-01 with a written brief. Lower confidence than the Claude researcher notes: treat its URLs and figures as unverified unless they match a Claude note or are obviously primary sources; the worker was told to mark missing web access and unverified claims itself (see its first line).

## 1. Web access status: working

WebFetch and WebSearch returned real pages; search backend intermittently returned HTTP 429, so some were retried or replaced with direct fetches. Every source URL was fetched; unchecked claims are under Unverified.

## 2. Tor

**Snowflake** is the closest model for a browser client that reaches a native anonymity network without speaking its transport. Client and volunteer proxy rendezvous through a broker, establish a WebRTC data channel, and the proxy relays bytes to a Tor bridge over TCP. The proxy sees the client's IP, timing and volume, and can drop or delay traffic, but cannot read or modify the Tor stream: Tor's end-to-end encryption sits inside Snowflake, and WebRTC's DTLS is only hop-by-hop. Proxies are temporary and unauthenticated; the client does not learn the bridge address ([Snowflake paper](https://www.usenix.org/system/files/usenixsecurity24-bocovich.pdf)).

**WebTunnel** makes the bridge look like an HTTPS site. The operator needs a static IPv4 address, a domain, a valid TLS certificate, and a reverse proxy (NGINX/Apache) that sends a secret path to `127.0.0.1:15000` with WebSocket upgrade headers. The reverse proxy/bridge sees the client IP; the Tor link is carried inside TLS ([Tor WebTunnel setup](https://community.torproject.org/relay/setup/webtunnel/)). This is a direct precedent: the certificate is not optional.

**Arti in the browser.** The concrete browser-resident Tor work I found is the Ethereum Foundation's Arti/WASM experiment. Arti was compiled to WebAssembly and uses Snowflake because the browser has no sockets. Fast-bootstrap and zstd reportedly cut bootstrap to seconds; the TypeScript wrapper exposes a `fetch()`-like API and persists in IndexedDB. Upstream merge is in progress. Named blockers: threads/`spawn`, native sockets, filesystem, SQLite, system time, and `ring`'s C/assembly needing pure-Rust crypto ([Ethereum Foundation, 2026-03-20](https://reads.ethereum.foundation/blog/embedding-arti-in-the-browser/)). I did not find an official Tor Project statement of a browser client.

## 3. libp2p

libp2p has the most explicit treatment of the browser-certificate problem. Its current browser-connectivity page (site commit 2026-01-29) lays out three transports:

- **WebSocket.** The browser on an HTTPS page may only use `wss://`; plain `ws://` works only from `http://` pages. The secure handshake costs about 5 RTTs (TCP, TLS, Upgrade, multistream, Noise/TLS). Most nodes have no domain or CA certificate, so WebSocket remains niche. AutoTLS gives a node `[PeerID].libp2p.direct` and an ACME-issued certificate; IP certificates need ports 80/443, and LAN addresses hit DNS-rebinding protections ([libp2p browser connectivity](https://libp2p.io/docs/browser-connectivity/)).
- **WebTransport.** The browser verifies a CA chain or a SHA-256 hash of a self-signed certificate supplied via `serverCertificateHashes`; libp2p puts the hash in the multiaddr (`/ip4/.../udp/4001/quic/webtransport/certhash/<hash>`) and runs Noise on the first stream to authenticate peers and bind the hash. It costs about 3 RTTs. W3C requires such certificates to be valid at most 14 days and non-RSA; libp2p recommends two overlapping 14-day certificates and rolling the advertised hash ([spec](https://github.com/libp2p/specs/blob/master/webtransport/README.md), [docs](https://libp2p.io/docs/webtransport/)).
- **WebRTC Direct.** The no-CA option for browser-to-node. The server advertises IP, UDP port and DTLS cert hash; the browser synthesizes SDP from the multiaddr, so no signalling is needed, and self-signed DTLS is accepted. Noise then authenticates the libp2p peer. Current docs call it “most commonly used for browser to node communication.” The spec is r2 (2026-06-20); v2 removes SDP munging because Chromium's `WebRTC-NoSdpMangleUfrag` trial breaks v1 ([spec](https://raw.githubusercontent.com/libp2p/specs/master/webrtc/webrtc-direct.md)).

For 2026 browser→server links, the practical order is WebRTC Direct when there is no CA certificate but a public UDP endpoint; WebTransport with a pinned hash when QUIC is acceptable and the client can rotate the hash; WSS only with a stable domain and CA certificate. None of these solves the WebSocket certificate problem: WebSocket has no certificate-hash API.

## 4. Veilid

Veilid is the closest architectural precedent for a browser node. Its networking documentation describes UDP, TCP and WebSocket transports; SSL is optional and described as being for HTTPS WebSockets used by web apps. DNS is used only at bootstrap (TXT records), and all nodes are equal, with relay configuration for low-capability nodes ([Veilid networking](https://veilid.com/documentation/networking/)).

The `veilid-wasm` README is unusually explicit about browser constraints. A WASM node cannot use TCP/UDP or DNS, so it can communicate only over WebSockets and must bootstrap directly at a URL such as `ws://bootstrap-v1.veilid.net:5150/ws`. Because it cannot open ports, it selects another node as its **Inbound Relay** so other nodes can reach it over a WebSocket. `ws://` works only from an `http://` page, and `wss://` only from an `https://` page with a valid certificate. The README also warns that `wss://` URLs containing an IP address are rejected by veilid-core (as of v0.2.3), even if a CA will issue a certificate for that IP. An **Outbound Relay** feature is planned but “not currently implemented”; it would be hosted with the web app and would need a CA-trusted certificate ([veilid-wasm README](https://gitlab.com/veilid/veilid/-/raw/main/veilid-wasm/README.md)). This is the same corner we are in: a browser node cannot listen, needs WSS on HTTPS pages, and needs a trusted certificate or a relay.

## 5. Other networks

- **Nym.** Its TypeScript SDK has a WASM client that builds Sphinx packets, and `@nymproject/mix-websocket` exposes a browser-like `WebSocket` over the mixnet. Traffic reaches ordinary WebSocket/WSS destinations through an IPR exit gateway, so the destination sees the IPR's IP, not the browser's ([Nym WASM client](https://nym.com/docs/developers/clients/webassembly-client), [mix-WebSocket](https://nym.com/docs/developers/mix-websocket)).
- **Yggdrasil.** A WASM fork runs a browser node with WebSocket peers, autopeering including `wss`, and DNS fallback. Its page notes (as of 08.05.2026) that most public peers reject cross-origin WebSocket, the browser can only dial out, and `wss` needs a reverse proxy ([demo](https://asciimoth.github.io/ygg/), [fork](https://github.com/asciimoth/ygg)).
- **Tailscale.** `cmd/tsconnect` is a JS/WASM client packaged for npm, with a dev server on `localhost:9090` and `sessionStorage`-backed state; I did not read its transport code ([tsconnect](https://github.com/tailscale/tailscale/tree/main/cmd/tsconnect)).
- **Hypercore/Holepunch.** Hyperswarm discovery is UDP-based and browsers have no UDP API. The issue records the conclusion that browser peers need signalling and a transport redesign; `hyperswarm-web` uses WebRTC between browser peers and a proxy to node peers ([hyperswarm #62](https://github.com/holepunchto/hyperswarm/issues/62)).
- **Lokinet/Session.** Lokinet is an LLARP onion router over Oxen service nodes, distributed as a native app; I found no browser client in its FAQ ([Lokinet FAQ](https://lokinet.org/faq)).

The common pattern: every browser client here uses a web transport to a certified relay/entry, or WebRTC with a fingerprint plus signalling. None dials a native peer's raw transport directly.

## 6. Certificates for home entry nodes

The decisive constraint is that **WebSocket from an HTTPS page requires `wss://` with a certificate the browser trusts**. There is no `serverCertificateHashes` equivalent for WebSocket. A GitHub Pages PWA is HTTPS, so `ws://` to a home node is blocked as mixed content, and a self-signed WSS certificate produces an interstitial JavaScript cannot bypass. Any Design A or B that is literally “NTCP2 over WebSocket” therefore needs a CA-trusted certificate. Only WebTransport (hash-pinned self-signed) and WebRTC (DTLS fingerprint) avoid CAs.

For a home node, the realistic options are:

1. **DDNS + Let's Encrypt DNS-01 + WSS.** DNS-01 writes a TXT record at `_acme-challenge.<domain>`; it works without an exposed web server, supports wildcards, and needs a DNS provider API. It cannot validate IPs. HTTP-01 needs port 80 and cannot issue wildcards; TLS-ALPN-01 needs 443, can validate IPs, but client support is limited ([challenge types, updated 2026-02-12](https://letsencrypt.org/docs/challenge-types/)). Let's Encrypt made six-day and IP-address certificates generally available in January 2026 ([announcement](https://letsencrypt.org/2025/01/16/6-day-and-ip-certs/), [GA update](https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability)). DNS-01 plus a DDNS A/AAAA record is the cleanest path: no port 80 exposure, wildcard possible, automated renewal. Inbound 443 must still be reachable; CGNAT requires a VPS/relay.
2. **WebTransport with `serverCertificateHashes`.** No domain or CA, but the certificate must be at most 14 days old and non-RSA, the client must know the hash in advance, and the server must rotate overlapping certificates. Embedding a hash in a GitHub Pages build means redeploying every 14 days; fetching it from the node at runtime is circular unless that fetch is trusted. Workable for a personal pinned node, not a public service.
3. **WebRTC Direct with a DTLS fingerprint.** The browser generates its own certificate, the server's fingerprint is in the multiaddr, and no CA is involved. A home node needs a public UDP endpoint and port forwarding; the browser's ephemeral UDP port traverses NAT because the browser initiates. Best fit for “my phone's browser connects to my home node” if the design can use WebRTC.
4. **IP-address certificates.** Useful only with a stable public IP and inbound 80/443; not suitable for CGNAT or many dynamic-IP setups. Veilid also shows an application may reject IP-based WSS even when a CA will issue the certificate.

The practical recommendation is DDNS + Let's Encrypt DNS-01 + a reverse proxy on 443, because it works with ordinary browser WebSocket/WebTransport clients and needs no custom trust. Behind CGNAT, no certificate solves reachability; the entry node must be on a VPS or behind a relay.

## 7. I2P community discussions

The I2P proposal index contains two relevant transport proposals. **Proposal 104, “TLS Transport,”** is a Draft whose design is “TBD”; its motivation is a snoop-resistant transport that looks like a Firefox HTTPS session, citing Tor as the model ([Prop 104](https://i2p.net/en/proposals/104-tls-transport)). **Proposal 109, “PT Transport,”** is an old open proposal (2014) for connecting routers through Pluggable Transports, with generic SOCKS/ExtORPort and NTCP layers ([Prop 109](https://i2p.net/en/proposals/109-pt-transport)). Design B is essentially an NTCP2 PT with WebSocket as the outer transport, so Prop 109 is its natural umbrella. I saw no WebSocket- or WebRTC-specific router transport proposal in the fetched index ([index](https://geti2p.net/spec/proposals)).

The i2pd issue history is more negative. In 2017 someone asked whether i2pd could run in a browser using Mozilla's HumbleNet; the maintainer replied it “wouldn't be able to talk to any other routers” because HumbleNet uses WebSockets and WebRTC rather than plain TCP/UDP ([i2pd #899](https://api.github.com/repos/PurpleI2P/i2pd/issues/899)). Another issue clarified that i2pd's “websockets” support was an optional UI/log event stream, not a router transport ([i2pd #922](https://api.github.com/repos/PurpleI2P/i2pd/issues/922)); it was dropped in 2020 for lack of use ([i2pd PR #1477](https://api.github.com/repos/PurpleI2P/i2pd/issues/1477), [flathub discussion](https://github.com/flathub/website.i2pd.i2pd/issues/8)). A 2025 comment there asks for WebSocket support again, but for Nostr relays, not router transport. The remaining WebSocket code in i2pd is in the client-tunnel code (`I2PTunnel.cpp`), not NTCP2 ([commit 149def0](https://github.com/PurpleI2P/i2pd/commit/149def0a5ebeb152b107122603c90083453ef8bf)).

Finally, the NTCP2 specification says it is “defined solely for point-to-point (router-to-router) transport of I2NP messages. It is not a general-purpose data pipe.” It uses Noise XK, obfuscated ephemeral keys, random padding, and a two-byte SipHash-obfuscated length; messages are at most 65,537 bytes ([NTCP2 spec](https://geti2p.net/spec/ntcp2)). Any “NTCP2 over WebSocket” design must preserve byte-stream and framing semantics, not just tunnel datagrams. Binary WebSocket frames can do that, but this is real transport integration, not a cosmetic wrapper.

## 8. Design A vs design B assessment

| | **A: user-run TCP forwarder** | **B: WS listener in the i2pd fork** |
|---|---|---|
| Browser connects to | the user's forwarder over WSS | a project entry node over WSS |
| What the middlebox learns | browser IP, ciphertext, timing; not router identity (inside Noise) | browser IP, and after the handshake the router identity; timing |
| What the destination router learns | forwarder IP, browser router identity | entry node IP, browser router identity |
| Certificate need | one CA-trusted WSS cert for the forwarder's domain | a CA-trusted WSS cert for every entry node |
| Server-side effort | low: WebSocket-to-TCP relay with destination allowlisting | high: new i2pd transport, stream adaptation, limits, deployment |
| Browser-side effort | full NTCP2 in JS/WASM | same |
| Anonymity effect | like a normal home I2P router: the first peer sees the home IP | hides home IP from other peers; entry node becomes a guard/bridge |
| Main risks | open-proxy abuse, single point, WSS cert | entry-node trust, ops/cert sprawl |

**Design A** is a transparent byte pipe. The browser runs NTCP2 end-to-end with a public I2P router; the forwarder relays bytes and cannot read the Noise stream. It does learn the browser IP and traffic metadata, and can drop/reorder traffic (NTCP2 detects this). The destination router sees the forwarder IP; it does see the browser router's static key and RouterInfo after the handshake. If the forwarder is on the user's home machine, only a colluding forwarder+destination sees both home IP and router identity. The server side can reuse WebSocket-to-TCP proxy patterns, but must allowlist I2P NTCP2 destinations/ports, authenticate the browser, limit connections and apply backpressure; otherwise it is an open TCP proxy.

**Design B** is not transparent: the entry node is the browser's NTCP2 peer and first I2P hop. It sees the browser IP and, after the handshake, the router identity, so it can link them; other peers see the entry node IP. That hides the home IP from arbitrary peers but moves trust to the entry node, which can traffic-analyze and becomes a concentration point. The fork must accept a WebSocket byte stream, feed the NTCP2 state machine, expose the result as a normal inbound peer, and handle connection limits/DoS. Every entry node needs a stable domain, certificate, DNS and monitoring.

**Certificate conclusion.** Because both designs are specified over WebSocket, both need CA-trusted WSS certificates. B does **not** escape the certificate problem: WebSocket has no certificate-hash option. Avoiding CAs requires redefining the browser transport as WebTransport (14-day pinned hashes) or WebRTC Direct (DTLS fingerprints), a larger change than “NTCP2 over WebSocket.”

**Effort conclusion.** Both need a browser-side NTCP2 implementation: X25519, ChaCha20-Poly1305, SHA-256, SipHash, AES-CBC for the obfuscated ephemeral key, and the Noise XK state machine. A's server side is commodity and can be prototyped quickly; B's is a transport feature in i2pd needing spec work, tests and deployment. For a first prototype, A is the lower-risk way to validate browser NTCP2 and PWA packaging. For a product where any project node is a first hop, B gives better UX but needs a trust/anonymity story, certificate automation (perhaps an AutoTLS-style per-node subdomain), and an entry-node operations plan. A sensible path is A now, sharing NTCP2-over-stream framing with a later B, and considering WebTransport if avoiding CAs outweighs reusing WebSocket.

## 9. Unverified

- I did not search zzz.i2p or other I2P forums; the I2P section is based on proposals and i2pd issue records only.
- Arti/WASM upstream status may have changed since the 2026-03-20 post; Tailscale `tsconnect` transport details (exact relay path) were not read, only directory/package facts.
- I did not verify whether GitHub Pages' inability to set COOP/COEP headers blocks required WASM threads/SharedArrayBuffer; this is a packaging risk, not a transport finding.
- No throughput/latency measurements were made, and Safari's WebTransport support was not tested (libp2p marks it partial).
- The exact Let's Encrypt rules for IP certificates beyond the announcement and challenge pages were not read in full.
- The claim that no I2P router currently accepts a WebSocket transport is based on the proposal index and i2pd history, not an exhaustive implementation survey; the same caveat applies to the negative results for Session/Lokinet browser clients.

## 10. Sources

Fetched during this task:

- https://www.usenix.org/system/files/usenixsecurity24-bocovich.pdf
- https://community.torproject.org/relay/setup/webtunnel/
- https://reads.ethereum.foundation/blog/embedding-arti-in-the-browser/
- https://libp2p.io/docs/browser-connectivity/
- https://libp2p.io/docs/webtransport/
- https://github.com/libp2p/specs/blob/master/webtransport/README.md
- https://raw.githubusercontent.com/libp2p/specs/master/webrtc/webrtc-direct.md
- https://veilid.com/documentation/networking/
- https://gitlab.com/veilid/veilid/-/raw/main/veilid-wasm/README.md
- https://asciimoth.github.io/ygg/
- https://github.com/asciimoth/ygg
- https://nym.com/docs/developers/clients/webassembly-client
- https://nym.com/docs/developers/mix-websocket
- https://github.com/holepunchto/hyperswarm/issues/62
- https://github.com/tailscale/tailscale/tree/main/cmd/tsconnect
- https://letsencrypt.org/docs/challenge-types/
- https://letsencrypt.org/2025/01/16/6-day-and-ip-certs/
- https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability
- https://geti2p.net/spec/proposals
- https://i2p.net/en/proposals/104-tls-transport
- https://i2p.net/en/proposals/109-pt-transport
- https://geti2p.net/spec/ntcp2
- https://api.github.com/repos/PurpleI2P/i2pd/issues/899
- https://api.github.com/repos/PurpleI2P/i2pd/issues/922
- https://api.github.com/repos/PurpleI2P/i2pd/issues/1477
- https://github.com/flathub/website.i2pd.i2pd/issues/8
- https://github.com/PurpleI2P/i2pd/commit/149def0a5ebeb152b107122603c90083453ef8bf
- https://lokinet.org/faq
