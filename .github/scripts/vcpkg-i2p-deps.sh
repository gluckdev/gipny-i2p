#!/usr/bin/env bash
# Windows: point i2p-embed's build.rs at boost and OpenSSL from vcpkg, static
# against the dynamic CRT Rust uses (triplet x64-windows-static-md). Installs
# them into <root> unless a cache already did. i2p-embed.yml and release.yml
# both call it, so the router that is tested is the router that ships.
#
#   .github/scripts/vcpkg-i2p-deps.sh <root>
set -euo pipefail
root_dir=${1:?usage: vcpkg-i2p-deps.sh <install root>}
triplet=x64-windows-static-md
root="$root_dir/$triplet"
if [ ! -d "$root/lib" ]; then
  "$VCPKG_INSTALLATION_ROOT/vcpkg" install --triplet "$triplet" \
    --x-install-root="$root_dir" \
    boost-asio boost-program-options boost-algorithm boost-property-tree \
    boost-system boost-lexical-cast boost-beast boost-atomic boost-smart-ptr \
    openssl zlib
fi
# The library names carry the toolset and boost version; take what is there.
po="$(cd "$root/lib" && ls boost_program_options*.lib | head -1)"
at="$(cd "$root/lib" && { ls boost_atomic*.lib 2>/dev/null || true; } | head -1)"
# zlib's static library is zs.lib in a static triplet, zlib.lib otherwise.
z=""
for cand in zs.lib zlib.lib; do [ -f "$root/lib/$cand" ] && { z=$cand; break; }; done
[ -n "$z" ] || { echo "no zlib in $root/lib"; exit 1; }
libs="${po%.lib},libssl,libcrypto,${z%.lib}${at:+,${at%.lib}}"
echo "libraries: $libs"
{
  echo "I2P_EMBED_INCLUDE_DIRS=$(cygpath -w "$root/include")"
  echo "I2P_EMBED_LIB_DIRS=$(cygpath -w "$root/lib")"
  echo "I2P_EMBED_LIBS=$libs"
} >> "$GITHUB_ENV"
