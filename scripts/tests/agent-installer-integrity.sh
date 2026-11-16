#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
source "$repo_dir/scripts/install-agent.sh"

# Simulate macOS host queries, without pretending Linux is an ARM runner.
sysctl() {
  [[ $1 == -in ]] || return 2
  case $2 in
    sysctl.proc_translated) [[ ${GIPNY_TEST_TRANSLATED:-missing} != missing ]] || return 1; printf '%s\n' "$GIPNY_TEST_TRANSLATED" ;;
    hw.optional.arm64) [[ ${GIPNY_TEST_ARM64:-missing} != missing ]] || return 1; printf '%s\n' "$GIPNY_TEST_ARM64" ;;
    *) return 2 ;;
  esac
}
[[ $(detect_agent_arch darwin x86_64) == amd64 ]]
GIPNY_TEST_TRANSLATED=0 GIPNY_TEST_ARM64=0
[[ $(detect_agent_arch darwin x86_64) == amd64 ]]
GIPNY_TEST_TRANSLATED=1 GIPNY_TEST_ARM64=missing
[[ $(detect_agent_arch darwin x86_64) == arm64 ]]
GIPNY_TEST_TRANSLATED=missing GIPNY_TEST_ARM64=1
[[ $(detect_agent_arch darwin x86_64) == arm64 ]]
[[ $(detect_agent_arch darwin arm64) == arm64 ]]
[[ $(detect_agent_arch linux x86_64) == amd64 ]]
[[ $(detect_agent_arch linux aarch64) == arm64 ]]
if detect_agent_arch linux mipsel 2>/dev/null; then echo 'Unsupported CPU accepted' >&2; exit 1; fi
unset -f sysctl

release_base=https://github.com/gluckdev/gipny-i2p/releases/download/v1.2.3
amd_url=$release_base/gipny-agent_1.2.3_darwin-amd64.tar.gz
arm_url=$release_base/gipny-agent_1.2.3_darwin-arm64.tar.gz
sums_url=$release_base/SHA256SUMS.txt
selected=$(select_agent_release_urls gluckdev/gipny-i2p 1.2.3 darwin arm64 <<< "$amd_url"$'\n'"$arm_url"$'\n'"$sums_url")
[[ $selected == "$arm_url"$'\n'"$sums_url" ]]
# No emulated-Intel fallback when native ARM64 is absent; duplicates and
# foreign repositories also fail before downloads or install writes.
for urls in "$amd_url"$'\n'"$sums_url" "$arm_url"$'\n'"$arm_url"$'\n'"$sums_url" \
  "$arm_url" "${arm_url/gluckdev\/gipny-i2p/foreign\/repo}"$'\n'"$sums_url"; do
  if select_agent_release_urls gluckdev/gipny-i2p 1.2.3 darwin arm64 <<< "$urls" >/dev/null 2>&1; then
    echo 'Missing/ambiguous/foreign native package accepted' >&2; exit 1
  fi
done
printf '%s\n' 'Agent host-architecture/Rosetta/exact release selection regressions: PASS'

probe_dir=$(mktemp -d)
trap 'rm -rf -- "$probe_dir"' EXIT
printf '%s' 'synthetic verified release archive' > "$probe_dir/archive"
digest=$(sha256sum "$probe_dir/archive"); digest=${digest%% *}
printf '%s  %s\n' "$digest" agent.tar.gz > "$probe_dir/manifest"
verify_agent_archive "$probe_dir/archive" "$probe_dir/manifest" agent.tar.gz
printf '%s *./%s\r\n' "${digest^^}" agent.tar.gz > "$probe_dir/manifest"
verify_agent_archive "$probe_dir/archive" "$probe_dir/manifest" agent.tar.gz

reject() {
  if verify_agent_archive "$probe_dir/archive" "$probe_dir/manifest" agent.tar.gz 2>/dev/null; then
    printf '%s\n' 'Installer integrity regression: unsafe manifest/archive accepted' >&2
    exit 1
  fi
}
printf '%s  %s\n' "$digest" other.tar.gz > "$probe_dir/manifest"; reject
printf '%s  %s\n%s  %s\n' "$digest" agent.tar.gz "$digest" agent.tar.gz > "$probe_dir/manifest"; reject
printf '%s\n' 'malformed checksum' > "$probe_dir/manifest"; reject
printf '%s  %s\n%s\n' "$digest" agent.tar.gz garbage > "$probe_dir/manifest"; reject
printf '%s  %s\n' "$digest" agent.tar.gz > "$probe_dir/manifest"
printf '%s' 'corrupted bytes' > "$probe_dir/archive"; reject
[[ $(cat "$probe_dir/archive") == 'corrupted bytes' ]]
printf '%s\n' 'Agent installer checksum valid/binary/duplicate/missing/malformed/mismatch regressions: PASS'

# Cards are parsed by the checksum-verified staged executable before the
# installer stops a service or replaces data. This fixture tests argument
# passing, failure propagation and quiet handling without compiling anything.
card=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["valid"][0]["encoded"])' "$repo_dir/tests/fixtures/contact-cards-v3.json")
master_card_shape "$card"
for malformed in 'gipny:v1:legacy-without-relay' 'gipny:v3:short' "$card=" "$card " 'not-a-card'; do
  if master_card_shape "$malformed"; then echo 'Invalid master card shape accepted' >&2; exit 1; fi
done
cat > "$probe_dir/agent" <<'FAKE'
#!/bin/sh
[ "$#" = 2 ] && [ "$1" = --check-master-card ] && [ "$2" = "$GIPNY_TEST_CARD" ] || exit 19
printf '%s\n' 'synthetic child stdout'
printf '%s\n' 'synthetic child stderr' >&2
exit "${GIPNY_TEST_CARD_EXIT:-0}"
FAKE
chmod 0755 "$probe_dir/agent"
export GIPNY_TEST_CARD=$card
[[ -z $(verify_staged_master "$probe_dir/agent" "$card") ]]
export GIPNY_TEST_CARD_EXIT=17
if verify_staged_master "$probe_dir/agent" "$card"; then echo 'Rejected staged card was accepted' >&2; exit 1; fi
printf '%s\n' 'Agent installer v3 staged-validation/quiet-failure regressions: PASS'
