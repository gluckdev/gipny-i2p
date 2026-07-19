Ruled out for go-i2p v0.1.59999: in the built-in I2CP session path, go-i2p does **not** publish a LeaseSet whose leases point at zero-hop tunnels.

## Scope

This is a source-only analysis of go-i2p `v0.1.59999` (`/tmp/go-i2p`), per the task.

## Evidence

1. Session tunnel readiness does **not** require `hop_count > 0`; it only checks that inbound and outbound session pools each have at least one active tunnel (`lib/i2cp/server_tunnels.go:277-284`, `"if len(result.inboundTunnels) == 0 || len(result.outboundTunnels) == 0 { ... }"`).

2. The readiness monitor reads **session** pools (`session.InboundPool()` / `session.OutboundPool()`), not exploratory router pools (`lib/i2cp/server_tunnels.go:270-272`).

3. Those session pools are explicitly client pools (`IsClientPool: true`) in I2CP setup (`lib/i2cp/server_tunnels.go:111-122`, `"IsClientPool: true"`).

4. Exploratory/router pools are separate and default `IsClientPool` to false (`lib/tunnel/pool.go:102-104`, `lib/tunnel/pool.go:108-119`), so exploratory tunnel success does not by itself satisfy client-session publication readiness.

5. Zero-hop inbound tunnels are represented with no hops (`Hops: nil`) when registered active (`lib/i2np/tunnel_manager_build.go:188-191`, `"Hops:      nil"`).

6. Lease construction for session publication skips any tunnel with zero hops (`lib/i2cp/session.go:861-864`, `"if len(tun.Hops) == 0 { ... continue }"`) and errors if nothing valid remains (`lib/i2cp/session.go:891-893`, `"has no valid leases to publish"`).

7. Lease gateways are taken from the first hop of each retained tunnel (`lib/i2cp/session.go:901-905`, `"gatewayBytes := tun.Hops[0]"`), so zero-hop tunnels cannot contribute a gateway.

8. The RequestVariableLeaseSet payload path also strips zero-hop tunnels (`lib/i2cp/server_tunnels.go:389-400`, `"removes nil and zero-hop tunnels"`, `"if tun != nil && len(tun.Hops) > 0"`).

9. If readiness is met but all inbound tunnels are zero-hop, sending the LeaseSet request fails (no valid tunnels) and maintenance is not started (`lib/i2cp/server_tunnels.go:291-296`, `lib/i2cp/server_tunnels.go:310-318`, `lib/i2cp/server_tunnels.go:399-401`).

10. The log line `"Publishing all LeaseSets"` is emitted at the start of periodic publisher sweep even before checking whether any LeaseSets exist (`lib/netdb/publisher.go:239-246`), so that line alone is not proof that a LeaseSet with leases was actually published.

## Exact conditions implied by this code

- A zero-hop tunnel can satisfy only the **count-based** readiness gate if it appears in the session pool (`lib/i2cp/server_tunnels.go:277-284`).
- But zero-hop tunnels are excluded before lease entries are formed (`lib/i2cp/session.go:861-864`, `lib/i2cp/server_tunnels.go:395-397`).
- Therefore, the router does not publish lease entries sourced from zero-hop tunnels in this path; if no non-zero-hop inbound tunnels exist, lease creation/request fails instead (`lib/i2cp/session.go:891-893`, `lib/i2cp/server_tunnels.go:399-401`).

## What I could not determine

1. I did not fully trace external `common/lease_set2` validation internals to prove whether a custom external I2CP client could submit a hand-crafted LeaseSet2 containing arbitrary lease gateways via `CreateLeaseSet2`; this analysis is about go-i2p’s own tunnel-derived LeaseSet path (`lib/i2cp/server_dispatch.go:391-421`, `lib/i2cp/session.go:344-371`).
2. I did not establish, from source alone, why your observed run logged `Publishing all LeaseSets` while client tunnels timed out; I only established that this log line is emitted before the zero/empty check (`lib/netdb/publisher.go:239-246`).
