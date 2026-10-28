#!/usr/bin/env bash
# Apply reviewed public source changes; never substitute a prebuilt router.
set -euo pipefail
repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
router_dir=${1:-"$repo_dir/third_party/i2pd"}
patch_file="$repo_dir/i2p-embed/patches/0001-clean-owned-private-material.patch"
git -C "$router_dir" rev-parse --verify HEAD >/dev/null
if git -C "$router_dir" apply --reverse --check "$patch_file" 2>/dev/null; then
  printf '%s\n' 'i2pd owned-private-buffer cleanup patch already applied'
else
  # Context mismatch is a hard failure; review a new pin rather than silently
  # shipping unpatched sources or accepting an unrelated partial modification.
  git -C "$router_dir" apply --check "$patch_file"
  git -C "$router_dir" apply "$patch_file"
  printf '%s\n' 'Applied public i2pd owned-private-buffer cleanup patch'
fi
