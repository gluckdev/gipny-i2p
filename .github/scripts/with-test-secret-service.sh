#!/usr/bin/env bash
# GHA-only real Secret Service for the live agent scenario. No production
# fallback, checked-in password, profile keyfile, keyring cache or artifact.
set -euo pipefail
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_OS:-}" != Linux ]; then
  echo 'This credential fixture runs only on GitHub Actions Linux runners.' >&2
  exit 1
fi
[ "$#" -gt 0 ] || { echo 'usage: with-test-secret-service.sh command [args...]' >&2; exit 1; }
state_dir=$(mktemp -d "${RUNNER_TEMP:?}/gipny-secret-service.XXXXXXXX")
chmod 700 "$state_dir"
cleanup() { rm -rf -- "$state_dir"; }
trap cleanup EXIT
export GIPNY_CI_SECRET_STATE="$state_dir"
dbus-run-session -- bash -euo pipefail -c '
  umask 077
  export XDG_CONFIG_HOME="$GIPNY_CI_SECRET_STATE/config"
  export XDG_DATA_HOME="$GIPNY_CI_SECRET_STATE/data"
  export XDG_CACHE_HOME="$GIPNY_CI_SECRET_STATE/cache"
  export XDG_RUNTIME_DIR="$GIPNY_CI_SECRET_STATE/runtime"
  export GNOME_KEYRING_CONTROL="$GIPNY_CI_SECRET_STATE/control"
  mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR" "$GNOME_KEYRING_CONTROL"
  # Password is ephemeral, passed over stdin, never exported or logged.
  keyring_password=$(openssl rand -hex 32)
  printf "%s" "$keyring_password" | gnome-keyring-daemon \
    --foreground --unlock --components=secrets \
    --control-directory="$GNOME_KEYRING_CONTROL" \
    > "$GIPNY_CI_SECRET_STATE/daemon.log" 2>&1 &
  daemon_pid=$!
  unset keyring_password
  stop_daemon() {
    status=$?
    trap - EXIT
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
    exit "$status"
  }
  trap stop_daemon EXIT
  # The unlocked daemon must own org.freedesktop.secrets before the first
  # client call: otherwise D-Bus activates a second, locked daemon without a
  # login collection, and secret-tool fails on a prompt that has no display.
  owner=
  for _ in $(seq 1 100); do
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
      cat "$GIPNY_CI_SECRET_STATE/daemon.log" >&2
      exit 1
    fi
    owner=$(dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus \
      org.freedesktop.DBus.GetConnectionUnixProcessID string:org.freedesktop.secrets 2>/dev/null \
      | awk "/uint32/ {print \$2}") || true
    [ -z "$owner" ] || break
    sleep 0.2
  done
  if [ "$owner" != "$daemon_pid" ]; then
    echo "Secret Service is not owned by the unlocked test daemon (owner: ${owner:-none})" >&2
    exit 1
  fi
  # Native Secret Service registration/read/write must work before spending
  # time bootstrapping I2P. These are public probe bytes, not app credentials.
  printf "%s" synthetic-provider-probe | timeout 20s secret-tool store \
    --label="GIPNY CI provider probe" service app.gipny.ci binding-id probe
  probe=$(timeout 10s secret-tool lookup service app.gipny.ci binding-id probe)
  [ "$probe" = synthetic-provider-probe ]
  timeout 10s secret-tool clear service app.gipny.ci binding-id probe
  echo "Ephemeral native Secret Service ready; this is not TPM/hardware attestation."
  "$@"
' gipny-ci-secret-service "$@"
