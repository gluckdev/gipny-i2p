#!/bin/bash
# Build the bundled i2pd router on a GitHub macOS runner. CI only — this project
# has no local builds; i2pd-build.yml and release.yml both call this so the
# router they test and the router they ship are built the same way.
#
# Leaves a stripped, ad-hoc signed third_party/i2pd/i2pd that links nothing
# outside the OS. Expects third_party/i2pd unshallowed (git describe needs tags).
set -euo pipefail
cd "$(dirname "$0")/../../third_party/i2pd"

# The router's minimum macOS is the runner's: Homebrew bottles are built for the
# image's own OS version, and their static archives carry that floor into the
# binary. core/tauri.conf.json's minimumSystemVersion must match.
export MACOSX_DEPLOYMENT_TARGET=15.0

brew install boost openssl@3
BOOSTROOT="$(brew --prefix boost)"
SSLROOT="$(brew --prefix openssl@3)"

# HOMEBREW=1 selects upstream's Makefile.homebrew. What we pass over it:
#  - BOOSTROOT/SSLROOT: it hardcodes openssl@3.5 under a guessed brew root.
#  - USE_STATIC=yes: Homebrew's dylib paths do not exist on a user's Mac.
#  - CXXFLAGS with NO_TORRENTS, and LDLIBS without boost_json: Makefile.homebrew
#    has no TORRENTS switch and links boost_json unconditionally. NO_TORRENTS is
#    exactly what TORRENTS=no defines on Linux, and without the torrent client
#    nothing needs json.
#  - USE_UPNP=no and USE_GIT_VERSION=yes, as on every other platform.
make HOMEBREW=1 USE_STATIC=yes USE_UPNP=no USE_GIT_VERSION=yes \
  BOOSTROOT="$BOOSTROOT" SSLROOT="$SSLROOT" \
  CXXFLAGS="-O2 -Wall -Wno-overloaded-virtual -DNO_TORRENTS" \
  LDLIBS="-lz $SSLROOT/lib/libcrypto.a $SSLROOT/lib/libssl.a $BOOSTROOT/lib/libboost_program_options.a -lpthread" \
  -j"$(sysctl -n hw.ncpu)"

echo "--- size before strip ---"; ls -lh i2pd
strip i2pd
# strip invalidates the signature the linker applied, and Apple Silicon will not
# execute an unsigned binary at all. Ad-hoc is all a build without an Apple
# developer identity can give it.
codesign --force --sign - i2pd
codesign --verify --verbose i2pd
echo "--- size shipped ---"; ls -lh i2pd

EXPECTED="$(git describe --tags)"
ACTUAL="$(./i2pd --version | head -1)"
echo "expected source revision: $EXPECTED"
echo "binary reports:           $ACTUAL"
case "$ACTUAL" in
  *"$EXPECTED"*) echo "version check: OK" ;;
  *) echo "version check: FAILED — binary does not report the pinned revision"; exit 1 ;;
esac

echo "--- architecture and minimum macOS ---"
lipo -archs i2pd
otool -l i2pd | grep -A4 LC_BUILD_VERSION | grep -E 'minos|sdk' || true

echo "--- dynamic dependencies (want: /usr/lib and /System only) ---"
otool -L i2pd
foreign="$(otool -L i2pd | tail -n +2 | awk '{print $1}' | grep -vE '^(/usr/lib/|/System/Library/)' || true)"
if [ -n "$foreign" ]; then
  echo "router links libraries that will not exist on a user's Mac:"
  echo "$foreign"
  exit 1
fi
