#!/usr/bin/env bash
# boost_program_options as a static library built with -fPIC, into $1/lib.
#
# The desktop app's crate is also a cdylib (the Android build needs it), and a
# shared object cannot take code built without -fPIC. Ubuntu 22.04 ships
# libboost_program_options.a without it on x86_64 (rust-lld: "relocation
# R_X86_64_PC32 cannot be used against symbol ...; recompile with -fPIC",
# release run 36056978015). Built from the same version as the image's headers
# (1.74), so both halves agree.
set -euo pipefail

out="$1"
version=1.74.0
sha256=afff36d392885120bcac079148c177d1f6f7730ec3d47233aa51b0afa4db94a5

if [ -f "$out/lib/libboost_program_options.a" ]; then
  echo "boost_program_options (PIC) already in $out"
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tarball="boost_${version//./_}.tar.gz"
if [ -n "${BOOST_TARBALL:-}" ]; then cp "$BOOST_TARBALL" "$work/$tarball"; else curl -sSfL -o "$work/$tarball" "https://archives.boost.io/release/$version/source/$tarball"; fi
echo "$sha256  $work/$tarball" | sha256sum -c -
tar -C "$work" -xzf "$work/$tarball"
cd "$work/boost_${version//./_}"
./bootstrap.sh --with-libraries=program_options >/dev/null
./b2 -j"$(nproc)" -d0 link=static runtime-link=shared variant=release \
  cxxflags=-fPIC cflags=-fPIC --with-program_options \
  --prefix="$out" --libdir="$out/lib" --includedir="$work/unused-headers" install
ls -l "$out/lib/"
