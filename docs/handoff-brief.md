Repo: gluckdev/gipny-i2p — a Rust messenger (Tauri desktop + Android) that runs
over I2P. Rust speaks SAMv3 (`yosemite` crate) to a bundled router.

Context handoff for whoever picks this up next — a person or another agent.
Replaces the 2026-07-19 brief, which described a go-i2p router that no longer
exists in this tree.

STATE (2026-09-16, evening).

The repository is live again: unarchived, Actions unlocked, every workflow has
run. What is proven on GitHub's runners, not just written:

- **Delivery over live i2p** with a relay per bot, so a message only arrives if
  the sender deposits on the *recipient's* relay: e2e-i2pd run 35075367552, 5/5.
- **The in-process relay** (`libcore/src/relay_server.rs`): the same test with
  no standalone relay, each bot served by an `EphemeralRelay` inside the harness
  process — run 35076520090, 5/5.
- **The router pin moves on delivery.** The bump job pinned i2pd 99eab7ac after
  run 35075367552; that revision builds on Linux (glibc and musl) and all three
  Android ABIs (i2pd-build run 35076553013).
- **v0.4.0 is tagged and published** (16:55 MSK): desktop for Linux
  x86_64/arm64, Windows, macOS arm64/x86_64, Android arm64/armv7, relay and
  agent tarballs. Two of its parts shipped broken; 0.4.1 fixes them (next
  section).

AGENT MODE AND 0.4.1 (2026-09-16, later)

The afternoon's agent work was done by another tool from the plan in
docs/plans/2026-09-16-agent-mode.md; docs/plans/2026-09-16-inspection.md is
the audit of it and the source of what follows.

- Agent mode in the app and the headless `gipny-agent` exist and are proven
  over live i2p: e2e-i2pd run 35182927011, job "e2e (agent binary)". The
  harness is the master, the real binary is a separate process started with
  the master's v2 card; GRANT creates the contact on the master by itself,
  commands run one at a time (start/end pairs never interleave), an uploaded
  script is on disk when its command runs, OFF is answered with REVOKE and
  exit 0. Command RTT over the relay was 4–5 s on the first run.
- **The desktop app could not start its router at all in 0.4.0.** 5e9e337
  passed `--meshnets.yggdrasil=false`; i2pd declares that option as a boost
  bool_switch and refuses a value, exiting before SAM opens. Nothing in CI
  spawned the router through libcore — the e2e jobs attach to one their own
  script starts — until the agent tarball smoke test in release.yml did. That
  smoke step is now the only CI check of `RouterHandle::spawn`; keep it.
- Shipped broken in 0.4.0, fixed on main for 0.4.1: the agent tarball had no
  i2pd (the agent looks for it next to its own executable) — now bundled with
  a systemd unit; the "start built-in relay" button wrote a per-launch
  throwaway destination into the client's own relay setting — withdrawn.
- Relay discovery (eb339bd) stays: every non-typing message carries the
  sender's relay inside the ratchet envelope and the receiver updates the
  contact. README's delivery section says so.
- Still open from the agent plan: installers (install-agent.sh/.ps1, a
  launchd plist), macOS and Windows agent tarballs.
- Behaviour to keep in mind: a COMMAND from any contact is stored pending;
  the moment that contact is made master, its backlog runs.
- **v0.4.1 is tagged and published** (2026-09-17, on c6b7c57): the router
  fix above, the agent tarball with i2pd, the phone layout that cannot grow
  wider than the screen (`ui/dev/preview.html` is the rig it was built on).
- Merged after 0.4.1: PR #66, the attachment metadata sanitizer redone per the
  inspection's section B1 (JPEG/PNG/WebP/PDF by content, fail closed on what
  cannot be cleaned, never on console uploads), with its tests; PR #65
  (issue #31), the crypto/serde stack on current majors with compatibility
  fixtures generated on the old stack. The voice messages (B2) are not
  started: the first step is a microphone spike per webview, and the parked
  first attempt is in 86c757a.

THE BUILT-IN RELAY (2026-09-17, for 0.4.2)

The owner opened 0.4.1, was told there is no relay, and said what the design
had always been: a relay under the hood of every app. It is there now, and on
by default. `docs/relay-independence.md`, "Status (2026-09-17)", is the
description; the decisions behind it:

- **A new address at every launch, never stored** — the owner's choice between
  that and a persisted destination. The cost is delivery only while both apps
  run, and two people who both restart without overlapping have to exchange
  cards again. Do not "fix" this by persisting the key without asking.
- The relay is personal (`MemStoreLimits::personal`): an app holds nobody
  else's mail.
- A profile that already names a relay stays external. Profiles that pressed
  the 0.4.0 button hold a dead address and are not rescued automatically —
  there is no way to tell it from a live one; the release notes say how.
- Not proven by CI: `Core`'s own wiring (auto-start, announcements, the
  reachability notice). The e2e job proves the personal relay and the start
  order over live i2p through `SessionManager`; the rest is unit tests and
  the owner's check on a CI build.

RELAY IN THE AGENT TOO (2026-09-17, same day)

The owner's next ask: the relay had to be everywhere, agents included.
`gipny-agent` now hosts a personal relay for itself the same way — see
`docs/relay-independence.md`. It reused the app's design outright rather than
inventing a second one: `--relay` changed meaning from "the relay to collect
from, defaulting to the master's" to "an external override instead of the
agent's own built-in one." The agent already re-sends its GRANT message to
the master unconditionally on every restart, and every outgoing payload
already carries the sender's current relay address
(`session.rs::build_payload_from_db`) — so a fresh relay address at every
agent restart needed no new announcement mechanism, just setting the relay
before that first send. Do not read this as an oversight to "improve" later;
it was free because the pieces were already there.

AUTO-UPDATE FOR THE APP AND THE AGENT (2026-09-17, same day again)

The owner's ask: check for a new release at every launch, download and
install it in the background, and let the *next* launch be the one that runs
it — never disrupt the session in progress. Two decisions, made with the
owner before writing anything:

- **Transport is GitHub Releases through the local i2pd HTTP proxy with an
  outproxy** (`exit.stormycloud.i2p`), not raw clearnet (would leak "this IP
  runs gipny" to GitHub on every check) and not the old signed-manifest
  protocol in `libcore/src/update.rs` (nothing was ever built server-side for
  it — `DEFAULT_UPDATE_ONION` was empty and stayed that way). That old
  protocol, its wire format, and `UPDATE_VERIFY_KEY` are gone; TLS to GitHub
  through the outproxy already gives end-to-end authenticity, and
  `SHA256SUMS.txt` (already produced by the release pipeline) is a cheap
  defense-in-depth check on top of it, not the source of trust.
- `libcore::router` now enables i2pd's `httpproxy` (was hard off) with that
  outproxy configured, on its own free local port
  (`RouterHandle::http_proxy_port`, threaded through `I2pNode`). `None` on
  Android or when attached to a router we don't own (`GIPNY_SAM_PORT`) — the
  updater treats that as "unavailable this run", not an error.
- Real release asset names (`gh release view`, not a guess) drive matching:
  `gipny-i2p_<ver>_amd64.AppImage`, `_x64-setup.exe` for the app,
  `gipny-agent_<ver>_linux-<arch>.tar.gz` for the agent. `.deb`/macOS/Android
  stay "downloaded, cannot self-install" — same honesty the old
  `InstallStrategy` had, just against real filenames now.
- "Next launch, not now" per platform: Linux (AppImage, and the agent's own
  binary) renames the verified download over the running file's path
  immediately — safe because this process keeps executing what it already
  loaded, and only the *next* exec of that path sees the new bytes. Windows
  cannot overwrite a running exe, so it stages the installer and
  `apply_staged_windows_installer` runs it as `installer.exe /S /UPDATE /R
  /NCRC` and exits, at the very start of the *next* launch (`boot()` in
  `core/src/lib.rs`, before the vault is unlocked). Tauri's NSIS template
  honours `/R` in silent mode and starts the app again itself; `/UPDATE` skips
  re-creating shortcuts. This is the one piece of the change that is **not
  verified beyond a compile check** — it needs a real Windows machine.
- Auto-install is on by default (`Core::auto_update_enabled`, Settings →
  "обновляться автоматически", same default-on convention as
  `attachment_privacy`). Off, the old manual modal (`ui/src/app.ts`,
  `openUpdateModal`) still shows the version/notes/size and installs on a
  click — unchanged flow, `target_key` just no longer exists on `UpdateInfo`.
  On, `check_and_emit_update` downloads and installs in the same call,
  silently, and emits `UpdateStaged { version }` — a toast, not a modal.
- `gipny-agent` gets the same checker (`Updater::new(node, Component::Agent)`,
  new — the agent had no updater at all before), on the same cadence
  (`UPDATE_CHECK_INITIAL_SECS`/`UPDATE_CHECK_INTERVAL_SECS`, now shared
  constants in `libcore::update` instead of duplicated in `core.rs`), logged
  with `eprintln!` since it has no UI.
- Per-version "already handled" dedup (`dismissed_update_version`, both the
  app's DB setting and the agent's) matters more than it did: without it, a
  process that auto-installed but was not restarted would redownload and
  reinstall the same version every `UPDATE_CHECK_INTERVAL_SECS` forever,
  because `env!("CARGO_PKG_VERSION")` cannot change until the process
  actually restarts.

DELIVERY HOLES, BEFORE THE RELAY NETWORK (2026-09-17, phase 1 of 5)

The owner wants closing a client never to lose mail, and chose a network of
relays that store sealed mail for each other, with addresses that stay
ephemeral (plan and decisions: docs/relay-independence.md once phase 3 lands;
until then the phase list lives in the PR descriptions). Phase 1 fixes what
lost mail with no new protocol at all:

- An undelivered message used to be retried eight times over about fifteen
  minutes and then abandoned. `list_unacked_outgoing` and
  `pending_outbound_for_recipient` now retry until the message is
  `RETRY_TTL_MS` (7 days) old, with the same capped backoff.
- The X3DH init payload now carries the sender's relay, in core and in
  `session.rs`. A contact created from an init had no relay to answer to.
- `session.rs` caught up with core: every payload is stamped with our relay
  (was only messages built from the DB), the connection to our own relay
  drops when that relay changes (was: after the 75 s dead threshold), and
  callbacks, edits, pins and delivery acks go to the contact's relay instead
  of our own, where nobody would ever collect them.
- Relay login: plain `Auth` signed the bare challenge, so a relay a client
  connected to could pass on another relay's challenge and log in there as
  that client. `ClientToRelay::AuthV2` signs `"gipny-relay-auth-v2" ||
  SHA-256(relay destination) || challenge`; the hash is computed from either
  spelling of the address (base64 or `.b32.i2p`). Relays grant collecting,
  acking and publishing only to `AuthV2`; plain `Auth` may still deposit and
  fetch bundles, so 0.4.2 clients can keep writing to new relays. Clients try
  `AuthV2` and fall back to `Auth` only when the relay hangs up on it, which
  is what a relay from before it does; a relay faking that gains nothing but
  deposit rights. **Remove plain `Auth` once 0.4.2 is no longer in use.**

WHAT CHANGED SINCE THE LAST BRIEF

go-i2p is gone from the repository. It never finished building client tunnels
and delivered nothing (docs/i2p-transport-evaluation.md); i2pd carried 5/5
messages over live i2p on the same harness. The switch had been made for desktop
only, in 769d6ae, and everything else was left behind. Now:

- `i2p-router/` deleted. No Go anywhere in the build.
- Android runs i2pd in-process via JNI. `android-router/jni` builds the same
  i2pd sources plus two entry points (`Java_app_gipny_GipnyService_nativeStart/
  StopSam`) into `libi2pd.so`. The standalone binary the old workflow produced
  could never have worked: it exports no JNI symbols.
- Relay systemd units, the testnet relay workflow and `run-e2e.sh` all run i2pd.
  `run-e2e.sh` lost its mock-SAM mode entirely — it ran in seconds and proved
  nothing, which is how a dead transport stayed hidden.
- Submodules are pinned again instead of tracking branch heads.

THINGS THAT WERE BROKEN AND ARE NOW FIXED — worth knowing because each one hid
in plain sight:

- `core/tauri.conf.json` carried two resource globs while only one could ever
  match. tauri-build errors on a glob with no match, so **no fresh clone could
  build at all**, and both release.yml and build.yml were failing on it.
- A fresh install never left the boot screen. `RelayConnected` was the only
  transition into the main view, `DEFAULT_RELAY` is empty so the core never
  dials, and the relay field that would fix it lives in Settings — inside the
  view it could not reach.
- Duress decoy returned "bad key" instead of a decoy profile, because the decoy
  master key is random and cannot open `data.db`. It now opens `decoy.db`. The
  existing test passed the whole time: it stopped at the vault.
- The update loop dialed an empty destination forever, and each failure counted
  against the *shared* relay health counter, forcing SAM session rebuilds.
- The e2e job was `continue-on-error`, so its check was green at 0/5 delivered;
  and a `set -e` interaction meant a failed harness never even recorded
  `delivered=false`.
- The Windows router was built as the tray/GUI daemon (`USE_WIN32_APP` defaults
  to yes in Makefile.mingw), and the release binary carried no version stamp.

#49 (LeaseSet exposure) is answered and closed out in
docs/go-i2p-leaseset-analysis.md: ruled out by source reading, with the limits
of that evidence stated. It is moot for anything shipped from here, since
go-i2p is gone; it is not moot for v0.3.4 and earlier.

WHAT IS STILL OPEN, IN ORDER

1. **0.4.2 on real devices.** The built-in relay has not run inside the app
   anywhere but the owner's machine. Two installs, fresh profiles: the banner
   goes away in a minute or two, "my card" fills in, the second device can
   write to the first, a restart of one is followed by the other.
2. **An offline mailbox for people who want one.** Built-in delivers only
   while both apps run. External mode and the `gipny-relay` tarball cover it
   for those who operate a relay; nothing is baked in (`DEFAULT_RELAY` is
   empty) and whether to operate a public one is the owner's call.
3. **Voice messages** (inspection, B2), agent installers and macOS/Windows
   agent tarballs (agent plan §5–§6).
4. **macOS** legs ran green in the v0.4.0 release (dmg for both arches).
   Unsigned; floor is macOS 15. Nobody has launched the dmg on a Mac yet.
5. **Android on a real device.** The router is built and asserted to be in the
   APK; nothing has run on hardware.

KNOWN DEAD CODE, DELIBERATELY LEFT ALONE

- The whole inbound/P2P surface of `libcore/src/net.rs` — `connect`, `accept`,
  `Frame` — is unused. It is also exactly what a relay-less design would build
  on, which is why it has not been deleted.
- `libcore/src/session.rs` and `core/src/core.rs` are ~1500-line copy-paste
  siblings. Bots cannot create groups or send typing indicators, and every
  messaging fix has to be written twice.

USEFUL TO KNOW

- The UI can be looked at without a backend or an APK: `cd ui && npm run dev`,
  then http://127.0.0.1:5173/dev/preview.html. It boots the real app against a
  mocked Tauri IPC (`ui/dev/mock.ts`) at phone, tablet and desktop widths, with
  fixtures that are hostile on purpose (host names, b32 addresses and
  516-character destinations with no break opportunity), and a button that
  lists every element sticking out of its viewport. The phone layout was eight
  screens wide for want of exactly this.
- `tools/sam-eepsite.py` fetches a real eepsite through whatever router is on
  SAM 7656. It is the fastest way to tell "the router works" from "our code is
  wrong", and it is what ended a day of guessing last time.
- The e2e job's real signal is the `[e2e] SUCCESS` line and the echo count in
  `[e2e-timing]`. That is still true even now that the job fails honestly.
- Local shell may carry `CC`/`CXX` pointing at an Android NDK from a previous
  session; `cargo check` then tries to build OpenSSL for Android and fails
  confusingly. `unset CC CXX`.

CONVENTIONS: commits carry the repository owner's git identity and no AI
attribution trailers. Other tools commit here under that same identity; before
treating a change in the tree as your own, check its provenance
(docs/plans/2026-09-16-inspection.md, "Контекст"). Do not commit router binaries or DEBUG logs — .gitignore
covers them, including `core/resources/i2pd`, which it did not before.
