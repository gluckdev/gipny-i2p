#!/bin/bash
# Local e2e loop over a real i2pd router: relay + two bots, messages carried by
# live i2p. Expect ~15-30 min on a cold profile (reseed + tunnel building), and
# expect it to need a working uplink.
#
# There is no mock any more. The old `-tags mocksam` loop ran in seconds but
# bypassed i2p entirely, so a pass told you nothing about the transport — and
# that is exactly how a dead transport stayed hidden for weeks (see
# docs/i2p-transport-evaluation.md).
#
# Router binary: $GIPNY_I2P_BIN, else core/resources/i2pd, else i2pd on PATH.
set -e

ROUTER_BIN="${GIPNY_I2P_BIN:-}"
if [ -z "$ROUTER_BIN" ]; then
  if [ -x core/resources/i2pd ]; then
    ROUTER_BIN=core/resources/i2pd
  elif command -v i2pd >/dev/null 2>&1; then
    ROUTER_BIN=$(command -v i2pd)
  else
    echo "[e2e] no i2pd found: set GIPNY_I2P_BIN, put one in core/resources/, or apt install i2pd" >&2
    exit 1
  fi
fi
echo "[e2e] router: $ROUTER_BIN ($("$ROUTER_BIN" --version 2>&1 | head -1))"

mkdir -p e2e-state/relay-router e2e-state/relay e2e-state/bot-a e2e-state/bot-b e2e-logs
rm -f e2e-state/relay/dest.pub

echo "[e2e] starting i2pd (SAM on 127.0.0.1:7656; first run reseeds)..."
"$ROUTER_BIN" \
  --datadir="$PWD/e2e-state/relay-router" \
  --sam.enabled=true \
  --sam.address=127.0.0.1 \
  --sam.port=7656 \
  --http.enabled=false \
  --httpproxy.enabled=false \
  --socksproxy.enabled=false \
  --upnp.enabled=false \
  --log=stdout \
  > e2e-logs/relay-router.log 2>&1 &
ROUTER_PID=$!

cleanup() {
  echo "[e2e] cleaning up..."
  kill "$RELAY_PID" 2>/dev/null || true
  kill "$ROUTER_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Tunnel building, not just process start: SAM answering means the bridge is up,
# and the harness then waits on its own sessions.
echo "[e2e] waiting for SAM on 7656 (up to 5 min)..."
for i in $(seq 1 300); do
  if timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/7656' 2>/dev/null; then
    echo "[e2e] SAM ready after ${i}s"
    break
  fi
  if ! kill -0 "$ROUTER_PID" 2>/dev/null; then
    echo "[e2e] i2pd died during startup!"
    tail -50 e2e-logs/relay-router.log
    exit 1
  fi
  sleep 1
done

echo "[e2e] starting relay..."
GIPNY_RELAY_DATA=e2e-state/relay \
./core/relay/target/release/gipny-relay \
  > e2e-logs/relay.log 2>&1 &
RELAY_PID=$!

echo "[e2e] waiting for dest.pub (relay needs its own tunnels; up to 10 min)..."
for i in $(seq 1 600); do
  [ -s e2e-state/relay/dest.pub ] && break
  sleep 1
done

if [ ! -s e2e-state/relay/dest.pub ]; then
  echo "[e2e] dest.pub never appeared!"
  tail -50 e2e-logs/relay.log
  exit 1
fi

RELAY_DEST=$(cat e2e-state/relay/dest.pub)
echo "[e2e] relay destination: $RELAY_DEST"

echo "[e2e] running e2e harness..."
export E2E_RELAY_DEST=$RELAY_DEST
export E2E_N_MESSAGES=${E2E_N_MESSAGES:-2}
export E2E_TIMEOUT_SECS=${E2E_TIMEOUT_SECS:-900}
export E2E_WORK_DIR=e2e-state
export GIPNY_SAM_PORT=7656

./target/release/e2e-harness
echo "[e2e] test finished successfully!"
