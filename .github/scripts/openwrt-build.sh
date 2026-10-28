#!/usr/bin/env bash
set -euo pipefail
target=${1:?Rust target required}
cache_dir=${2:?private build directory required}
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Never turn a stub/check-only result into a distributable agent.
test "${I2P_EMBED_STUB:-0}" != 1
test "${I2P_EMBED_SKIP_NATIVE:-0}" != 1
test -z "${I2P_EMBED_PREBUILT_DIR:-}"
source "$cache_dir/env.sh"
bash "$repo_dir/scripts/apply-i2pd-memory-patches.sh" "$repo_dir/third_party/i2pd"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$cache_dir/target}"
export CARGO_BUILD_JOBS="${GIPNY_OPENWRT_JOBS:-2}" CARGO_INCREMENTAL=0
cargo build --locked --release -p gipny-agent --target "$target"
binary="$CARGO_TARGET_DIR/$target/release/gipny-agent"
python3 "$repo_dir/.github/scripts/check-openwrt-elf.py" "$target" "$binary"
python3 "$repo_dir/.github/scripts/check-openwrt-elf.py" "$target" "$cache_dir/native-probe"
runner=()
if [ -n "$GIPNY_OPENWRT_QEMU" ]; then runner=("$GIPNY_OPENWRT_QEMU" -cpu "$GIPNY_OPENWRT_QEMU_CPU"); fi
timeout 30 "${runner[@]}" "$cache_dir/native-probe"
status=0
timeout 30 "${runner[@]}" "$binary" --help > "$cache_dir/agent-help.txt" 2>&1 || status=$?
test "$status" = 0 || test "$status" = 1
grep -F 'Usage: gipny-agent' "$cache_dir/agent-help.txt"
