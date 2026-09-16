# third_party

Router sources gipny builds and bundles, pinned as submodules so a build is
reproducible from this repository alone rather than from whatever happened to be
on a developer's disk.

| Submodule | Pinned at | Used for |
|---|---|---|
| `i2pd` | `2.60.0-190-g62bf51cd` (a commit, not the tag) | the router itself — desktop builds link it statically, and Android builds the same sources into a JNI library |
| `i2pd-android` | a commit on upstream `master` | its `binary/jni` dependency scripts (boost, OpenSSL) and its `DaemonAndroid` wrapper; i2pd itself carries no Android build |

The i2pd pin runs well past the `2.60.0` tag it started at; `git submodule
status` is the authority, and `git describe --tags` in CI stamps the binary with
it. The pin is advanced only by `e2e-i2pd.yml`'s bump job, and only after a run
that actually delivered messages over live i2p — compiling is not the bar.

Both are upstream, unmodified. Anything we need to change on top lives in this
repository — as a patch under `docs/patches/`, or as a step in the workflow — so
it is visible in review instead of hiding in a fork nobody can see.

**These point at upstream, not at forks of ours, and that is deliberate.** A
submodule pins which revision we build; it does not make the code ours. We
cannot commit to it, which is why the one change we need — moving
Boost-for-Android past an NDK whitelist older than any NDK the runners ship — is
a command in the Android build rather than a commit.

Forking (into `gluckdev/`) would turn such changes into reviewable commits and
cut the dependency on upstream not rewriting tags. It also means keeping two
repositories in sync with an actively developed codebase, and falling behind on
a cryptographic router is worse than the workaround. Revisit when there is a
second or third change upstream will not take; one line does not justify it.

Clone with submodules:

```
git clone --recurse-submodules …
# or, in an existing checkout:
git submodule update --init --recursive
```

`release.yml` builds the router it ships from these pins.
`i2pd-build.yml` covers the wider matrix (musl, armeabi-v7a, the second Android
ABI). `e2e-i2pd.yml` deliberately runs ahead of the `i2pd` pin — that is how the
pin earns its way forward.

The Android library is *not* built from `i2pd-android`'s own `app/jni`: that
exports `Java_org_purplei2p_*` symbols for their app. `android-router/jni` in
this repository compiles the same i2pd sources plus gipny's two JNI entry
points, reusing only `DaemonAndroid.cpp` from upstream.

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

Boost-for-Android is a nested submodule of `i2pd-android`, so this repository
cannot pin it with a gitlink; the override is the `BOOST_FOR_ANDROID_REV` env in
`i2pd-build.yml` and `release.yml`, and the two must match. It is pinned at
`7943955c4d11a5bd61381a8b200c28619323eb0f` — the revision that produced the
first `libi2pd.so` to build and export the JNI entry points on all three ABIs
(run 35061210535). Every Android build prints the revision it resolved.
