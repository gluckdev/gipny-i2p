Repo: gluckdev/gipny-i2p — a Rust messenger (Tauri desktop + Android) that runs
over I2P. Rust speaks SAMv3 (`yosemite` crate) to a bundled router.

Context handoff for whoever picks this up next — a person or another agent.
Replaces the 2026-07-19 brief, which described a go-i2p router that no longer
exists in this tree.

STATE (2026-09-16).

THE BLOCKER, AND IT IS NOT TECHNICAL: **the GitHub repository is archived.**
Nothing has run since 2026-07-20 — no CI, no scheduled jobs, no testnet relay —
and nothing can be pushed, released, or closed until the owner unarchives it
(Settings → General → Danger Zone). Every workflow below is written and
committed; none of it has executed.

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

1. **Unarchive the repository.** Everything below is blocked on it.
2. **Run the pipelines once.** None of this work has been executed: not the
   Android JNI build, not the release path, not the e2e. Expect the first
   Android run to need fixing — it has never compiled.
3. **Pin Boost-for-Android.** `BOOST_FOR_ANDROID_REV` still defaults to
   `master`. It is the one build input this repo does not record, and the
   Android router is not reproducible until a measured revision is pasted in.
4. **Deal with the published releases.** v0.3.4 and earlier ship go-i2p and
   deliver nothing. They are still the download link in the README's own words
   until they are pulled or marked.
5. **Deploy a relay, or decide not to need one.** `DEFAULT_RELAY` is empty and
   there is no production relay. See docs/relay-independence.md — the
   recommendation there is to make the relay address part of the contact card
   before building anything else.
6. **Test the crypto core.** `libcore/src/crypto.rs` — X3DH, Double Ratchet,
   header encryption — has no tests. It is the largest coverage gap in the repo
   by a wide margin.
7. **Android on a real device.** i2pd will be built for it and the APK will be
   asserted to contain it, but nothing has run on hardware.

KNOWN DEAD CODE, DELIBERATELY LEFT ALONE

- `libcore/src/proxy.rs` (sing-box lifecycle) is exported and has no callers.
- The whole inbound/P2P surface of `libcore/src/net.rs` — `connect`, `accept`,
  `Frame` — is unused. It is also exactly what a relay-less design would build
  on, which is why it has not been deleted.
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
