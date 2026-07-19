Verdict: **ruled out** for router-generated LeaseSets in go-i2p v0.1.59999 — this code path does not publish leases from zero-hop tunnels.

## Question

Does go-i2p publish a LeaseSet whose leases point at zero-hop tunnels?

## Evidence from source

### (a) Tunnel readiness logic

- `monitorTunnelsAndRequestLeaseSet` is started for each created I2CP session (`lib/i2cp/server_connection.go:649-653`, "`go s.monitorTunnelsAndRequestLeaseSet(session, conn)`").
- Readiness checks only that the session's own inbound/outbound pools have at least one active tunnel (`lib/i2cp/server_tunnels.go:270-284`, "`inboundPool := session.InboundPool()` ... `if len(...) == 0 ... return false`"), and active means only `State == TunnelReady` (`lib/tunnel/pool.go:414-417`, "`if tunnel.State == TunnelReady`"), with no hop-count check there.
- Those session pools are explicitly client pools (`lib/i2cp/server_tunnels.go:111-122`, "`IsClientPool: true`"), and build requests inherit that flag as `IsClientTunnel` (`lib/tunnel/pool.go:627-630`, "`IsClientTunnel: p.config.IsClientPool`"), where `IsClientTunnel` is documented as client-vs-exploratory (`lib/tunnel/builder.go:58`, "`True for I2CP session-scoped client pools (vs exploratory router pools)`").

Conclusion for (a): readiness can accept zero-hop **client** tunnels (state-only check), but exploratory tunnels are tracked in separate pools and do not satisfy this session check.

### (b) What goes into a published LeaseSet

- LeaseSet generation uses active inbound tunnels (`lib/i2cp/session.go:773-795`, "`CreateLeaseSet ... using active inbound tunnels`" and "`leases, err := s.buildLeasesFromTunnels(tunnels)`").
- Lease gateway is taken from the first hop (`lib/i2cp/session.go:900-905`, "`gatewayBytes := tun.Hops[0]`"), but tunnels with no hops are skipped (`lib/i2cp/session.go:861-864`, "`if len(tun.Hops) == 0 { ... continue }`"), and publication fails if none remain (`lib/i2cp/session.go:891-893`, "`no valid leases to publish`").
- Zero-hop inbound tunnels are represented with no hops (`lib/i2np/tunnel_manager_build.go:188-191`, "`Hops: nil`", "`State: tunnel.TunnelReady`"), so they are exactly the tunnels skipped above.
- Publishing itself does not rewrite lease gateways; it sends the serialized LeaseSet bytes (`lib/i2cp/session_maintenance.go:295-297`, "`publisher.PublishLeaseSet(destHash, leaseSetBytes)`"; `lib/router/leaseset_publisher.go:45-55`, "`PublishLeaseSet ... StoreLeaseSet`"), and periodic netdb publication republishes stored entries (`lib/netdb/publisher.go:243-253`, "`GetAllLeaseSets()` ... `publishLeaseSetEntry(lsEntry)`").

Conclusion for (b): in this path, zero-hop tunnels are filtered out before LeaseSet bytes are built, so their gateway cannot be published.

### (c) Can (a) and (b) combine into the suspected exposure?

For router-generated LeaseSets, no. Even if readiness triggers, zero-hop tunnels (`Hops` empty) are excluded from lease construction and cannot become lease gateways in published data (`lib/i2cp/session.go:861-864,891-893`; `lib/i2np/tunnel_manager_build.go:188-191`).

## Exact conditions

- A router-generated LeaseSet is publishable only when session inbound active tunnels yield at least one lease with `len(tun.Hops) > 0` (`lib/i2cp/session.go:861-864,891-893`).
- If active inbound tunnels are only zero-hop (empty `Hops`), LeaseSet creation fails with "no valid leases to publish" and that generated LeaseSet is not published (`lib/i2cp/session.go:891-893`; `lib/i2cp/session_maintenance.go:213-216`).

## What I could not determine

1. Whether the specific `Publishing all LeaseSets` log line observed in issue #46 was publishing an I2CP session LeaseSet or some other already-stored entry; that log only indicates the publisher loop is iterating DB entries (`lib/netdb/publisher.go:239-244`, "`Publishing all LeaseSets` ... `GetAllLeaseSets()`").
2. Whether a separate client could submit a crafted `CreateLeaseSet2` whose leases use self-gateway values: this handler accepts client-provided serialized LeaseSet2 and publishes it (`lib/i2cp/server_dispatch.go:389-420`, "`client provides the complete serialized LeaseSet2`" ... `publishLeaseSet2WithLogging`), while `ValidateLeaseSet2Data` here checks structure/destination/expiry (\"`destination must match`\", \"`must not already be expired`\") but I found no lease-gateway policy check in this path (`lib/i2cp/session_maintenance.go:336-366`).
