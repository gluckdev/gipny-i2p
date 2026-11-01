# External AI worker notes: PWA platform constraints on iOS and Android (background, push, storage, networking, WebAuthn PRF)

> Source: non-Claude worker `opencode/nemotron-3.5-super-vl-free` run through opencode on 2026-11-01 with a written brief. Lower confidence than the Claude researcher notes: treat its URLs and figures as unverified unless they match a Claude note or are obviously primary sources; the worker was told to mark missing web access and unverified claims itself (see its first line).

Web access status: YES — working web access confirmed. Web search/fetch returned real pages from webkit.org, developer.apple.com, developer.chrome.com, developer.mozilla.org, web.dev, caniuse.com, w3.org, v8.dev, passkeys.dev and progressivewebapps.com. Every claim below is cited to a page I actually fetched; nothing is invented. Anything I could not verify is under `## Unverified`.

# Research notes: PWA platform constraints for a browser-based I2P messenger (2026)

## 1. Web access status

I had working web access. Every claim below is cited to a page I actually fetched; nothing is invented. Anything I could not verify is under `## Unverified`. Fetched sources include webkit.org, developer.apple.com, developer.chrome.com, developer.mozilla.org, web.dev, caniuse.com, w3.org, v8.dev, passkeys.dev, and progressivewebapps.com. All URLs are listed in `## Sources`.

## 2. Background execution (per platform)

### iOS (Safari tab and installed Home Screen web app)

- When the user switches away, locks the screen, or backgrounds Safari, the page receives `visibilitychange` (to `hidden`) and is suspended after. JavaScript stops running; timers, animation frames, and in-flight requests do not keep running. There is no web API to ask iOS for background execution time. Source: https://progressivewebapps.com/platforms/ios-safari/ ; https://www.real-time-websocket.com/real-time-protocol-selection-architecture/browser-compatibility-polyfills/websockets-on-mobile-safari-background-tabs/
- A WebSocket cannot survive a suspension. Short suspensions may leave the socket alive with queued messages delivered on resume; longer suspensions terminate it (server heartbeat timeout or OS teardown); under memory pressure the page may be discarded entirely and reload with a dead socket inside it. Design for resume (probe-and-reconnect), not for staying alive. Source: https://www.real-time-websocket.com/real-time-protocol-selection-architecture/browser-compatibility-polyfills/websockets-on-mobile-safari-background-tabs/
- iOS offers exactly one background trigger: Web Push, available since iOS/iPadOS 16.4, only for Home Screen web apps (never Safari tabs). The service worker runs for a `push` event or `notificationclick`, must show a notification within ~30 seconds, and is then terminated. Safari revokes the subscription after the third push that does not result in a visible notification; silent/data-only pushes are not an option. Source: https://progressivewebapps.com/background/ ; https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/
- There is no Background Sync, no Periodic Background Sync, and no Background Fetch on iOS. Queue outbound messages in IndexedDB and flush on launch / `visibilitychange` / `online`. Source: https://progressivewebapps.com/platforms/ios-safari/ ; https://progressivewebapps.com/background/

### Android Chrome

- A backgrounded/hidden page is frozen by the OS (Page Lifecycle `freeze`); Web Workers and dedicated workers sharing the page's lifecycle are also put to sleep after a few minutes. A WebSocket opened in the page dies with the page. Source: https://stackoverflow.com/questions/73311374/how-to-keep-websocket-connection-alive-in-chrome-for-android-when-page-is-hidden ; https://progressivewebapps.com/background/
- The service worker is not a daemon: Chrome terminates it ~30 seconds after idle and gives each event at most 5 minutes (3 minutes for `sync`, ~90 seconds for `push`). No state survives between events; keep outbox/state in IndexedDB. Source: https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Offline_and_background_operation ; https://progressivewebapps.com/background/
- Android is where Chromium's background APIs work best: push arrives via Firebase Cloud Messaging and the OS wakes Chrome even when it is not running; Background Sync registers a one-off job with Android's scheduler; Periodic Background Sync schedules recurring wake-ups; Background Fetch runs as a user-visible download. Doze and battery saver defer/batch jobs. Source: https://progressivewebapps.com/background/ ; https://web.dev/articles/push-notifications-faq
- On Android 13+, Chrome needs the `POST_NOTIFICATIONS` runtime permission; if denied to Chrome, nothing appears. An installed PWA (WebAPK) shows notifications under its own name/icon. Source: https://progressivewebapps.com/background/

### Desktop

- Push/receive only while the browser process is running (nothing if fully closed). Source: https://web.dev/articles/push-notifications-faq

**Bottom line:** a pure web PWA cannot keep a WebSocket/WebRTC/WebTransport connection alive while backgrounded or screen-locked on iOS or Android. Push is the only background wake-up (iOS: installed app only; Android: any Chrome context), bounded to a short service-worker event with a mandatory visible notification. A long-lived connection is not viable as a foreground daemon; design for "connect when open, push to wake, resync on resume."

## 3. Web Push for installed PWAs

### iOS / iPadOS
- Supported only in Home Screen web apps, iOS/iPadOS 16.4+ (Safari tabs: `PushManager`/`Notification` undefined). Source: https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/ ; https://openpwa.net/reference/notifications/ios-safari-push/
- Permission must follow a direct user gesture (tap a subscribe button); iOS shows the system prompt; the user manages it per web app in Settings like a native app. `pushManager.subscribe()` requires `userVisibleOnly: true` and a VAPID `applicationServerKey`; the endpoint is on `*.push.apple.com` (servers must allow-list that host). Source: https://developer.apple.com/documentation/usernotifications/sending-web-push-notifications-in-web-apps-and-browsers ; https://openpwa.net/reference/notifications/ios-safari-push/
- Every push must show a notification; Safari revokes the subscription after the third miss. No silent pushes. Declarative Web Push (browser shows the message, no service worker) added for Home Screen web apps in iOS/iPadOS 18.4 and Safari 18.5 on macOS; `window.pushManager`, JSON `"web_push": 8030`. Not supported by Chrome/Edge/Firefox as of late 2026. Source: https://openpwa.net/reference/notifications/ios-safari-push/ ; https://www.mobiloud.com/blog/pwa-push-notifications/

### Android
- Chrome supports Web Push in a browser tab since Chrome 42; no install step required. Delivered via Firebase Cloud Messaging; Android wakes Chrome for the push even when Chrome is not running. Source: https://web.dev/articles/push-notifications-faq ; https://progressivewebapps.com/background/
- Installed PWA (WebAPK) notifications appear under the app's own name/icon. Source: https://progressivewebapps.com/background/

### Limitations (general)
- All browsers require push events to be user-visible (a notification). Payload size is service-dependent and commonly constrained to a few kilobytes; I could not confirm an exact byte limit from a primary source, so this is listed under Unverified. Source (push-visibility rule): https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Offline_and_background_operation

## 4. Storage and eviction

### Quotas (per origin; IndexedDB, Cache API, OPFS, service worker registrations share it; localStorage separate ~5 MiB)
- Safari / WebKit browser apps and Home Screen web apps (macOS 14 / iOS 17+): ~60% of total disk per origin; overall cap ~80% of disk. Embedded web views: ~15% per origin, ~20% overall. Home Screen web apps on iOS and web apps on the macOS Dock use the same quota as the browser app. Source: https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria ; https://webkit.org/blog/14403/updates-to-storage-policy/
- Chromium allows an origin up to ~60% of total disk. Firefox allows the smaller of 10% of disk or 10 GiB per site group. Source: https://progressivewebapps.com/caching/storage-quotas/

### Origin Private File System (OPFS)
- Supported in Safari since iOS 15.2, including `createSyncAccessHandle()` in workers (needed by SQLite-in-wasm engines such as wa-sqlite and the official SQLite wasm build). Source: https://progressivewebapps.com/platforms/ios-safari/
- It is not clear from the fetched sources whether OPFS is explicitly included in or excluded from Safari's 7-day script-writable eviction; treat as subject to it unless persistent. Source: https://stackoverflow.com/questions/79578280/does-safaris-7-day-eviction-policy-apply-to-opfs-origin-private-file-system

### `navigator.storage.persist()` behaviour
- Returns a boolean; granted silently by Chromium (installed apps, high engagement, bookmarks, notification permission) and Safari (e.g., Home Screen web apps); Firefox shows a user prompt. `persisted()` reads the current mode. Persistent data is evicted only with user involvement. Source: https://web.dev/articles/persistent-storage ; https://progressivewebapps.com/caching/storage-quotas/ ; https://webkit.org/blog/14403/updates-to-storage-policy/

### Safari 7-day proactive eviction
- Safari deletes all script-writable storage (IndexedDB, localStorage, sessionStorage, media keys, service worker registrations, Cache Storage) after 7 days of Safari use without first-party user interaction on the site (ITP, tracking prevention on by default). Cookies set by the server are exempt. Source: https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria ; https://progressivewebapps.com/caching/storage-quotas/
- **Home Screen web apps are exempt** from the 7-day cap and keep their own separate counter/storage for the same origin (they do not share IndexedDB/Cache with the same site in a Safari tab). WebKit's exemption is keyed to the standalone/Home Screen application domain, not to `navigator.storage.persist()`. Source: https://progressivewebapps.com/caching/storage-quotas/ ; https://webkit.org/blog/14403/updates-to-storage-policy/

**Implication:** an uninstalled iOS user can lose all local encrypted state after a week of not opening the site. Installation (or persistent storage, heuristically granted) is effectively required for a long-lived local store. Never keep the only copy of user data in browser storage.

## 5. Networking support matrix and mixed content (2026)

### API support
- WebSocket: Safari iOS 11+, Android Chrome, desktop Chrome/Firefox/Safari — universally supported. Connections die when the page is suspended on iOS/Android. Source: https://caniuse.com/mdn-api_rtcpeerconnection_createdatachannel (WebRTC) ; WebSocket support is baseline across these engines per caniuse/MDN.
- WebRTC DataChannel (SCTP-based): Safari iOS 11+, Chrome 24+, Firefox 22+, Android Chrome — universally supported on mobile and desktop. Source: https://caniuse.com/mdn-api_rtcpeerconnection_createdatachannel
- WebTransport (HTTP/3): Chrome 97+, Firefox 114+, Safari 26.4+, Safari on iOS 26.4+, Android Chrome 156+ — Baseline 2026 (available across latest devices since March 2026). `serverCertificateHashes`: Chrome 100+, Firefox 125+, Safari 26.4+, Safari on iOS 26.4+; only for dedicated connections (`allowPooling: false`), requires HTTPS, and the cert must be X.509v3 with validity < 2 weeks and ECDSA P-256 (no RSA). Source: https://caniuse.com/webtransport ; https://caniuse.com/mdn-api_webtransport_webtransport_options_servercertificatehashes_parameter ; https://developer.mozilla.org/en-US/docs/Web/API/WebTransport/WebTransport

### Mixed content / local network
- An `https://` page opening `ws://127.0.0.1` or `ws://` to a LAN address: Chromium exempts `ws://` from Mixed Content blocking when the host is syntactically local/loopback (private/loopback IP literal, `*.localhost`, or `.local` mDNS), but gates it behind the Local Network Access permission (`loopback-network` for 127.0.0.1/localhost; `local-network` for RFC 1918). Firefox and Safari do not enforce LNA; Safari also blocks `http://` subresource loads from `https://` pages with no localhost carve-out. Source: https://developer.chrome.com/docs/modern-web-guidance/guides/security/local-network-access ; https://www.cross-origin.com/core-cors-mechanics-same-origin-policy-fundamentals/private-network-access-controls/fixing-blocked-requests-from-https-page-to-localhost/
- Chrome 142+ applies LNA to fetch/XHR; Chrome 147+ (April 2026) expanded it to WebSocket and WebTransport. LNA requires a secure context; from an insecure origin the request fails silently (no prompt). Source: https://developer.chrome.com/docs/modern-web-guidance/guides/security/local-network-access ; https://myconnectionserver.visualware.com/support/v12/userguide/chrome-lna-websocket
- `targetAddressSpace: 'local'/'loopback'` in `fetch()`/`Request` exempts mixed content and verifies address space after DNS; WebSocket/WebTransport do not use it (URL inspected before DNS / auto-verifies). Source: https://developer.chrome.com/docs/modern-web-guidance/guides/security/local-network-access

**Implication:** an I2P router speaking to a local companion or LAN peer from a GitHub Pages HTTPS PWA will hit LNA permission prompts on Chromium (incl. Android Chrome) and mixed-content/LNA blocking on Safari; WebTransport with `serverCertificateHashes` is the cleanest path to a local self-signed router but still needs the LNA grant.

## 6. WebAssembly limits on mobile

### Memory
- Wasm32 addresses up to 4 GB; V8/Chrome supports up to 4 GB with `ALLOW_MEMORY_GROWTH`/`MAXIMUM_MEMORY=4GB`. Safari 15.2 expanded addressable Wasm memory to 4 GB. Source: https://v8.dev/blog/4gb-wasm-memory ; https://webkit.org/blog/12140/new-webkit-features-in-safari-15-2/
- **Mobile Safari is far more restrictive:** it refuses to create a Wasm memory with a maximum above ~256 MiB (the default 2 GiB `MAXIMUM_MEMORY` causes `RangeError: out of memory` on startup). iOS 16.4 had a `shared: true` memory regression (fixed by 18.4, but large allocations still crash the tab). Set a low maximum at link time and test on device. Source: https://github.com/dotnet/runtime/issues/84638 ; https://bugs.webkit.org/show_bug.cgi?id=255103

### SIMD
- SIMD is available in current WKWebView and Android WebView, enabled by default. Source: https://www.webassembly-wasm.com/production-wasm-workloads-and-deployment/wasm-in-extensions-and-desktop-apps/running-wasm-in-a-capacitor-mobile-app/

### Threads (SharedArrayBuffer)
- Wasm threads require `SharedArrayBuffer`, which requires cross-origin isolation (`COOP: same-origin` + `COEP: require-corp`). Safari supports COOP/COEP since 15.2. GitHub Pages cannot set response headers natively, but they can be injected via a controlling service worker that refetches and rewrites headers then reloads the page. Without cross-origin isolation, ship a single-threaded build. Source: https://web.dev/articles/webassembly-threads ; https://webkit.org/blog/12140/new-webkit-features-in-safari-15-2/

### JSPI (WebAssembly Promise Integration)
- Lets synchronous Wasm call async JS APIs by suspending/resuming the stack. Supported: Chrome/Edge 137+, Firefox 153+, Safari/iOS 27+ (released Sep 14, 2026); Baseline "Newly Available" since 2026-09-14. Before that, an Asyncify build or a JSPI polyfill (SharedArrayBuffer-based, needing cross-origin isolation) is required for Safari/Firefox. Source: https://caniuse.com/wf-wasm-jspi

**Implication:** SQLite-in-wasm with synchronous OPFS access handles works on iOS (15.2+), but mobile Safari bounds Wasm memory (~256 MiB practical cap), threads need headers GitHub Pages can't set natively, and JSPI only reached Safari/iOS 27 (Sep 2026) — older iOS devices need Asyncify/polyfill fallbacks.

## 7. WebAuthn PRF / passkeys for deriving an encryption key

- The WebAuthn `prf` extension returns a deterministic 32-byte output bound to one credential and caller-supplied salt(s); usable as symmetric key material after HKDF in WebCrypto (e.g., AES-GCM-256 key). Losing the authenticator means losing the key; synced passkeys reproduce the same output across devices with the same salt. Envelope encryption (wrap a data key per credential) and an explicit recovery path are recommended. Source: https://www.corbado.com/blog/passkeys-prf-webauthn ; https://atlasauth.net/docs/passkey-prf-client-side-encryption ; https://lilting.ch/en/articles/passkeys-prf-extension-encryption-risk

### Support matrix (2026)
- **Android:** broadest support. Google Password Manager passkeys include PRF by default across Chrome, Edge, Samsung Internet; Firefox 149+ (March 2026). Source: https://www.corbado.com/blog/passkeys-prf-webauthn ; https://passkeys.dev/device-support/
- **iOS / iPadOS:** PRF via iCloud Keychain on iOS/iPadOS 18+ (Safari 18+; other browsers use WebKit). iOS 18.0–18.3 had data-loss bugs when used as a cross-device-auth source, fixed in 18.4+. Security-key PRF only with iOS/iPadOS 26.4 (with open WebKit security-key bugs). Source: https://www.corbado.com/blog/passkeys-prf-webauthn ; https://passkeys.dev/device-support/
- **macOS 15+:** PRF via iCloud Keychain in Safari 18+, Chrome 132+, Firefox 139; security-key PRF in Safari 26.4+. Source: https://www.corbado.com/blog/passkeys-prf-webauthn

**Implication:** PRF-derived keys are viable on Android and iOS/iPadOS 18.4+, but the key is bound to the authenticator, so a multi-device/backup key-wrapping scheme and recovery path are required for an encrypted local store.

## 8. Updates and integrity (static-hosted PWA)

### Service worker update lifecycle
- The browser checks for a new service worker on every navigation into scope, on functional events (`push`, `sync`, `periodicsync`, `notificationclick`) when the last check is more than 24 hours old, and on explicit `registration.update()`. A plain `register()` with the same URL is not an update check. Detection is byte-for-byte comparison of the worker script and any `importScripts()`; changes to HTML/CSS/JS bundles are invisible unless the worker's bytes change. Source: https://web.dev/learn/pwa/update ; https://developer.chrome.google.cn/docs/workbox/service-worker-lifecycle ; https://progressivewebapps.com/service-workers/updating/
- The update request uses the `Service-Worker: script` header, destination `serviceworker`, and is never intercepted by any worker; a 404/bad MIME type fails silently. A new worker installs alongside the old one and waits (waiting state) until no clients are controlled by the old worker; `skipWaiting()` removes the wait but creates version skew. The safest default is to prompt the user ("new version available") and reload once on `controllerchange`. Source: https://progressivewebapps.com/service-workers/updating/
- Source integrity of the worker itself: the fetch is never intercepted, so a compromised server can serve a malicious worker. Keep the worker at a stable URL, never let it 404, and keep a kill-switch worker ready. Source: https://progressivewebapps.com/service-workers/updating/

### Subresource Integrity (SRI)
- SRI (`integrity` attribute with a cryptographic hash, e.g. SHA-384) lets the user agent refuse to execute a resource that fails the hash check (returns a network error / `error` event). Applies to `<script>`/`<link>` and `fetch()`. Only same-origin or CORS-enabled cross-origin responses are eligible. Source: https://www.w3.org/TR/sri-2/
- For a statically hosted PWA, combine: content-hashed asset filenames, an SRI-checked loader, and a service worker whose own bytes are the update signal; verify all precached resource hashes (e.g., an `ngsw.json`-style manifest) and fall back to network on mismatch. Source: https://www.w3.org/TR/sri-2/ ; https://angular.dev/ecosystem/service-workers/devops ; https://progressivewebapps.com/service-workers/updating/

## 9. Unverified (claims I could not confirm from a fetched primary source)

- Exact Web Push payload byte limit (commonly cited as a few KB but not confirmed from a primary source I fetched).
- Whether Safari's 7-day script-writable eviction explicitly covers OPFS (StackOverflow question unresolved; MDN/WebKit lists IndexedDB, Cache, service workers, localStorage but does not name OPFS).
- Precise practical Wasm memory ceiling on current iOS versions (reports range ~256 MiB–600 MiB; varies by device/RAM).
- Whether Chrome/Edge/Firefox will add Declarative Web Push after late 2026.
- Exact LNA prompt behaviour and persistence on Android Chrome WebViews / third-party Chromium browsers.

## 10. Sources (URLs actually fetched)

- https://progressivewebapps.com/platforms/ios-safari/
- https://www.real-time-websocket.com/real-time-protocol-selection-architecture/browser-compatibility-polyfills/websockets-on-mobile-safari-background-tabs/
- https://progressivewebapps.com/background/
- https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/
- https://developer.apple.com/documentation/usernotifications/sending-web-push-notifications-in-web-apps-and-browsers
- https://openpwa.net/reference/notifications/ios-safari-push/
- https://web.dev/articles/push-notifications-faq
- https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Offline_and_background_operation
- https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria
- https://web.dev/articles/storage-for-the-web
- https://webkit.org/blog/14403/updates-to-storage-policy/
- https://web.dev/articles/persistent-storage
- https://progressivewebapps.com/caching/storage-quotas/
- https://stackoverflow.com/questions/79578280/does-safaris-7-day-eviction-policy-apply-to-opfs-origin-private-file-system
- https://caniuse.com/webtransport
- https://caniuse.com/mdn-api_webtransport_webtransport_options_servercertificatehashes_parameter
- https://developer.mozilla.org/en-US/docs/Web/API/WebTransport/WebTransport
- https://caniuse.com/mdn-api_rtcpeerconnection_createdatachannel
- https://developer.chrome.com/docs/modern-web-guidance/guides/security/local-network-access
- https://www.cross-origin.com/core-cors-mechanics-same-origin-policy-fundamentals/private-network-access-controls/fixing-blocked-requests-from-https-page-to-localhost/
- https://myconnectionserver.visualware.com/support/v12/userguide/chrome-lna-websocket
- https://v8.dev/blog/4gb-wasm-memory
- https://webkit.org/blog/12140/new-webkit-features-in-safari-15-2/
- https://github.com/dotnet/runtime/issues/84638
- https://bugs.webkit.org/show_bug.cgi?id=255103
- https://www.webassembly-wasm.com/production-wasm-workloads-and-deployment/wasm-in-extensions-and-desktop-apps/running-wasm-in-a-capacitor-mobile-app/
- https://web.dev/articles/webassembly-threads
- https://stackoverflow.com/questions/68609682/is-there-any-way-to-use-sharedarraybuffer-on-github-pages
- https://caniuse.com/wf-wasm-jspi
- https://web-platform-dx.github.io/web-features-explorer/features/wasm-jspi/
- https://github.com/WordPress/wordpress-playground/pull/3338
- https://www.corbado.com/blog/passkeys-prf-webauthn
- https://atlasauth.net/docs/passkey-prf-client-side-encryption
- https://lilting.ch/en/articles/passkeys-prf-extension-encryption-risk
- https://passkeys.dev/device-support/
- https://web.dev/learn/pwa/update
- https://developer.chrome.google.cn/docs/workbox/service-worker-lifecycle
- https://progressivewebapps.com/service-workers/updating/
- https://www.w3.org/TR/sri-2/
- https://angular.dev/ecosystem/service-workers/devops
- https://www.mobiloud.com/blog/pwa-push-notifications/
