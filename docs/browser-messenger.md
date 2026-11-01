# Browser messenger

The browser entry is `ui/browser.html`. It uses the existing Gipny screens with
a Dedicated Worker and a Rust WASM core. It does not pair with a native client.
The feature is under review in [PR #144](https://github.com/gluckdev/gipny-i2p/pull/144).
The live messenger acceptance run must pass before this is treated as ready.

## Runtime and storage

The worker runs the pinned emissary client router and an in-process streaming
destination. Its default exploratory and client pools each use two-hop paths.
There are no SAM/I2CP TCP listeners and no transit service. NTCP2 remains
encrypted between the browser router and I2P peers; the WSS forwarder transports
bounded byte streams to addresses verified from RouterInfo records.

The Rust core owns the messenger identity, signed/one-time prekeys, destination
private keys and ratchet state. X3DH, Double Ratchet, message padding and native
relay/message formats are shared with `libcore`. JavaScript receives public
identity information and the plaintext needed to display conversations.

Profiles use OPFS in a worker, exclusive sync handles and a Web Lock. Two slots
alternate encrypted records. XChaCha20-Poly1305 authenticates contents and record
headers; a separate checksum detects incomplete writes. The newest complete
record is selected before checking credentials, so changing a password does not
allow an old password to unlock the older slot. File I/O errors are surfaced.

Password wrapping uses Argon2id with 64 MiB, three iterations and one lane.
Passkeys are enrolled only when WebAuthn returns a real 32-byte PRF result.
Credential signatures and IDs are never substituted for the encryption secret.
Keep the password as a recovery method when an authenticator is unavailable.

An outgoing ratchet advance and its exact ciphertext outbox entry are encrypted,
written and flushed before sending. Incoming messages and receipts are flushed
before acknowledging the deposit. A failed write stops the profile and requires
reopening it; the old ratchet is never reused after an ambiguous write.

Locking stops the network, closes storage and terminates the worker. Navigation
and page freeze also terminate it. Returning to a retained page reloads the
locked UI. An offline service worker caches the application shell and generated
WASM/worker assets, excluding network configuration, snapshots and profile data.
Updates wait for old application windows to close.

Browser memory does not provide the native client's `mlock`/`mprotect` boundary.
Rust secret buffers are zeroized when released; JavaScript plaintext and the
browser's own memory/storage lifecycle have different guarantees. OPFS remains
subject to browser quota and site-data removal.

## Validation and artifacts

All builds and test execution run in GitHub Actions. The `Browser router WASM`
workflow builds `ui/browser-dist` after generating the WASM package and uploads
`browser-messenger-pwa`. Its native integration job obtains a fresh verified
netDb snapshot, then runs two Chrome processes with independent data directories.

`browser-router/tests/run_messenger_ci.py` checks actual production worker RPC,
WebAuthn PRF with a virtual CTAP2 authenticator, two live client destinations,
bidirectional encrypted messages, delivery receipts, an inline attachment and
worker restart with restored identity/destination/history/ratchet and offline
outbox retry. Public receipts are uploaded separately. Passwords, capabilities,
CDP traffic and private browser directories are excluded from artifacts.

The shared crypto/codec tests also check tampering, identity pinning, replay,
out-of-order messages and fixed-integer bincode compatibility. Vault tests check
wrong credentials, password rotation, passkey wrapping and torn-slot recovery.
An emulated or virtual authenticator test is not physical mobile validation.

## Hosting preparation

Serve the artifact over HTTPS with `application/wasm` for `.wasm`, no third-party
scripts, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` and
`X-Frame-Options: DENY`. Use `/browser.html` as the start page. Preserve the CSP in
the HTML and its `wasm-unsafe-eval` permission. Hosting under a subpath requires
building with the corresponding `VITE_BASE` in GitHub Actions.

Provide an uncached `browser-config.json` beside `browser.html`:

```json
{
  "snapshot_url": "https://chat.example.org/snapshot.tar.gz",
  "snapshot_sha256": "<SHA256 from the snapshot manifest>",
  "forwarder_url": "wss://forwarder.example.org:8443/stream",
  "capability": "<43-character unpadded base64url token>"
}
```

The capability is visible to the browser. It is a forwarder protocol boundary,
not a profile password or protection against a malicious client that knows it.
The forwarder still enforces exact Host/Origin, verified public IPv4 targets,
freshness, connection/rate/byte/time limits and TLS. Never enable handshake debug
logging or arbitrary target registration.

A matching TLS forwarder command, using separately supplied certificates and a
verified `candidates.tsv`, is:

```sh
python3 browser-router/forwarder/server.py \
  --listen 0.0.0.0:8443 \
  --host forwarder.example.org:8443 \
  --origin https://chat.example.org \
  --capability-file /run/gipny/forwarder-capability \
  --candidates /srv/gipny/candidates.tsv \
  --tls-cert /run/gipny/fullchain.pem \
  --tls-key /run/gipny/privkey.pem \
  --max-connections 32 --session-seconds 600 --idle-seconds 120 \
  --max-bytes 67108864
```

Snapshot generation uses `collect-netdb` and `prepare-netdb`, built in GitHub
Actions with `native-peer`. The packer records the actual committed i2pd pin and
refuses a mismatching checkout. Publish the snapshot, its SHA256 configuration
and the corresponding allowlist together. The current forwarder loads its
allowlist at startup; refreshing it requires a controlled restart. Expired
RouterInfo targets are rejected. A stale allowlist is not a permanent service.

A public server/domain, certificate access and physical mobile browsers still
need to be supplied for deployment and device acceptance. This branch prepares
the code and artifacts; it does not publish a public service.

The first browser transport supports direct messages, group fanout and inline
attachments up to 1 MiB per message within a 16 MiB encrypted profile. Native
console/agent execution, large multipart file transfer, DHT discovery, transit,
Yggdrasil, native installers, destructive attempt counters and duress profiles
require separate browser implementations. Unsupported commands return errors.
