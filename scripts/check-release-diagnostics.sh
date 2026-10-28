#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
probe_dir="$(mktemp -d)"
trap 'rm -rf "$probe_dir"' EXIT
rustc --edition=2021 -C opt-level=3 -C debug-assertions=no "$repo_dir/i2p-embed/tests/diagnostics_probe.rs" -o "$probe_dir/release-policy"
GIPNY_DEBUG_LOG=1 GIPNY_I2P_LOGLEVEL=debug "$probe_dir/release-policy" 2>"$probe_dir/release.stderr"
test ! -s "$probe_dir/release.stderr"
if LC_ALL=C grep -aFq 'GIPNY_PRIVATE_TRACE_SENTINEL_20261028' "$probe_dir/release-policy"; then
  printf '%s\n' 'Release diagnostic trace survived in the binary' >&2
  exit 1
fi
rustc --edition=2021 -C debug-assertions=yes "$repo_dir/i2p-embed/tests/diagnostics_probe.rs" -o "$probe_dir/developer-policy"
"$probe_dir/developer-policy" 2>"$probe_dir/developer.stderr"
grep -Fq 'GIPNY_PRIVATE_TRACE_SENTINEL_20261028' "$probe_dir/developer.stderr"
rustc --edition=2021 --test "$repo_dir/i2p-embed/src/diagnostics.rs" -o "$probe_dir/policy-tests"
"$probe_dir/policy-tests"
printf '%s\n' 'Release/developer diagnostics policy and release binary sentinel: PASS'
