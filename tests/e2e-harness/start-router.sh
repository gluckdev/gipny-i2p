#!/bin/bash
# Start the shared i2pd router the e2e jobs run against, and wait until it has
# tunnels. Both jobs in .github/workflows/e2e-i2pd.yml call this, so the relay
# job and the in-process relay job cannot drift into testing different routers.
#
# Leaves the pid in relay-router.pid and logs in e2e-logs/. Expects i2pd on PATH
# and the workspace as the working directory.
set -e
: "${GITHUB_WORKSPACE:=$PWD}"

mkdir -p e2e-state/relay-router e2e-state/relay e2e-state/bot-a e2e-state/bot-b e2e-logs
I2PD_DATA_DIR="$GITHUB_WORKSPACE/e2e-state/relay-router"
I2PD_LOG_FILE="$GITHUB_WORKSPACE/e2e-logs/relay-router.log"

nohup i2pd \
  --datadir="$I2PD_DATA_DIR" \
  --sam.enabled=true --sam.address=127.0.0.1 --sam.port=7656 \
  --http.enabled=false --httpproxy.enabled=false --socksproxy.enabled=false \
  --log=file --logfile="$I2PD_LOG_FILE" --loglevel=info \
  > e2e-logs/relay-router.stdout.log 2>&1 &
echo $! > relay-router.pid
echo "[e2e-setup] relay router pid=$(cat relay-router.pid)"

# Wait for SAM to be ready (bash /dev/tcp probe — nc not guaranteed)
echo "[e2e-setup] waiting for SAM on 127.0.0.1:7656..."
for i in $(seq 1 180); do
  if timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/7656' 2>/dev/null; then
    echo "[e2e-setup] SAM ready after ${i}×2 s"
    break
  fi
  if ! kill -0 "$(cat relay-router.pid)" 2>/dev/null; then
    echo "relay router died during startup"
    tail -50 e2e-logs/relay-router.log
    exit 1
  fi
  sleep 2
done

# Wait until the router reports a couple of outbound/inbound tunnels.
# This is a stronger readiness signal than sleeping for a fixed window.
echo "[e2e-setup] waiting for i2pd tunnels..."
outbound=0
inbound=0
for i in $(seq 1 180); do
  # Count over the whole log: i2pd is chatty at info level, and a
  # tail window can scroll past the tunnel lines and undercount.
  outbound="$(grep -c 'Tunnel: Outbound tunnel .* has been created' e2e-logs/relay-router.log || true)"
  inbound="$(grep -c 'Tunnel: Inbound tunnel .* has been created' e2e-logs/relay-router.log || true)"
  if [ "$outbound" -ge 2 ] && [ "$inbound" -ge 2 ]; then
    echo "[e2e-setup] i2pd tunnels ready after ${i}×2 s (outbound=$outbound inbound=$inbound)"
    break
  fi
  if ! kill -0 "$(cat relay-router.pid)" 2>/dev/null; then
    echo "relay router died during tunnel warmup"
    tail -50 e2e-logs/relay-router.log
    exit 1
  fi
  sleep 2
done
if [ "$outbound" -lt 2 ] || [ "$inbound" -lt 2 ]; then
  echo "i2pd tunnels did not warm up in time (outbound=$outbound inbound=$inbound)"
  tail -120 e2e-logs/relay-router.log
  exit 1
fi
