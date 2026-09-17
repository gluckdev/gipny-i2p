#!/usr/bin/env bash
# Emulator smoke test for the no-router x86_64 APK (build.yml, "android emulator smoke").
#
# A file rather than an inline `script:` because android-emulator-runner runs
# every line of an inline script in its own `sh -c`: variables set on one line
# (APK, START_OUTPUT) were empty on the next, so `adb install ""` failed.
set -e

APK=$(find apk-x86_64 -name '*.apk' | head -1)
test -n "$APK" || { echo "no APK in apk-x86_64"; exit 1; }
echo "Installing $APK"
adb install "$APK"

# Cold start: `am start -W` blocks until the activity is displayed
# and reports TotalTime (ms from launch to first frame).
# The installed id is app.gipny.i2p, but the Kotlin namespace (and so the
# activity class) is app.gipny: `.MainActivity` would resolve to a class that
# does not exist.
START_OUTPUT=$(adb shell am start -W -n app.gipny.i2p/app.gipny.MainActivity)
echo "$START_OUTPUT"
COLD_MS=$(echo "$START_OUTPUT" | awk '/TotalTime/ {print $2}')

# Assert the process is alive.
adb shell pidof app.gipny.i2p

# GipnyService: wait up to 15 s for the foreground-service start log.
# A hard assertion; the job has continue-on-error, so it never blocks others.
timeout 15 adb logcat -s GipnyService | grep -m1 "GipnyService started"

# No SAM assertion: this APK ships no router by construction. Record what the
# service said about it so a broken JNI path is still visible.
timeout 10 adb logcat -d -s GipnyService | tail -20 || true

# Memory snapshot.
MEMINFO=$(adb shell dumpsys meminfo app.gipny.i2p 2>/dev/null || echo "")
PSS_KB=$(echo "$MEMINFO" | awk '/TOTAL PSS/ {print $NF}' | head -1)

{
  echo "### Android Emulator Smoke (x86_64, API 33)"
  echo ""
  echo "| Metric | Value |"
  echo "|--------|-------|"
  echo "| Cold start (TotalTime) | ${COLD_MS:-n/a} ms |"
  echo "| Total PSS | ${PSS_KB:-n/a} kB |"
} >> "$GITHUB_STEP_SUMMARY"
