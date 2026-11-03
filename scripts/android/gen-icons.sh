#!/bin/bash
# Rasterize the transcript's tool and file icons (the iOS asset catalog's
# SVGs — one source for both apps) into PNG assets for the Android painter.
#
#   scripts/android/gen-icons.sh <assets_out_dir> <res_out_dir>
#
# The line icons (desktop set, tab and tool glyphs) become VectorDrawables
# (svg2vd.py) so they tint and scale like any Android icon; file-type icons
# (multi-color) are rasterized.
#
# Needs rsvg-convert (`brew install librsvg`); without it the painter falls
# back to Material symbols.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$1"
RES="$2"
mkdir -p "$OUT" "$RES"
python3 "$ROOT/scripts/android/svg2vd.py" "$RES"
if ! command -v rsvg-convert >/dev/null; then
  echo "note: rsvg-convert not found — skipped icon rasterization"
  exit 0
fi
for svg in "$ROOT"/apps/ios/Zeron/Assets.xcassets/{ToolIcons,FileIcons}/*.imageset/*.svg; do
  name="$(basename "$(dirname "$svg")" .imageset)"
  png="$OUT/$name.png"
  [[ "$png" -nt "$svg" ]] && continue
  rsvg-convert -w 72 -h 72 --keep-aspect-ratio "$svg" -o "$png"
done
