# third_party

Router sources gipny builds and bundles, pinned as submodules so a build is
reproducible from this repository alone rather than from whatever happened to be
on a developer's disk.

| Submodule | Pinned at | Used for |
|---|---|---|
| `i2pd` | tag `2.60.0` | the router itself — desktop builds link it statically |
| `i2pd-android` | upstream default branch | its `binary/jni` target, which produces a standalone i2pd executable for Android; i2pd itself carries no Android build |

Both are upstream, unmodified. Anything we need to change on top lives in this
repository — as a patch under `docs/patches/`, applied by CI — so it is visible
in review instead of hiding in a fork nobody can see.

Clone with submodules:

```
git clone --recurse-submodules …
# or, in an existing checkout:
git submodule update --init --recursive
```

`.github/workflows/i2pd-build.yml` builds from these.

## Why these are here and go-i2p is not

go-i2p was the original transport and never finished building client tunnels,
so it delivered nothing — see `docs/i2p-transport-evaluation.md`. With i2pd
underneath, the same relay, bots and harness deliver messages over live i2p.

An experimental go-i2p patch (first hop taken from already-connected peers,
i2pd-style) is kept as `docs/patches/go-i2p-first-hop-selection.patch` because
the measurements around it are worth keeping. It is a record, not a dependency:
it improved tunnel building without making the network reachable, and it is not
built here.

## Android note

`i2pd-android` pins Boost-for-Android to a commit whose NDK whitelist is older
than the NDKs the CI runners ship, so the Android build overrides that pin at
build time. Details are in the workflow next to where it happens.
