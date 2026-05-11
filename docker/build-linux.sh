#!/usr/bin/env bash
set -euo pipefail

cd /src/ui
npm install
npm run build

cd /src/core
cargo tauri build --bundles appimage deb

VERSION=$(grep '^version' /src/core/Cargo.toml | head -1 | cut -d'"' -f2)
OUT=/src/release-artifacts
mkdir -p "$OUT"

find /src/target -path '*/bundle/appimage/*.AppImage' -exec cp {} "$OUT/" \;
find /src/target -path '*/bundle/deb/*.deb' -exec cp {} "$OUT/" \;

BIN=$(find /src/target -maxdepth 3 -type f -name gipny -executable | head -1)
if [[ -n "$BIN" ]]; then
    TGZ=$(mktemp -d)
    mkdir -p "$TGZ/gipny-${VERSION}"
    cp "$BIN" "$TGZ/gipny-${VERSION}/"
    cp -r /src/core/icons "$TGZ/gipny-${VERSION}/"

    cat > "$TGZ/gipny-${VERSION}/gipny.desktop" <<EOF
[Desktop Entry]
Name=gipny
Comment=Tor-routed E2E encrypted messenger
Exec=gipny
Type=Application
Icon=gipny
Categories=Network;InstantMessaging;
Terminal=false
EOF

    cat > "$TGZ/gipny-${VERSION}/install.sh" <<'EOF'
#!/usr/bin/env bash
set -e
PREFIX="${PREFIX:-$HOME/.local}"
DIR="$(cd "$(dirname "$0")" && pwd)"
install -Dm755 "$DIR/gipny" "$PREFIX/bin/gipny"
install -Dm644 "$DIR/icons/icon.png" "$PREFIX/share/icons/hicolor/256x256/apps/gipny.png" 2>/dev/null || true
install -Dm644 "$DIR/gipny.desktop" "$PREFIX/share/applications/gipny.desktop"
echo "installed: $PREFIX/bin/gipny"
EOF
    chmod +x "$TGZ/gipny-${VERSION}/install.sh"

    tar -czf "$OUT/gipny-${VERSION}-x86_64.tar.gz" -C "$TGZ" "gipny-${VERSION}"
    rm -rf "$TGZ"
else
    echo "[!] gipny binary not found, skipping tar.gz"
fi

echo
echo "== linux artifacts =="
ls -lh "$OUT/"