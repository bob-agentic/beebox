#!/usr/bin/env bash
# Renders the app icon. `iconutil` and `sips` ship with macOS, so this needs no
# Xcode and no third-party tools.
set -euo pipefail
OUT=${1:?usage: make-icns.sh <output.icns>}
# Resolved before the cd below, so a relative path stays relative to where the
# caller stood rather than to this script.
OUT=$(cd "$(dirname "$OUT")" 2>/dev/null && printf '%s/%s' "$PWD" "$(basename "$OUT")" \
      || printf '%s' "$OUT")
cd "$(dirname "$0")/.."

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Drawn fresh each time: the checked-in PNG has been downsampled in the past,
# and an icns built from it is blurry at the largest sizes.
python3 icons/make-icon.py
SRC=icons/icon.png

mkdir -p "$WORK/AppIcon.iconset"
for size in 16 32 64 128 256 512; do
  sips -z $size $size "$SRC" --out "$WORK/AppIcon.iconset/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$SRC" \
       --out "$WORK/AppIcon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
mkdir -p "$(dirname "$OUT")"
iconutil -c icns "$WORK/AppIcon.iconset" -o "$OUT"
