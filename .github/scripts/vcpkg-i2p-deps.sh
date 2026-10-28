#!/usr/bin/env bash
# Windows: point i2p-embed's build.rs at boost and OpenSSL from vcpkg, static
# against the dynamic CRT Rust uses (triplet x64-windows-static-md). Installs
# them into <root> unless a cache already did. i2p-embed.yml and release.yml
# both call it, so the router that is tested is the router that ships.
#
#   .github/scripts/vcpkg-i2p-deps.sh <root>
set -euo pipefail
root_dir=${1:?usage: vcpkg-i2p-deps.sh <install root>}
ports_commit=9eec002df1cd109f8f104cd966a763daf9751489 # peeled commit of public vcpkg 2026.10.16
ports_root="${RUNNER_TEMP:?GitHub runner required}/gipny-vcpkg-ports"
if [ ! -d "$ports_root/.git" ]; then
  git init "$ports_root"
  git -C "$ports_root" remote add origin https://github.com/microsoft/vcpkg.git
fi
git -C "$ports_root" fetch --depth 1 origin "$ports_commit"
if [ "$(git -C "$ports_root" cat-file -t "$ports_commit")" != commit ]; then
  echo "vcpkg ports pin must be a commit object, not an annotated tag" >&2
  exit 1
fi
git -C "$ports_root" checkout --detach "$ports_commit"
test "$(git -C "$ports_root" rev-parse HEAD)" = "$ports_commit"
# The public SDK's vcpkg driver builds these pinned ports from source. Do not
# consume opaque library packages from an external vcpkg binary registry.
export VCPKG_BINARY_SOURCES=clear
echo "GIPNY_VCPKG_COMMIT=$ports_commit" >> "$GITHUB_ENV"
triplet=x64-windows-static-md
root="$root_dir/$triplet"
if [ ! -d "$root/lib" ]; then
  "$VCPKG_INSTALLATION_ROOT/vcpkg" install --triplet "$triplet" \
    --vcpkg-root="$ports_root" \
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
