#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob
packages=(core/gen/apple/build/arm64/*.ipa)
[[ ${#packages[@]} == 1 ]]
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
ditto -x -k "${packages[0]}" "$stage"
apps=("$stage"/Payload/*.app)
[[ ${#apps[@]} == 1 ]]
info="${apps[0]}/Info.plist"
[[ $(plutil -extract CFBundleIdentifier raw "$info") == app.gipny.i2p ]]
[[ -n $(plutil -extract NSMicrophoneUsageDescription raw "$info") ]]
exe="${apps[0]}/$(plutil -extract CFBundleExecutable raw "$info")"
lipo -verify_arch arm64 "$exe"
xcrun vtool -show-build "$exe" | grep -Eq 'platform IOS$'
mkdir -p ios-artifacts
cp "${packages[0]}" ios-artifacts/Gipny-iOS-arm64-unsigned.ipa
(cd ios-artifacts && shasum -a 256 Gipny-iOS-arm64-unsigned.ipa > SHA256SUMS.txt)
