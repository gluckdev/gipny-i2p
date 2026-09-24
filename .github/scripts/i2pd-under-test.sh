#!/usr/bin/env bash
# Put third_party/i2pd at the revision e2e-i2pd.yml tests, and install what
# compiling it in needs. Every e2e job calls this, so they cannot drift into
# testing different routers: each compiles the same revision into its own
# binaries (i2p-embed), and there is no router beside them.
#
#   .github/scripts/i2pd-under-test.sh <sha>
set -euo pipefail
sha=${1:?usage: i2pd-under-test.sh <sha>}
git submodule update --init --recursive -- third_party/i2pd
git -C third_party/i2pd fetch --quiet origin "$sha" || git -C third_party/i2pd fetch --quiet origin
git -C third_party/i2pd checkout --quiet "$sha"
echo "i2pd under test: $(git -C third_party/i2pd log --oneline -1)"
sudo apt-get update -qq
sudo apt-get install -y -qq libboost-dev libboost-program-options-dev libssl-dev zlib1g-dev
mkdir -p e2e-state e2e-logs
