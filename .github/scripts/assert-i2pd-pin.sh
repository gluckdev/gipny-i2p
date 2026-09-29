#!/usr/bin/env bash
# Fail unless the router in third_party/i2pd is the revision this commit pins.
#
# The pin is the whole reason the router is vendored instead of tracked by
# name: i2p-embed compiles that exact tree into the app, and a release must
# never carry a router nobody looked at. But every build checks the submodule
# out fresh, over the network, at a revision chosen by whatever the last
# `git submodule update` was pointed at — nothing in the build itself compares
# what landed on disk with what HEAD asks for. A job pointed at the wrong
# ref, or a submodule file edited in the tree, would build happily and produce
# an artifact that looks like a release and is not the pinned one.
#
# Cheap to run, and it turns a silent substitution into a red build. Pair it
# with .github/scripts/i2pd-under-test.sh, which does the same job for the
# e2e jobs: there the revision under test is passed in by hand, here it comes
# from the tree itself.
set -euo pipefail

sub=third_party/i2pd

# ls-tree reads the index's idea of the submodule, which is the pin as
# committed. --format keeps it to the bare object name.
expected=$(git ls-tree HEAD -- "$sub" | awk '{print $3}')

if [ -z "$expected" ]; then
  echo "$sub is not a submodule in this commit" >&2
  exit 1
fi

if [ ! -e "$sub/.git" ]; then
  echo "$sub is not checked out; run 'git submodule update --init $sub' first" >&2
  exit 1
fi

actual=$(git -C "$sub" rev-parse HEAD)

if [ "$expected" != "$actual" ]; then
  echo "i2pd pin mismatch in $sub" >&2
  echo "  HEAD pins: $expected" >&2
  echo "  on disk:   $actual" >&2
  echo "Building this would ship a router other than the one under review." >&2
  exit 1
fi

echo "i2pd pin: $actual"
