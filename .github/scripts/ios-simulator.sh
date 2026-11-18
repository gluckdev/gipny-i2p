#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob
apps=(core/gen/apple/build/arm64-sim/*.app)
[[ ${#apps[@]} == 1 ]]
udid=$(xcrun simctl list devices available -j | python3 -c '
import json, sys
devices = json.load(sys.stdin)["devices"]
print(next(d["udid"] for runtime, entries in sorted(devices.items(), reverse=True)
           if ".iOS-" in runtime for d in entries if d["name"].startswith("iPhone")))')
xcrun simctl boot "$udid"
xcrun simctl bootstatus "$udid" -b
xcrun simctl install "$udid" "${apps[0]}"
xcrun simctl launch "$udid" app.gipny.i2p
sleep 5
xcrun simctl spawn "$udid" launchctl list | grep -E '^[0-9]+[[:space:]].*app\.gipny\.i2p'
