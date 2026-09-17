# Getting rid of the relay

An open design question, written down because the relay is currently the single
thing standing between a working build and a working product: `DEFAULT_RELAY` is
empty, no production relay is deployed, and with no relay address the client can
receive nothing and send nothing.

This is a note to decide from, not a plan that has been agreed.

## What the relay actually buys

Three things, and they are worth separating because only one of them is hard to
replace.

1. **Offline delivery.** The relay holds encrypted blobs until the recipient
   comes online. Without it, both parties must be online simultaneously.
2. **Reachability without publishing.** The client runs with `publish: false`
   (`libcore/src/net.rs`), so it has no LeaseSet and nothing in the public netdb.
   Nobody can open a connection *to* it. All delivery is outbound, to the relay.
3. **Sealed sender.** The relay sees `{recipient, blob}` and never the sender
   (`from = [0u8; 32]` on the wire). It routes by public key, not by address.

Point 3 is not actually a reason to keep a relay — a direct connection gives the
peer your destination, but the peer already knows who you are. It is a reason the
*current* design is safe, not a requirement of any future one.

## What already exists

More than one might expect. `libcore/src/net.rs` carries a complete
point-to-point surface that nothing calls today:

- `Frame` (including a `Hello { identity, onion }` handshake left from the Tor
  era), `Connection::send/recv/close`
- `I2pNode::connect`, `connect_retry`, `accept`
- `spawn_inbound`, gated behind `GIPNY_I2P_ACCEPT=1`

`core/src/core.rs` touches `self.node` in four places, none of which dial or
accept. So the transport primitives for direct delivery are written and unused;
what is missing is everything around them.

## What breaks without a relay

**Reachability.** Direct delivery requires a *published, stable* destination:
persistent i2p keys on disk and a LeaseSet in the public netdb. That reverses
two deliberate decisions — the address is ephemeral per session and never
written to disk, and `publish: false` keeps us out of the netdb entirely. The
cost is real: a published LeaseSet ties a long-lived destination to a
continuously observable presence pattern. It does not expose an IP (that is what
the tunnels are for, and it is the question #49 turned on), but "this
destination was reachable between 09:00 and 18:00 on weekdays" is metadata the
current design simply does not emit.

**Offline delivery.** This is the hard one. Peer-to-peer means both online, which
for a phone means almost never. Any fix is some form of store-and-forward, and a
store-and-forward node that holds your ciphertext is a relay by another name —
the question is only who runs it and how many there are.

**Address freshness.** Contact cards embed the destination
(`gipny:v1:<dest>:<sign_pk>:<dh_pk>`), and the destination is regenerated every
launch. A card shared today is stale tomorrow and nothing notices. Direct
delivery makes that a hard failure rather than a cosmetic one.

## The options, and what each costs

**A. Direct P2P, relay only as offline fallback.** Persistent published
destination; dial the peer directly when reachable; fall back to the relay when
not. Keeps offline delivery, removes the relay from the common path, and the
relay stops being able to observe the timing of most messages. Costs: LeaseSet
publication, a reachability probe, two delivery paths to keep correct, and the
relay still has to exist for the fallback.

**B. Pure P2P, no offline delivery.** Both online or nothing. Honest, simple,
and a different product — closer to a secure chat than to a messenger. Would
have to be stated plainly in the UI, because "message sent" would stop meaning
"message will arrive".

**C. Many small relays instead of one.** Does not remove the relay; removes the
*single* one. Users or communities run their own; a contact card carries its
owner's relay. Much cheaper to build than A or B — the relay already exists,
`core/relay/` is a small crate with systemd units — and it addresses the actual
operational risk today, which is that one unowned relay is a single point of
failure for delivery. Costs: discovery, and the fact that your relay learns your
contact graph's timing even if not its content.

**D. Dead drops in the netdb.** Encrypted blobs at deterministic addresses
derived from the pair's shared secret. Removes the trusted node entirely. Also
the largest amount of new cryptographic protocol, in a codebase whose crypto core
(`libcore/src/crypto.rs`) currently has no tests at all.

## Recommendation

**C now, A next, and not B or D.**

C is days of work on code that already exists and fixes the thing that actually
blocks the product: there is no deployed relay and no plan for who operates one
forever. Making the relay address part of the contact card, rather than a global
constant, turns "we must run infrastructure" into "anyone can".

A is the real answer to the question as asked, and it becomes much easier once C
exists, because the fallback path is already there and already tested. Its
prerequisite is a decision about persistent destinations and LeaseSet
publication, which is an anonymity trade-off and not only an engineering one — it
deserves its own write-up before anyone implements it.

B is a different product. D is a research project, and this codebase should
earn some test coverage over its existing crypto before it grows more.

## Status (2026-09-16)

C is built and proven. A v2 contact card carries its owner's relay, each
contact's messages are deposited on that contact's relay, and the e2e job runs
with a separate relay per bot, so delivery only succeeds if routing follows the
card (e2e-i2pd run 35075367552, 5/5). The crypto core has tests now
(`libcore/tests/crypto.rs`).

The relay also runs inside a client: `libcore/src/relay_server.rs` serves the
same protocol from memory only, on a destination generated per start and never
stored. It carried 5/5 over live i2p with each bot served by one
(run 35076520090). The app does not start it yet. That waits on discovery: a
relay whose address changes every launch is only useful once contacts can learn
the current one.

Still open: no relay is baked in (`DEFAULT_RELAY` is empty), so a fresh install
has nowhere to start until someone operates one.

## Status (2026-09-17)

The app starts the in-client relay now, at every launch, and a fresh install
needs no relay from anyone. That closes the point above without baking an
address in.

How it is wired (`core/src/core.rs`):

- Two modes, `relay_mode` in the settings: **built-in** (the default for a
  profile that names no relay) and **external** (a profile that already names
  one keeps it; upgrading moves nobody).
- Built-in: `Core::start` spawns `run_hosted_relay`, which starts an
  `EphemeralRelay` with `MemStoreLimits::personal(own sign_pk)` — it holds mail
  and a prekey bundle for its owner and answers `ERR_NOT_SERVED` to anyone
  else. The address lives in memory only. It is never written to the settings;
  that is what 0.4.0's button did, and it left clients listening on a relay
  that no longer existed.
- Per launch, by the owner's decision: a destination that persisted would be a
  stable LeaseSet whose appearances trace the owner's online hours. The cost is
  accepted and documented — delivery happens while both apps run, and there is
  no mailbox for later.
- Discovery of the new address: once the relay is up, every contact we share a
  session with gets an empty payload (the keepalive shape) carrying
  `relay_address`; the send loop keeps trying until each has been told.
  Mail for a contact whose relay does not answer stays queued without burning
  retry attempts, and goes out when their announcement arrives.
- What cannot be recovered automatically: both sides restart without ever
  being online together. Each then holds the other's dead address. After ten
  minutes with mail queued the chat says so, and the fix is a fresh card —
  re-adding a contact by card keeps history and keys and replaces the relay.
- `gipny-agent` hosts a personal relay for itself too, exactly the same way
  and at the same point in startup (before it prints its own card — a card
  with no reachable relay is useless to hand to a master). `--relay` remains
  an external override, for an agent operator who wants an offline mailbox
  instead; pointed at somebody *else's* personal relay, it exits with an
  explanation (`ERR_NOT_SERVED`). Since the agent's relay is ephemeral like
  the app's, it re-sends its GRANT message to the master on every restart —
  already unconditional — which carries the new address for free through the
  same per-message `relay_address` field contacts use; no separate
  announcement was needed. The one gap this doesn't close: if the agent and
  its master both restart without ever being online together since, the
  master needs a fresh copy of the agent's `card.txt` (SSH access), the same
  as re-adding a contact by card, just with higher friction for a headless box.

Proof: the e2e job "relays inside the bots" starts each bot with no relay,
then a *personal* relay for that bot's key, then tells the bot the address —
the app's order — and delivers over live i2p. What it does not exercise is
`Core` itself (the harness drives `SessionManager`), so the wiring above is
covered by the app crate's unit tests and a manual check on a CI build.

## Decision (2026-09-17): a network of relays after all

The recommendation above said "not D". The owner reversed that, for a reason
the earlier options could not answer: with built-in relays, **closing the app
loses mail**. The relay dies with the process, its address changes on every
launch, and two contacts who were never online at the same time lose each
other for good. The owner's requirements, asked one by one:

- Offline mail is held by **the whole gipny network**: every relay stores
  sealed items for others, replicated.
- Addresses **stay ephemeral**; a contact's current address is found through
  the network, not by asking for a fresh card. There must never be a "your
  contact is gone, re-add their card" moment: mail sent to a dead address
  reaches the new one, and the card update back is acknowledged too.
- Adding a card sends a **request** the other side accepts.
- Entry into the network: contacts' last relays, a saved table of nodes, and a
  **seed node run on a GitHub Actions worker** that carries its state, encrypted,
  from one run to the next.

It is not D as written above. D stored blobs in i2p's own netdb; this stores
them on gipny relays, which already exist in every app and agent, and reuses
their connections. It is five phases:

1. Delivery holes with no new protocol (#70).
2. Contact requests (#71).
3. `dht/` (`gipny-dht`): protocol, sealing and an in-memory network, with no
   integration yet.
4. Integration into the app and the agent.
5. `core/relay` as the seed node, its encrypted state on GitHub, and a live-i2p e2e.

### What a storing node can learn

Nothing it could use to tell who talks to whom.

- **Keys.**
  - Mail between two contacts is stored under `HMAC(pair, "mail" ‖ recipient ‖ day)`. `pair` is derived from X25519 of the two identities' static keys, so only those two can compute the key.
  - A first letter goes under `HMAC(card, "intro" ‖ day)`. Only someone holding the recipient's card can compute that.
  - Keys roll over daily, so no long-lived mailbox exists to watch.
- **Values.**
  - Every value is sealed again on top of the session layer (XChaCha20-Poly1305). Otherwise the ratchet header, stored in the clear, would link one letter to the next.
  - Values are padded to a power-of-two size.
  - A first letter names its sender, so it is sealed to the recipient's public key with an ephemeral key instead.
- **Records.**
  - Address and prekey-bundle records are signed by their owner *inside* the ciphertext. Readers take the newest valid one.
  - An address record is readable only by the pair it was made for, so strangers holding a card cannot track when someone is online.
- **Node identity.** A node's id is the hash of its relay destination. Nothing ties it to the person running the node.
- **Storing** needs no login. Each store costs proof of work bound to the connection's challenge, the key and the value, and nodes enforce quotas.
- **Deleting** needs a token found only inside the sealed value, so only the recipient can delete.

### What it does not promise

Mail arrives only if some storing node outlives the recipient's absence.
Those nodes are contacts' apps, agents, and the seed; the seed is briefly down
at every six-hour handover. Phones never store for others. Looking something
up across i2p takes tens of seconds. That is acceptable for the offline path,
and direct delivery stays the main path.

## Prerequisites either way

- `DEFAULT_RELAY` must stop being a compile-time constant (`libcore/src/relay.rs`).
- Contact cards need a relay field and a version bump (`ui/src/api.ts`).
- The stale-address problem has to be solved before any direct path: today the
  destination in a card is ephemeral, and validation accepts anything ending in
  `.i2p` (`ui/src/contact.ts`).
