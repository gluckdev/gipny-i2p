#!/usr/bin/env bash
# The live test opens a real router and sends a real message over the real i2p
# network, so it fails sometimes for reasons that have nothing to do with the
# code: a tunnel that does not come up this second, a peer that stops answering
# mid-stream, a reseed that lands while the test is connecting. That is what
# "Custom { kind: UnexpectedEof, error: \"early eof\" }" on a run whose pin check
# passed and whose build is byte-identical to a green one is — the transport, not
# the build.
#
# Retrying every failure would hide real bugs, so only known-transient
# signatures are retried. Anything else — a failed assertion, a compile error, a
# panic in our own code — fails on the first attempt, because that is the case
# where retrying only costs 45 minutes of runner and tells us nothing new.
#
# Usage: live-test-retry.sh <label> <command...>
# Env:  LIVE_TEST_ATTEMPTS (default 3)
# Writes the last attempt's output to live.txt, which the summary steps grep.

set -uo pipefail

label="${1:?usage: live-test-retry.sh <label> <command...>}"
shift
[ "$#" -gt 0 ] || { echo "no command given" >&2; exit 2; }

attempts="${LIVE_TEST_ATTEMPTS:-3}"
out="live.txt"

# Transport-level trouble only. Deliberately specific: a broad "error" match
# would swallow the assertion failures these retries exist not to hide.
transient='early eof|UnexpectedEof|connection reset|ECONNRESET|EPIPE|broken pipe|tunnel[^[:alnum:]]*(timeout|failed|unavailable|no route)|failed to (connect|establish|accept)|peer[^[:alnum:]]*(unreachable|not found)|handshake (timed out|failed)|read on the server|reseed'

is_transient() {
  grep -qaE "$transient" "$out"
}

# Kept for the summary even on a passing retry, so a green run that took three
# tries says so instead of looking like it always works.
retried=0
skipped_first=0

for attempt in $(seq 1 "$attempts"); do
  if [ "$attempt" -gt 1 ]; then
    retried=1
    # The router's own netDb and tunnels from a failed attempt are what the next
    # one trips over, so start each try from a clean directory.
    rm -rf "${I2P_EMBED_TEST_DIR:?}"
    echo "::notice::attempt $attempt of $attempts: giving the network another go"
  fi

  "$@" 2>&1 | tee "$out"
  rc=${PIPESTATUS[0]}

  if [ "$rc" -eq 0 ]; then
    if [ "$retried" -eq 1 ]; then
      echo "::notice::passed on attempt $attempt of $attempts — the earlier failures were transport, not the build"
    fi
    exit 0
  fi

  if [ "$attempt" -eq "$attempts" ]; then
    echo "::error::$label failed on all $attempts attempts (last exit $rc)"
    exit "$rc"
  fi

  if ! is_transient; then
    echo "::error::$label failed with exit $rc, and not in a way that reads as the network. Not retrying — a retry would only spend a runner to fail the same way."
    exit "$rc"
  fi

  echo "::notice::$label failed on attempt $attempt with what looks like a transport problem, retrying"
  sleep 20
done

# Unreachable: the loop always exits.
echo "::error::$label: retry loop fell through" >&2
exit 1
