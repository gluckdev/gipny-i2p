#!/usr/bin/env bash
set -euo pipefail

cd /src/ui
npm install
npm run build

cd /src/core
cargo build --release --target x86_64-pc-windows-gnu

VERSION=$(grep '^version' /src/core/Cargo.toml | head -1 | cut -d'"' -f2)
OUT=/src/release-artifacts
mkdir -p "$OUT"

BIN=/src/target/x86_64-pc-windows-gnu/release/gipny.exe
if [[ ! -f "$BIN" ]]; then
    echo "[!] gipny.exe not found at $BIN" >&2
    exit 1
fi

WV2_CACHE=/root/.cache/webview2
WV2_DLL="$WV2_CACHE/WebView2Loader.dll"
mkdir -p "$WV2_CACHE"
if [[ ! -f "$WV2_DLL" ]]; then
    echo "[*] downloading WebView2 SDK"
    wget -q -O "$WV2_CACHE/webview2.nupkg" "https://www.nuget.org/api/v2/package/Microsoft.Web.WebView2"
    unzip -o -q "$WV2_CACHE/webview2.nupkg" -d "$WV2_CACHE/extracted"
    cp "$WV2_CACHE/extracted/build/native/x64/WebView2Loader.dll" "$WV2_DLL"
fi

NSIS_PLUGINS_CACHE=/root/.cache/nsis-plugins
mkdir -p "$NSIS_PLUGINS_CACHE"
if [[ ! -f "$NSIS_PLUGINS_CACHE/Plugins/x86-unicode/INetC.dll" ]]; then
    echo "[*] downloading NSIS Inetc plugin"
    wget -q -O "$NSIS_PLUGINS_CACHE/inetc.zip" "https://nsis.sourceforge.io/mediawiki/images/c/c9/Inetc.zip"
    unzip -o -q "$NSIS_PLUGINS_CACHE/inetc.zip" -d "$NSIS_PLUGINS_CACHE"
fi
mkdir -p /usr/share/nsis/Plugins/x86-unicode /usr/share/nsis/Plugins/amd64-unicode
cp -f "$NSIS_PLUGINS_CACHE/Plugins/x86-unicode/INetC.dll" /usr/share/nsis/Plugins/x86-unicode/ 2>/dev/null \
    || cp -f "$NSIS_PLUGINS_CACHE/Plugins/amd64-unicode/INetC.dll" /usr/share/nsis/Plugins/x86-unicode/
cp -f "$NSIS_PLUGINS_CACHE/Plugins/amd64-unicode/INetC.dll" /usr/share/nsis/Plugins/amd64-unicode/ 2>/dev/null || true

STAGE=$(mktemp -d)
cp "$BIN" "$STAGE/gipny.exe"
cp "$WV2_DLL" "$STAGE/WebView2Loader.dll"
cp /src/core/icons/icon.ico "$STAGE/app.ico"

OUT_EXE="$OUT/gipny-${VERSION}-setup.exe"
makensis \
    -DVERSION="$VERSION" \
    -DEXE_PATH="$STAGE/gipny.exe" \
    -DDLL_PATH="$STAGE/WebView2Loader.dll" \
    -DICON_PATH="$STAGE/app.ico" \
    -DOUT_FILE="$OUT_EXE" \
    /usr/local/share/gipny/installer.nsi

ZIP_DIR=$(mktemp -d)
mkdir -p "$ZIP_DIR/gipny-${VERSION}"
cp "$BIN" "$ZIP_DIR/gipny-${VERSION}/gipny.exe"
cp "$WV2_DLL" "$ZIP_DIR/gipny-${VERSION}/WebView2Loader.dll"
cat > "$ZIP_DIR/gipny-${VERSION}/README.txt" <<EOF
gipny ${VERSION} — portable

run: double-click gipny.exe
requires WebView2 Runtime (preinstalled on Win10 20H2+ / Win11)
if missing, run the setup .exe installer instead
EOF
(cd "$ZIP_DIR" && zip -r "$OUT/gipny-${VERSION}-x86_64-portable.zip" "gipny-${VERSION}")

rm -rf "$STAGE" "$ZIP_DIR"

echo
echo "== windows artifacts =="
ls -lh "$OUT/"