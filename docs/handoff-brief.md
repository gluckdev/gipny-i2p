Repo: gluckdev/gipny-i2p — a Rust messenger (Tauri desktop + Android) that runs
over I2P. Rust speaks SAMv3 (`yosemite` crate) to a bundled router binary; the
router is currently a thin Go wrapper (`i2p-router/`) around go-sam-bridge's
embedded go-i2p router.

Context handoff for whoever picks this up next — a person or another agent.
Replaces the earlier root-level brief about #42, which is now resolved.

STATE (2026-07-19). Everything above the router works; the router does not.
Branch `fix/i2p-router-i2cp-wiring`, not merged. Full write-up with all
measurements: docs/i2p-transport-evaluation.md. Read that before starting — it
will save you a day.

Four defects were stacked. Three were ours and are fixed:

1. The SAM bridge was built with no I2CP client at all, so no STREAM session had
   a transport (#42, 97788e6).
2. The router closes I2CP connections idle for 30 s, and go-i2cp neither keeps
   them alive nor notices the close, so the connection was always dead before
   the first session (go-i2p#54, e0a3826 — now obsolete, see 3).
3. One I2CP client only ever carries one working session: go-i2cp reads the
   SessionStatus of the first session on a connection and no other. That capped
   the bridge at a single SAM session, which is why the relay always worked and
   every bot always failed. Fixed by giving each SAM session its own I2CP client
   (18edc66). Verified: 7 sessions in CI.

The fourth is upstream and blocks everything: go-i2p never finishes building
client tunnels (go-i2p#55). Sessions are created, stream managers register, and
then every client tunnel build expires. No LeaseSet is published. Nothing is
reachable in either direction.

The decisive experiment, and the one that ended a day of guessing: forget our
e2e harness and just fetch a real eepsite through the router
(`tools/sam-eepsite.py`). go-i2p never got as far as STREAM CONNECT. i2pd
2.60.0, same machine, same uplink, same script, minutes apart: tunnels in 4
seconds, STREAM CONNECT OK in 3.5 s, and a live HTTP 302 from the official
project site inside I2P. That reproduces on a GitHub runner too, so it is not
about one network.

SETTLED SINCE: the stack is fine, the router was the whole problem.

PR #48 (`.github/workflows/e2e-i2pd.yml`) is e2e.yml with only the router
swapped. Run 29693889188 delivered **5 of 5 messages over live i2p in 49 s**,
median RTT 5.8 s. The same harness has never delivered one message over go-i2p.
Relay, bot-sdk, session layer, shared-router arrangement (#45) — all correct.

So #46 is no longer "is go-i2p the problem" but "what does shipping i2pd cost".

IN FLIGHT AT THE END OF 2026-07-19:

- Branch `ci/i2pd-router-build`, workflow `.github/workflows/i2pd-build.yml`:
  builds i2pd ourselves for linux x86_64 (static), windows x86_64 (msys2), and
  Android arm64-v8a / x86_64 / armeabi-v7a. Nothing is wired into the app — it
  only answers whether we can produce the binaries.

  Run 29695096389: **linux and windows both built, all three Android legs
  failed** in Boost-for-Android with "Undefined or not supported Android NDK
  version: 27.3". So desktop is done and Android is one step from done.

  The NDK pin is still wrong, and the reason is worth knowing: the supported
  version list was read from Boost-for-Android's **master**, but i2pd-android
  pins that submodule to an older commit whose list is shorter — hence their
  documentation pinning NDK 21.4.7075529. Next step is to use 21.4 (installing
  it via sdkmanager, since the runners no longer ship it), or to advance the
  boost submodule to master and keep 27.3. 21.4 is the conservative choice and
  matches what upstream tests; 27.3 would keep router and APK on one toolchain,
  which is why it was tried first.
- PR #48 is ready for review, with a temporary push trigger already removed.
- #49 (LeaseSet exposure) is still open and still unanswered.

THINGS LEARNED THE HARD WAY IN THAT WORKFLOW, DO NOT REDISCOVER:

- Boost-for-Android matches the NDK version against a fixed list and refuses
  anything outside it. Read that list from the **submodule commit i2pd-android
  pins**, not from its master branch: master reaches 28.2, the pinned one does
  not even accept 27.3, and reading the wrong one cost a full CI cycle.
- i2pd-android's dependency scripts take arm64/x86_64/arm/x86, not ABI names,
  and an unrecognised argument falls through their `*)` case, builds nothing,
  and still exits 0.
- Their Application.mk pins APP_PLATFORM to android-16 (dropped by modern NDKs)
  and APP_ABI to "all"; both are overridden on the ndk-build command line.
- Desktop must build on ubuntu-22.04, the image release.yml uses. boost/openssl
  link statically but glibc cannot, so a newer image yields a router the bundles
  cannot run beside on older distros. Note this is a genuine regression against
  today's Go router, which is CGO_ENABLED=0 and depends on nothing; if it bites,
  the answer is a musl build.
- Android will not execute a binary from the app's data directory, so the router
  has to travel in jniLibs as lib*.so to land in nativeLibraryDir.
- The APK ships no 32-bit ABI today, so the armeabi-v7a leg is a measurement,
  not a commitment.

STILL OPEN, ROUGHLY IN ORDER:
1. #49 — LeaseSet exposure. Security, cheap to settle, blocks nothing else.
2. Finish reading the i2pd-build results; decide desktop bundling (glibc floor,
   which mingw DLLs the Windows installer needs).
3. Android integration: standalone binary in jniLibs versus libi2pd.so + JNI.
   The app already loads a router .so in-process via GipnyService.kt, so the JNI
   shape is closer to what exists; i2pd-android has a wrapper to borrow.
4. #46 — take the decision once 2 and 3 have numbers.

THE OPEN TASK — #49, and it is the one that actually matters right now:

Confirm or rule out that go-i2p publishes a LeaseSet exposing our IP.

With the experimental patch the tunnel-wait timeouts stopped and the log shows
`Publishing all LeaseSets`, while the only tunnels that ever completed were
zero-hop exploratory ones. A LeaseSet lists the gateways of inbound tunnels; for
a zero-hop tunnel that gateway is our own router. netdb is public. If that is
what is being published, current builds do not merely fail to deliver — they put
the user's IP in the public netdb next to their destination, which is the exact
inverse of what this app promises.

This is inferred from log lines. Nobody has looked at an actual published
LeaseSet. Any of three approaches settles it, and one needs no network at all —
see the issue. Reading whether `monitorTunnelsAndRequestLeaseSet` counts
exploratory zero-hop tunnels toward readiness may be enough to prove it from
source.

USEFUL TO KNOW:
- `DEBUG_I2P=debug` is what enables go-i2p's logging. The router's own `--debug`
  flag only reaches go-sam-bridge's embedding options and leaves the router
  silent. Expect ~300 MB of log for a few minutes.
- `NAMING LOOKUP` for a .b32.i2p address returns KEY_NOT_FOUND in 0.0 s — no
  netdb lookup is attempted. Use base64 destinations from the official hosts.txt
  (I2P source tree, installer/resources/hosts.txt) instead of names.
- The mock SAM server behind `-tags mocksam` bypasses i2p entirely. Useful for
  the local dev loop, useless as evidence — never draw conclusions from it.
- A green check on the e2e job means nothing: it is continue-on-error. The
  signal is the harness's `[e2e] SUCCESS` line and the echo count in the
  `[e2e-timing]` table. Two runs on main reported success while delivering zero
  messages; that is how #42 stayed hidden.
- go-i2p's own README says it "is probably very distinct on the network" and
  that you should use a more established router for now. That is a standing
  anonymity argument against it that no fix of ours can address.
- A patched go-i2p checkout sits at /home/artamonov/projects/go-i2p-fork (first
  hop taken from already-connected peers, i2pd-style). The same diff is in
  docs/patches/. It removed the tunnel-wait timeouts but did not make the
  network reachable, and it must not ship: it lacks i2pd's peer-count
  thresholds, so with few peers connected every tunnel shares the same gateways.

CONVENTIONS: commits are authored `gluckdev <dep_it@spbsot.kz>` with no AI
attribution trailers. Do not commit the experiment binaries or DEBUG_I2P logs —
.gitignore covers them.
