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

WHAT IS ALREADY BEING WORKED ON — do not duplicate:
- #47 / PR #48: GitHub Copilot is adding a workflow that runs the existing e2e
  job against i2pd instead of go-i2p, to prove whether our own stack delivers
  when the router underneath is competent. Workflow-only, touches no app code.
- #46 is the decision issue (keep patching go-i2p vs move to i2pd). Not a task.

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
