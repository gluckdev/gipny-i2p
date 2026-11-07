# Исторические проверки браузера / Historical browser validation

Записи ниже относятся только к указанным SHA. Новые WebRTC/Rust-forwarder изменения ими не проверены. / The results below apply only to their recorded commits, not the new transport.

[Текущее руководство / Current guide](../browser-messenger.md)

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

Latest validated code: `fa60b02`, 2026-11-01,
[run 41329839707](https://github.com/gluckdev/gipny-i2p/actions/runs/41329839707).
The complete automated two-browser acceptance passed in 464.10 s.
All native platform/build checks and CodeQL also passed; there were no open
CodeQL alerts for PR #144 at this checkpoint. Documentation-only follow-ups
record this tested code SHA rather than claiming a new code validation.

| Check | Result at `fa60b02` |
|---|---|
| Production PWA/WASM, UI tests and typecheck | PASS |
| WASM tests | 46/46 PASS, including 9/9 engine tests |
| Native recovery regressions, SessionManager and routing-path modules | 3 explicit regressions, 89 session tests and 14 routing-path tests PASS |
| Real NTCP2 handshakes | 3/3 PASS |
| Cold desktop / constrained Chrome | Both PASS: 38.31 / 145.07 s from navigation to ready, 32 / 32 s held, 3 / 3 roundtrips, 245 verified RouterInfo records |
| Live bidirectional messages, receipts and attachment name/bytes | PASS in two independent Chrome processes |
| Actual WebAuthn PRF enrollment/unlock with virtual CTAP2 authenticator | PASS |
| Blocked contact and unblocking exact-ciphertext retry | PASS |
| Password rotation, both-slot wrappers, old-password rejection, restored identity/destination/history/ratchet | PASS |
| Pending-message delivery after recipient restart, end-to-end receipt and duplicate-history check | PASS |
| Offline root navigation, cached WASM worker and rendered history with the HTTP server stopped | PASS; network config excluded from caches |
| Physical devices / public deployment | Not performed |

The public
[`browser-messenger-evidence` receipt](https://github.com/gluckdev/gipny-i2p/actions/runs/41329839707/artifacts/13093552039)
records all seven acceptance assertions with `stage: passed`.
The
[`browser-messenger-pwa` ZIP](https://github.com/gluckdev/gipny-i2p/actions/runs/41329839707/artifacts/13093200119)
is 1,982,901 bytes; GitHub artifact SHA256:
`64f3d0462ba2061ce0db47d2cb5fd75740daa4efd0db5a47c5ed8ddd38bdad8f`.
The verified emissary patch SHA256 is
`1ba0d7861a647ced678a93e00c8e0efdf236fb6c3273f94f832f02451568e553`.
This is a successful desktop CI checkpoint, not physical mobile validation
or a public deployment. Earlier failures remain part of the evidence below.

Earlier run `bee4456`,
[41326667198](https://github.com/gluckdev/gipny-i2p/actions/runs/41326667198),
passed 46 WASM tests, three recovery regressions, 89 session tests, NTCP2,
both cold profiles and the native platform/CodeQL checks. Cold readiness was
180.31 / 88.47 s with 153 verified RouterInfo records. Live bidirectional
messages, receipts, attachment bytes and PRF passed; the next blocked-contact
mailbox deposit timed out after 120 s, before restart and offline checks.
The sender reported `CANT_REACH_PEER` and a newer LeaseSet lookup while both
clients still had tunnels. `fa60b02` then fixed the route-cache mismatch:
superseded unexpired leases had remained eligible after the destination cache
was replaced with a newer complete set.

Earlier run `3e0ab82`,
[41322770968](https://github.com/gluckdev/gipny-i2p/actions/runs/41322770968),
passed the three recovery regressions, all 89 native session tests, 46 WASM
tests and both cold profiles. Live restart delivery timed out after 180 s.
Numeric diagnostics showed two encryption-key changes at the sender, repeated
non-newer lookup results and failed probes from the restored recipient.

Earlier run `b93fdc7`,
[41317354451](https://github.com/gluckdev/gipny-i2p/actions/runs/41317354451),
passed both native recovery regressions and all 46 WASM tests. Desktop and
constrained cold bootstrap passed (162.31 / 178.52 s to ready, 32 / 33 s held,
3 / 5 roundtrips, 157 verified RouterInfo records). Live bidirectional messages,
receipts, PRF, blocking and restored identity/address passed. Restart delivery
timed out after 180 s with six sender `DialFailure` errors.

The earlier intermediate `e6709dd` run passed all 46 WASM tests, but stopped at the new
native regression: its initial empty-queue assertion mistook local-destination
registration for a remote lookup. That assertion is corrected in source.
The follow-up also resets stale I2P ECIES sessions when the signed LeaseSet
encryption key changes and checks decryption by a restarted recipient. These
changes were subsequently checked in the later runs above; the stopped run
itself provided no new live result.

Earlier live evidence: `f72d7f4`, 2026-11-01,
[Actions run 41309131811](https://github.com/gluckdev/gipny-i2p/actions/runs/41309131811).

| Check | Result |
|---|---|
| Release PWA/WASM, UI tests and typecheck | PASS |
| WASM tests | 46/46 PASS, including 9/9 engine tests |
| Native fast/security matrix, build and CodeQL | PASS |
| Live destinations, bidirectional messages, receipts and attachment | PASS in two independent Chrome processes |
| Actual WebAuthn PRF helper with virtual CTAP2 authenticator | PASS |
| Blocked contact and unblocking exact-ciphertext retry | PASS |
| Password rotation, both-slot wrappers, old-password rejection and restored identity/address | Passed before the later delivery timeout |
| Delivery after recipient restart | FAILED: pending message did not arrive within 180 s after the restored destination became ready |
| Cold desktop / constrained bootstrap | Desktop PASS (230.29 s to ready, 32 s held, 3 roundtrips); constrained FAILED (no inbound tunnel by deadline) |
| Offline shell with HTTP server stopped | Not reached |
| Physical devices / public deployment | Not performed |

The recovery implementation refreshes a cached remote LeaseSet after a failed streaming
handshake. A peer can restart on the same destination keys with different
inbound tunnels while its previous LeaseSet is still unexpired. This source
change passed its native regression in later runs; the full automated live
acceptance subsequently passed at `fa60b02` above.
Failed refresh queries also keep the cache marked stale until a valid lookup
succeeds, rather than immediately reusing the old unexpired address.

Earlier baseline: `bb0578e`, 2026-11-01:

| Check | Result |
|---|---|
| Release WASM, production PWA and UI tests | PASS |
| WASM tests in Chrome | 45/45 PASS, including 9/9 BrowserEngine tests |
| Real NTCP2 handshakes | PASS |
| Cold bootstrap, desktop | PASS: 50.28 s from navigation, 33 s sustained availability, 5 roundtrips |
| Cold bootstrap, constrained Chrome | PASS: 34.35 s, 33 s sustained availability, 3 roundtrips; 4× CPU slowdown, 80 ms latency, 200/100 KiB/s down/up |
| Two independent browser messengers | FAILED during startup: lower-level snapshot URL validation rejected `localhost`; corrected and confirmed by later live runs above |
| Physical phones and public deployment | Not performed |

Source: [Actions run 41306799095](https://github.com/gluckdev/gipny-i2p/actions/runs/41306799095).
Both cold probes used a fresh snapshot with 175 verified RouterInfo records.
The loadable module contained 3,165,432 bytes WASM and 62,471 bytes JS.
The `browser-messenger-pwa` artifact ZIP was 1,977,472 bytes with SHA256
`b85c623f7abf16ce7f34eb06fda20676a334866ecb2462ff978b234f9e415546`.
These are measurements of that commit, not promises for every network or device.
The earlier constrained run at `e22a800` timed out; live I2P startup varies.
The successful WASM job does not make the failed integration workflow green.
Router-only bootstrap memory figures exclude the 64 MiB password-KDF workspace
and unlocked conversation state. They are not measurements of full messenger RAM.

The acceptance scenario must also reach offline history after stopping the HTTP
server. `fa60b02` reached and passed that final phase. Later code changes must
be checked against their own runs.

### Recovery diagnostics

On failure, `browser-messenger-evidence/receipt.json` includes bounded transport
status strings and `network_metrics` for the two clients, in Alice/Bob order.
The `browser_diagnostics` worker RPC returns numeric counters/gauges and
`relay_probes` totals; it does not return destination identifiers, message
contents, credentials or keys. Router counters reset when a new runtime starts.

| Signal | Meaning |
|---|---|
| `client_lease_refresh_requested`, `client_lease_query_success`, `client_lease_query_failure` | Forced refresh requests and completed lookup outcomes |
| `client_newer_lease_received`, `client_unchanged_lease_received` | Accepted lookup publication compared with the cached record; a discarded older reply is counted as unchanged |
| `client_stale_lease_query_ignored`, `client_stale_garlic_lease_ignored`, `client_stale_encryption_key_ignored` | An older signed publication was prevented from replacing newer state |
| `client_lease_encryption_key_changed` | A changed peer transport key caused old ECIES sessions/tags to be retired; application ratchets are separate |
| `client_stream_syn_rejected` | Streaming SYN retries were exhausted before the stream opened |
| `client_selected_lease_missing` | A selected route referenced an inbound tunnel absent from the destination cache |
| `relay_probes.attempted/completed/failed` | Authenticated Ping/Pong attempts to known contacts after network startup |

`CANT_REACH_PEER` identifies a failed stream handshake, not successful envelope
delivery. A successful lookup or an existing tunnel alone does not establish
that a restarted contact is reachable. Diagnose these together with the exact
failed assertion; do not treat a counter as proof of a root cause.

