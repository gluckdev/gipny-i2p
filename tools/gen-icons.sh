#!/bin/bash
# Rasterise every app icon from core/icons/icon.svg.
#
# Run this after editing the SVG, then commit the PNGs — they are checked in
# because both the Tauri bundler and the Android resource system want real files
# at build time, and neither rasterises SVG for us.
#
# Renderer is headless Chrome rather than ImageMagick: magick has no rsvg
# delegate here and falls back to its own SVG parser, which silently drops
# filters and gradients — the first attempt at this produced a tile with the
# mark missing entirely. Chrome is a real renderer and is already on the box.
set -euo pipefail

cd "$(dirname "$0")/.."
SVG="core/icons/icon.svg"
FG="core/icons/icon-foreground.svg"
CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || true)}"
[ -n "$CHROME" ] || { echo "need google-chrome or chromium to rasterise" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Rasterise each SVG once, large, then resample down. Rendering straight to the
# target size does not work: the SVGs declare width/height 512, so Chrome draws
# them at 512 regardless and a small --window-size simply crops the top-left
# corner — the first run produced a 32px icon that was a fragment of the tile.
# Downscaling from one high-resolution master is also what you want anyway, since
# magick's resampling beats re-rendering vector art at 32px.
master() { # master <svg> <out.png>
  "$CHROME" --headless --disable-gpu --no-sandbox \
    --force-device-scale-factor=2 \
    --default-background-color=00000000 \
    --window-size=512,512 \
    --screenshot="$2" \
    "file://$PWD/$1" >/dev/null 2>&1
  magick "$2" -background none -gravity northwest -crop 1024x1024+0+0 +repage "$2"
}

render() { # render <master.png> <size> <out>
  magick "$1" -background none -filter Lanczos -resize "$2x$2" -strip "$3"
  echo "  $3 (${2}px)"
}

master "$SVG" "$TMP/master.png"
master "$FG"  "$TMP/master-fg.png"
SVG="$TMP/master.png"
FG="$TMP/master-fg.png"

echo "desktop:"
render "$SVG" 32  core/icons/32x32.png
render "$SVG" 128 core/icons/128x128.png
render "$SVG" 256 core/icons/128x128@2x.png
render "$SVG" 512 core/icons/icon.png

# .ico carries several sizes; Windows picks per context.
for s in 16 24 32 48 64 128 256; do render "$SVG" "$s" "$TMP/ico-$s.png" >/dev/null; done
magick "$TMP"/ico-*.png core/icons/icon.ico
echo "  core/icons/icon.ico (16-256 multi-size)"

# Android. ic_launcher/ic_launcher_round are the full-tile icons Android 7 and
# older show; ic_launcher_foreground is the adaptive layer that
# mipmap-anydpi-v26/*.xml puts over drawable/ic_launcher_background on 8+. It is
# drawn on a 108dp canvas where only the middle 72dp is guaranteed visible, hence
# the separate SVG. Those XML files are hand-written, not generated here.
echo "android:"
res="core/gen/android/app/src/main/res"
for d in "mdpi 48 108" "hdpi 72 162" "xhdpi 96 216" "xxhdpi 144 324" "xxxhdpi 192 432"; do
  set -- $d
  dir="$res/mipmap-$1"
  mkdir -p "$dir"
  render "$SVG" "$2" "$dir/ic_launcher.png"
  # Round variant: same art, circular mask. Launchers that want a circle get one
  # that is actually circular rather than a rounded square they clip themselves.
  r=$(( $2 / 2 ))
  magick "$dir/ic_launcher.png" \
    \( -size "$2x$2" xc:none -fill white -draw "circle $r,$r $r,0" \) \
    -alpha set -compose CopyOpacity -composite "$dir/ic_launcher_round.png"
  echo "  $dir/ic_launcher_round.png (${2}px, circular)"
  render "$FG" "$3" "$dir/ic_launcher_foreground.png"
done

echo "done."
