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

1. **A relay people can use.** `DEFAULT_RELAY` is still empty. Relays now travel
   in the contact card (v2) and routing is per contact, so any relay works — but
   a fresh install has none to start from. The testnet relay runs on GitHub
   Actions, which is for testing; a relay real users depend on belongs on a
   machine someone operates. This is the owner's call.
2. **Discovery for in-process relays** (stage 3 of the relay plan). The server
   exists and is proven, but nothing starts it in the app: a relay nobody can
   find serves nobody.
3. **First release on i2pd.** v0.3.4 and earlier ship go-i2p and deliver
   nothing; the owner chose to leave them up. Tag only after the release
   workflow's Windows and macOS legs have been green once.
4. **macOS** builds were added today and have not completed a run yet: the
   router script (`.github/scripts/build-i2pd-macos.sh`), the app compile in
   build.yml, and the dmg legs in release.yml. Unsigned; floor is macOS 15.
5. **Android on a real device.** The router is built and asserted to be in the
   APK; nothing has run on hardware.

KNOWN DEAD CODE, DELIBERATELY LEFT ALONE

- The whole inbound/P2P surface of `libcore/src/net.rs` — `connect`, `accept`,
  `Frame` — is unused. It is also exactly what a relay-less design would build
  on, which is why it has not been deleted.
- `libcore/src/relay_server.rs` is not called by the app, deliberately, until
  discovery exists. The e2e job is its only caller.
- `libcore/src/session.rs` and `core/src/core.rs` are ~1500-line copy-paste
  siblings. Bots cannot create groups or send typing indicators, and every
  messaging fix has to be written twice.

USEFUL TO KNOW

- `tools/sam-eepsite.py` fetches a real eepsite through whatever router is on
  SAM 7656. It is the fastest way to tell "the router works" from "our code is
  wrong", and it is what ended a day of guessing last time.
- The e2e job's real signal is the `[e2e] SUCCESS` line and the echo count in
  `[e2e-timing]`. That is still true even now that the job fails honestly.
- Local shell may carry `CC`/`CXX` pointing at an Android NDK from a previous
  session; `cargo check` then tries to build OpenSSL for Android and fails
  confusingly. `unset CC CXX`.

CONVENTIONS: commits are authored `gluckdev <dep_it@spbsot.kz>` with no AI
attribution trailers. Do not commit router binaries or DEBUG logs — .gitignore
covers them, including `core/resources/i2pd`, which it did not before.
