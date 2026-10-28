#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
source "$repo_dir/scripts/install-agent.sh"
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
