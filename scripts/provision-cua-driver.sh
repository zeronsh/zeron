#!/usr/bin/env bash
# Zeron computer use: stage the cua-driver runtime into vendor/cua-driver/.
#
# Sources, in order: $CUA_DRIVER_SOURCE (a CuaDriver.app bundle, or a directory
# holding one), then CuaDriver.app in /Applications or ~/Applications. The
# packaging scripts copy vendor/cua-driver/ into
# Zeron.app/Contents/Resources/cua-driver/, where the bridge finds it.
#
# The runtime is staged as the whole CuaDriver.app bundle, not a bare binary:
# upstream builds since 0.34 refuse to run outside the bundle (the daemon needs
# its bundle identity for a WindowServer connection, so a copied Mach-O is
# killed on launch). vendor/cua-driver/aurora.cua-theme is Zeron's own agent
# cursor theme, staged beside the runtime under cursor-themes/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/vendor/cua-driver"

source="${CUA_DRIVER_SOURCE:-}"
if [[ -z "$source" ]]; then
  for candidate in \
    "/Applications/CuaDriver.app" \
    "$HOME/Applications/CuaDriver.app"; do
    if [[ -d "$candidate" ]]; then
      source="$candidate"
      break
    fi
  done
fi
if [[ -z "$source" ]]; then
  echo "CuaDriver.app not found. Install it (https://cua.ai/driver/install.sh) or set CUA_DRIVER_SOURCE=/path/to/CuaDriver.app" >&2
  exit 1
fi

# Accept either the bundle itself or a directory that contains it.
if [[ -d "$source/CuaDriver.app" ]]; then
  source="$source/CuaDriver.app"
fi
bundle_binary="$source/Contents/MacOS/cua-driver"
[[ -x "$bundle_binary" ]] || { echo "not a CuaDriver.app bundle: $source" >&2; exit 1; }

mkdir -p "$DEST"
rm -rf "$DEST/CuaDriver.app"
ditto "$source" "$DEST/CuaDriver.app"

# The version comes from the bundle's Info.plist, not from running the binary:
# a bare Mach-O copy of these builds is killed on launch (exit 137), and this
# script only ever stages it through the bundle path.
plist="$DEST/CuaDriver.app/Contents/Info.plist"
version="$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$plist" 2>/dev/null || echo unknown)"

# Zeron's agent cursor theme, next to the runtime. The driver scans this
# directory (CUA_DRIVER_CURSOR_THEME_DIR) and requires the filename to match the
# theme id compiled into the artifact.
theme="$ROOT/assets/cursor-themes/aurora.cua-theme"
theme_id="sh.zeron.cursor.aurora"
if [[ -f "$theme" ]]; then
  mkdir -p "$DEST/cursor-themes"
  install -m 644 "$theme" "$DEST/cursor-themes/$theme_id.cua-theme"
fi

# The bundle carries no licence of its own, so take it from the pinned release
# tag. provenance.json is Zeron's own record of what was vendored (hashes,
# version, patch state): it is hand-written and never staged from a source.
tag="cua-driver-rs-v${version}"
if [[ ! -f "$DEST/LICENSE.md" || "${CUA_DRIVER_REFRESH_LICENSE:-}" == "1" ]]; then
  curl -fsSL "https://raw.githubusercontent.com/trycua/cua/$tag/LICENSE.md" \
    -o "$DEST/LICENSE.md" 2>/dev/null \
    || echo "warning: could not fetch LICENSE.md for $tag" >&2
fi

echo "staged cua-driver $version -> $DEST" >&2
echo "hand-written provenance.json is kept; update its hashes when the runtime changes:" >&2
shasum -a 256 "$DEST/CuaDriver.app/Contents/MacOS/cua-driver" "$DEST/CuaDriver.app/Contents/MacOS/cua-cursor-theme" 2>/dev/null | sed "s|$DEST/||" >&2
