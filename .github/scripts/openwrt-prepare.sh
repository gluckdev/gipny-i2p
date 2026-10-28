#!/usr/bin/env bash
# Real C/C++ cross dependencies; never use the router stub in release builds.
# Usage: openwrt-prepare.sh <Rust target> <private cache/build directory>
set -euo pipefail
ow_target=${1:?Rust target required}
ow_cache=${2:?private build directory required}
ow_repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ow_repo/.github/scripts/openwrt-targets.sh" "$ow_target"
mkdir -p "$ow_cache"
ow_cache="$(cd "$ow_cache" && pwd)"
test "$ow_cache" != / && test "$ow_cache" != "$ow_repo"
mkdir -p "$ow_cache/downloads" "$ow_cache/toolchain" "$ow_cache/sources"

fetch_checked() {
  local url=$1 digest=$2 destination=$3
  if [ ! -f "$destination" ]; then
    curl --fail --silent --show-error --location --retry 3 "$url" -o "$destination.part"
    printf '%s  %s\n' "$digest" "$destination.part" | sha256sum -c -
    mv "$destination.part" "$destination"
  fi
  printf '%s  %s\n' "$digest" "$destination" | sha256sum -c -
}

fetch_checked "$ow_url" "$ow_sha256" "$ow_cache/downloads/$ow_archive"
if [ ! -d "$ow_cache/toolchain/${ow_archive%.tar.zst}" ]; then
  tar --zstd -xf "$ow_cache/downloads/$ow_archive" -C "$ow_cache/toolchain"
fi
ow_info=("$ow_cache/toolchain/${ow_archive%.tar.zst}"/toolchain-*/info.mk)
test "${#ow_info[@]}" -eq 1 && test -f "${ow_info[0]}"
ow_tc="$(dirname "${ow_info[0]}")"
ow_cross="$(sed -n 's/^TARGET_CROSS=//p' "${ow_info[0]}")"
test "$(sed -n 's/^LIBC_VERSION=//p' "${ow_info[0]}")" = 1.2.5
export STAGING_DIR="$ow_cache/toolchain"
export PATH="$ow_tc/bin:$PATH"
ow_cc="$ow_tc/bin/${ow_cross}gcc"
ow_cxx="$ow_tc/bin/${ow_cross}g++"
ow_ar="$ow_tc/bin/${ow_cross}ar"
ow_native="$ow_cache/native"
ow_jobs=${GIPNY_OPENWRT_JOBS:-2}
read -r -a ow_cpu_flags <<< "$ow_flags"

# Verify the compiler's prebuilt runtime ABI too. A soft-float flag cannot
# repair a hard-float libstdc++/libc; reject such a toolchain before compiling.
if [[ "$ow_target" == armv7-* ]]; then
  "$ow_cc" -Q --help=target > "$ow_cache/compiler-defaults.txt"
  grep -Eq 'mfloat-abi=.*soft$' "$ow_cache/compiler-defaults.txt"
  grep -Eq 'march=.*armv7-a(\+mp)?(\+sec)?$' "$ow_cache/compiler-defaults.txt"
fi
"$ow_cc" --version

# Use the exact OpenSSL source crate and checksum already pinned in Cargo.lock.
# Both i2pd and SQLCipher then link the same archive, without a second version.
read -r ow_ssl_version ow_ssl_sha < <(python3 - "$ow_repo/Cargo.lock" <<'PY'
import sys, tomllib
lock = tomllib.load(open(sys.argv[1], 'rb'))
p, = (p for p in lock['package'] if p['name'] == 'openssl-src')
print(p['version'], p['checksum'])
PY
)
ow_stamp="$(sha256sum "$ow_repo/.github/scripts/openwrt-prepare.sh" "$ow_repo/.github/scripts/openwrt-targets.sh" | sha256sum | cut -d' ' -f1):$ow_ssl_sha"
if [ "$(cat "$ow_native/.complete" 2>/dev/null || true)" != "$ow_stamp" ]; then
  mkdir -p "$ow_native/lib" "$ow_native/include" "$ow_cache/work"
  fetch_checked "https://archives.boost.io/release/1.74.0/source/boost_1_74_0.tar.gz" \
    afff36d392885120bcac079148c177d1f6f7730ec3d47233aa51b0afa4db94a5 "$ow_cache/downloads/boost.tar.gz"
  tar -xzf "$ow_cache/downloads/boost.tar.gz" -C "$ow_cache/sources" \
    boost_1_74_0/boost boost_1_74_0/libs/program_options/src
  ln -sfn ../../sources/boost_1_74_0/boost "$ow_native/include/boost"
  mkdir -p "$ow_cache/work/boost"
  for source in "$ow_cache/sources/boost_1_74_0/libs/program_options/src/"*.cpp; do
    "$ow_cxx" "${ow_cpu_flags[@]}" -O2 -fPIC -std=c++20 -DBOOST_ALL_NO_LIB \
      -I"$ow_native/include" -c "$source" -o "$ow_cache/work/boost/$(basename "${source%.cpp}").o"
  done
  "$ow_ar" crs "$ow_native/lib/libboost_program_options.a" "$ow_cache/work/boost/"*.o

  fetch_checked "https://static.crates.io/crates/openssl-src/openssl-src-$ow_ssl_version.crate" \
    "$ow_ssl_sha" "$ow_cache/downloads/openssl-src-$ow_ssl_version.crate"
  tar -xzf "$ow_cache/downloads/openssl-src-$ow_ssl_version.crate" -C "$ow_cache/work"
  (
    cd "$ow_cache/work/openssl-src-$ow_ssl_version/openssl"
    CC="$ow_cc" AR="$ow_ar" RANLIB="$ow_tc/bin/${ow_cross}ranlib" \
      CFLAGS="-O2 -fPIC $ow_flags" perl ./Configure "$ow_openssl" \
      no-shared no-tests no-module no-dso --prefix="$ow_native" --libdir=lib
    make -j"$ow_jobs" build_libs
    make install_dev
  )
  fetch_checked https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.xz \
    38ef96b8dfe510d42707d9c781877914792541133e1870841463bfa73f883e32 "$ow_cache/downloads/zlib.tar.xz"
  tar -xJf "$ow_cache/downloads/zlib.tar.xz" -C "$ow_cache/work"
  (
    cd "$ow_cache/work/zlib-1.3.1"
    CC="$ow_cc" AR="$ow_ar" RANLIB="$ow_tc/bin/${ow_cross}ranlib" \
      CFLAGS="-O2 -fPIC $ow_flags" ./configure --static --prefix="$ow_native"
    make -j"$ow_jobs"
    make install
  )
  printf '%s\n' "$ow_stamp" > "$ow_native/.complete"
fi

"$ow_cxx" "${ow_cpu_flags[@]}" -std=c++20 -O2 -static -static-libstdc++ -static-libgcc \
  -I"$ow_native/include" "$ow_repo/.github/scripts/openwrt-native-probe.cpp" \
  -L"$ow_native/lib" -lboost_program_options -lssl -lcrypto -lz -latomic -lgcc_eh -pthread \
  -o "$ow_cache/native-probe"

ow_key=${ow_target//-/_}; ow_upper=${ow_key^^}
{
  printf 'export %s=%q\n' STAGING_DIR "$STAGING_DIR" PATH "$PATH"
  printf 'export %s=%q\n' "CC_$ow_key" "$ow_cc" "CXX_$ow_key" "$ow_cxx" "AR_$ow_key" "$ow_ar"
  printf 'export %s=%q\n' "CFLAGS_$ow_key" "-O2 -fPIC $ow_flags" "CXXFLAGS_$ow_key" "-O2 -fPIC $ow_flags"
  printf 'export %s=%q\n' "CARGO_TARGET_${ow_upper}_LINKER" "$ow_cc"
  # OpenWrt's GCC specs omit -lgcc_eh even with -static-libgcc. C++ i2pd
  # catches exceptions, so link the real static unwinder explicitly.
  printf 'export %s=%q\n' "CARGO_TARGET_${ow_upper}_RUSTFLAGS" '-C target-feature=+crt-static -C link-arg=-static -C link-arg=-static-libstdc++ -C link-arg=-static-libgcc -C link-arg=-lgcc_eh'
  printf 'export %s=%q\n' I2P_EMBED_STATIC 1 I2P_EMBED_INCLUDE_DIRS "$ow_native/include" I2P_EMBED_LIB_DIRS "$ow_native/lib"
  printf 'export %s=%q\n' OPENSSL_DIR "$ow_native" OPENSSL_STATIC 1 OPENSSL_NO_VENDOR 1
  printf 'export %s=%q\n' GIPNY_OPENWRT_QEMU "$ow_qemu" GIPNY_OPENWRT_QEMU_CPU "$ow_qemu_cpu" GIPNY_OPENWRT_CC "$ow_cc" GIPNY_OPENWRT_CXX "$ow_cxx"
} > "$ow_cache/env.sh"
printf 'Source cross environment: %s\n' "$ow_cache/env.sh"
