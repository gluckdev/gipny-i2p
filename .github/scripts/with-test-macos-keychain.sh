#!/usr/bin/env bash
# Real file-based OS Keychain fixture for free ad-hoc builds. No credential
# files or passwords leave this ephemeral runner; never a production fallback.
set -euo pipefail
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_OS:-}" != macOS ]; then
  echo 'This Keychain fixture runs only on GitHub Actions macOS runners.' >&2
  exit 1
fi
[ "$#" -gt 0 ] || { echo 'usage: with-test-macos-keychain.sh signed-test-binary [args...]' >&2; exit 1; }
state_dir=$(mktemp -d "${RUNNER_TEMP:?}/gipny-macos-keychains.XXXXXXXX")
chmod 700 "$state_dir"
original_default=$(security default-keychain -d user | sed -E 's/^[[:space:]]*"//;s/"[[:space:]]*$//')
original_search=()
while IFS= read -r item; do original_search+=("$item"); done < <(
  security list-keychains -d user | sed -E 's/^[[:space:]]*"//;s/"[[:space:]]*$//'
)
first="$state_dir/first.keychain-db"
second="$state_dir/second.keychain-db"
cleanup() {
  fixture_status=$?
  trap - EXIT
  security default-keychain -d user -s "$original_default" || true
  security list-keychains -d user -s "${original_search[@]}" || true
  security delete-keychain "$first" "$second" 2>/dev/null || true
  rm -rf -- "$state_dir"
  exit "$fixture_status"
}
trap cleanup EXIT
# Only synthetic CI Keychain passwords use security(1) flags. The application
# device secret is generated inside Rust and passed directly through SecItem.
fixture_password=$(openssl rand -hex 32)
for keychain in "$first" "$second"; do
  security create-keychain -p "$fixture_password" "$keychain"
  security set-keychain-settings -lut 3600 "$keychain"
  security unlock-keychain -p "$fixture_password" "$keychain"
done
unset fixture_password
security list-keychains -d user -s "$first" "$second"
security default-keychain -d user -s "$first"
export GIPNY_CI_MAC_KEYCHAINS_ACTIVE=1 GIPNY_CI_MAC_OTHER_KEYCHAIN="$second"
"$@"
