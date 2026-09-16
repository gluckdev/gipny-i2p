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

## Prerequisites either way

- `DEFAULT_RELAY` must stop being a compile-time constant (`libcore/src/relay.rs`).
- Contact cards need a relay field and a version bump (`ui/src/api.ts`).
- The stale-address problem has to be solved before any direct path: today the
  destination in a card is ephemeral, and validation accepts anything ending in
  `.i2p` (`ui/src/contact.ts`).
