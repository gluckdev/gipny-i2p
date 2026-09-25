#!/usr/bin/env bash
# The reseed and family certificates as upstream i2pd has them *now* (its
# default branch), not as of the pinned submodule: reseed operators rotate and
# revoke them between i2pd releases, and a build must carry the current set.
#
#   scripts/fresh-i2p-certs.sh <dir>
#
# Lays out <dir>/reseed/*.crt and <dir>/family/*.crt, and in CI exports
# I2P_EMBED_CERTS_DIR=<dir> for the rest of the job (i2p-embed's build.rs
# compiles in what is there instead of the submodule's copy).
set -euo pipefail
dir=${1:?usage: fresh-i2p-certs.sh <dir>}
repo=https://github.com/PurpleI2P/i2pd
branch=$(git ls-remote --symref "$repo" HEAD | awk '/^ref:/ {sub("refs/heads/", "", $2); print $2}')
commit=$(git ls-remote "$repo" "refs/heads/$branch" | cut -f1)
[ -n "$commit" ] || { echo "cannot resolve upstream i2pd's $branch"; exit 1; }
rm -rf "$dir" && mkdir -p "$dir"
# From inside the dir: GNU tar reads "D:\…" (Windows) as host:path.
curl -fsSL --retry 3 "$repo/archive/$commit.tar.gz" \
  | (cd "$dir" && tar -xz --strip-components=3 "i2pd-$commit/contrib/certificates")
for sub in reseed family; do
  n=$(find "$dir/$sub" -name '*.crt' | wc -l)
  [ "$n" -gt 0 ] || { echo "upstream i2pd $commit has no $sub certificates"; exit 1; }
  echo "$n $sub certificates from i2pd $branch @ ${commit:0:12}"
done
if [ -n "${GITHUB_ENV:-}" ]; then
  abs=$(cd "$dir" && pwd)
  # Read by build.rs, a native program: a Windows path on Windows.
  if command -v cygpath >/dev/null; then abs=$(cygpath -w "$abs"); fi
  echo "I2P_EMBED_CERTS_DIR=$abs" >> "$GITHUB_ENV"
fi
