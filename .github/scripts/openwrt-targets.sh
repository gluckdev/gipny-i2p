#!/usr/bin/env bash
# Shared allowlist: release/PR builds use the same audited toolchain and ABI.
# Source with a Rust target triple as $1. SHA256 values: official 24.10.4 index.
case "${1:?Rust target required}" in
  aarch64-unknown-linux-musl)
    ow_board=armsr/armv8; ow_image=armsr-armv8; ow_libc=musl
    ow_sha256=01f179a1eee53887a672e9560dda44eba7ef7689b9b9bd48e8e033efeded8f99
    ow_flags='-march=armv8-a'; ow_openssl=linux-aarch64; ow_qemu=qemu-aarch64; ow_qemu_cpu=cortex-a53
    ;;
  armv7-unknown-linux-musleabi)
    # Cortex-A9 is ARMv7-A. mt7629's Cortex-A7 sysroot uses ARMv7VE;
    # merely overriding its compiler flags would leave incompatible archives.
    ow_board=bcm53xx/generic; ow_image=bcm53xx-generic; ow_libc=musl_eabi
    ow_sha256=3646bb39fac2a840d68e015e73e393a1c6370ca99c07b27396535aab306efbe9
    ow_flags='-march=armv7-a -mtune=generic-armv7-a -mfloat-abi=soft'; ow_openssl=linux-armv4; ow_qemu=qemu-arm; ow_qemu_cpu=cortex-a9
    ;;
  x86_64-unknown-linux-musl)
    ow_board=x86/64; ow_image=x86-64; ow_libc=musl
    ow_sha256=e3cf79964a00693a6839f2a9b7220ecafcff334d980a4b70d43206d3df0184d6
    ow_flags='-march=x86-64 -mtune=generic'; ow_openssl=linux-x86_64; ow_qemu=; ow_qemu_cpu=
    ;;
  i686-unknown-linux-musl)
    ow_board=x86/generic; ow_image=x86-generic; ow_libc=musl
    ow_sha256=a1ab1c200014e07b32152b5b621cedf5d8a7fae8724b78f0ed44c9f04cdb2738
    ow_flags='-march=pentium4 -mtune=generic'; ow_openssl=linux-x86; ow_qemu=qemu-i386; ow_qemu_cpu=qemu32
    ;;
  *) printf 'Unsupported OpenWrt release target: %s\n' "$1" >&2; return 1 2>/dev/null || exit 1 ;;
esac
ow_archive="openwrt-toolchain-24.10.4-${ow_image}_gcc-13.3.0_${ow_libc}.Linux-x86_64.tar.zst"
ow_url="https://downloads.openwrt.org/releases/24.10.4/targets/$ow_board/$ow_archive"
