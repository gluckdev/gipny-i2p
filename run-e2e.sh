#!/bin/bash
# Local e2e loop against the MOCK SAM server — no i2p router, no reseed, no
# tunnels. Seconds instead of ~15-30 min, but it proves nothing about the i2p
# transport; only the nightly e2e.yml job does that.
#
# The mock lives behind the `mocksam` build tag so it cannot end up in a shipped
# binary, hence the separate build below into gipny-i2p-router-mock.
set -e

mkdir -p e2e-state/relay-router e2e-state/relay e2e-state/bot-a e2e-state/bot-b e2e-logs
rm -f e2e-state/relay/dest.pub

echo "[e2e] building mock router (-tags mocksam)..."
CGO_ENABLED=0 go build -C i2p-router -tags mocksam \
  -o "$PWD/gipny-i2p-router-mock" .

echo "[e2e] starting router (MOCK — not real i2p)..."
./gipny-i2p-router-mock \
  --sam-listen 127.0.0.1:7656 \
  --data e2e-state/relay-router \
  --debug \
  --mock \
  > e2e-logs/relay-router.log 2>&1 &
ROUTER_PID=$!

cleanup() {
  echo "[e2e] cleaning up..."
  kill "$RELAY_PID" 2>/dev/null || true
  kill "$ROUTER_PID" 2>/dev/null || true
}
trap cleanup EXIT

echo "[e2e] waiting for SAM on 7656..."
for i in $(seq 1 60); do
  if timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/7656' 2>/dev/null; then
    echo "[e2e] SAM ready!"
    break
  fi
  if ! kill -0 "$ROUTER_PID" 2>/dev/null; then
    echo "[e2e] mock router died during startup!"
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

echo "[e2e] waiting for dest.pub..."
for i in $(seq 1 60); do
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
export E2E_N_MESSAGES=2
export E2E_TIMEOUT_SECS=300
export E2E_WORK_DIR=e2e-state
export GIPNY_SAM_PORT=7656

./target/release/e2e-harness
echo "[e2e] test finished successfully!"
