#!/usr/bin/env bash
# Validate the actual app from a native GitHub macOS build before upload.
set -euo pipefail

app="${1:?usage: verify-macos-bundle.sh APP_PATH RUST_TARGET}"
target="${2:?usage: verify-macos-bundle.sh APP_PATH RUST_TARGET}"
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) printf 'Unsupported macOS release target: %s\n' "$target" >&2; exit 1 ;;
esac

plist="$app/Contents/Info.plist"
test -f "$plist"
name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist")
if [[ "$name" != gipny ]]; then
  printf 'Unexpected app executable: %s\n' "$name" >&2
  exit 1
fi

main="$app/Contents/MacOS/$name"
test -x "$main"
shopt -s nullglob dotglob
entries=("$app/Contents/MacOS/"*)
if [[ ${#entries[@]} -ne 1 || "${entries[0]}" != "$main" ]]; then
  printf 'Desktop app must contain only its native gipny executable; unexpected Contents/MacOS entries:\n' >&2
  printf '  %s\n' "${entries[@]}" >&2
  exit 1
fi

actual_arch=$(lipo -archs "$main")
if [[ "$actual_arch" != "$arch" ]]; then
  printf 'Wrong app architecture: expected %s, got %s\n' "$arch" "$actual_arch" >&2
  exit 1
fi
codesign --verify --deep --strict --all-architectures --verbose=2 "$app"
printf 'macOS app bundle verified: %s (%s); no server gateway bundled\n' "$app" "$arch"
