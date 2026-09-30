#!/usr/bin/env bash
# Put third_party/i2pd at the revision this commit pins, prove it, and make the
# tree reusable by a restored build cache.
#
# Two jobs, in this order, and the order is the point.
#
# The pin check. The pin is the whole reason the router is vendored instead of
# tracked by name: i2p-embed compiles that exact tree into the app, and a
# release must never carry a router nobody looked at. Every build checks the
# submodule out fresh, over the network, at a revision chosen by whatever the
# `git submodule update` was pointed at — nothing in the build itself compares
# what landed on disk with what HEAD asks for. A job pointed at the wrong ref,
# or a submodule edited in the tree, would build happily and produce an
# artifact that looks like a release and is not the pinned one. Cheap to run,
# and it turns a silent substitution into a red build. Pair it with
# .github/scripts/i2pd-under-test.sh, which does the same job for the e2e jobs:
# there the revision under test is passed in by hand, here it comes from the
# tree itself.
#
# Then the timestamps. actions/checkout writes every file with the current
# time, so the C++ sources always look newer than the object files a restored
# build cache brought with it, and the cc crate rebuilds the whole router on
# every push — which is most of what these jobs cost. Stamping the sources with
# the commit's own date makes "changed" mean what it should: this is a
# different revision than last time, not a checkout.
#
# The stamp comes from the revision, so it cannot lie the way a "yesterday"
# stamp would: a different pin is a different date, and the cache misses. This
# only runs because the check above passed, so the tree being stamped is
# provably the pinned one.
#
# Not `-d @epoch`: BSD touch, on the macOS runners, takes no epoch. `%cd` with
# an explicit format is git rendering the commit's own timestamp, which is
# portable and deterministic, in the commit's own timezone.
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

stamp=$(git -C "$sub" log -1 --format=%cd --date=format:%Y%m%d%H%M.%S)

if [ -n "$stamp" ]; then
  # Tracked files only: that is the set cc compiles, and it is also the set
  # that must be reproducible for the stamp to mean anything.
  if [ -n "$(git -C "$sub" ls-files)" ]; then
    ( cd "$sub" && git ls-files -z | xargs -0 touch -t "$stamp" )
    echo "i2pd sources stamped $stamp (the pin's own date)"
  fi
fi
