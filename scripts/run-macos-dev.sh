#!/usr/bin/env bash
# Build and run an isolated macOS development bundle. Zeron.app may remain
# open: this bundle has a separate LaunchServices/TCC identity, data directory,
# and engine IPC port.
# Set ZERON_DEV_BUILD_ONLY=1 to prepare the signed bundle without launching it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
DEV_ROOT="$ROOT/target/macos-dev"
APP="$DEV_ROOT/Zeron Dev.app"
CONTENTS="$APP/Contents"
DATA_DIR="${ZERON_DEV_DATA_DIR:-$DEV_ROOT/data}"
IPC_PORT="${ZERON_DEV_IPC_PORT:-49777}"
BUNDLE_ID="${ZERON_DEV_BUNDLE_ID:-sh.zeron.app.dev}"

if pgrep -f -x "$CONTENTS/MacOS/zeron" >/dev/null 2>&1; then
  echo "Zeron Dev is already running. Quit it before rebuilding the signed bundle." >&2
  exit 1
fi

cd "$ROOT"
cargo build -p zeron

mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources" "$DATA_DIR"
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
install -m 755 "$TARGET_DIR/debug/zeron" "$CONTENTS/MacOS/zeron"
sed "s/__VERSION__/$VERSION/g" "$ROOT/dist/macos/Info-dev.plist" >"$CONTENTS/Info.plist"
plutil -replace CFBundleIdentifier -string "$BUNDLE_ID" "$CONTENTS/Info.plist"
plutil -replace LSEnvironment.ZERON_DATA_DIR -string "$DATA_DIR" "$CONTENTS/Info.plist"
plutil -replace LSEnvironment.ZERON_IPC_PORT -string "$IPC_PORT" "$CONTENTS/Info.plist"
if [[ -n "${CODEX_EXECUTABLE:-}" ]]; then
  plutil -insert LSEnvironment.CODEX_EXECUTABLE -string "$CODEX_EXECUTABLE" "$CONTENTS/Info.plist"
fi

mkdir -p "$CONTENTS/Resources/licenses"
cp "$ROOT/crates/voice/NOTICE.md" "$CONTENTS/Resources/licenses"/parakeet-v3.txt

# A stable identity gives TCC a stable signing requirement across rebuilds:
# ZERON_DEV_CODESIGN_IDENTITY, else an Apple Development identity, else the
# self-signed "Zeron Local Signing" (scripts/provision-local-signing.sh; it is
# untrusted, so `find-identity -v` hides it), else ad-hoc.
IDENTITY="${ZERON_DEV_CODESIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development:[^"]*\)".*/\1/p' | head -1)"
fi
if [[ -z "$IDENTITY" ]] && security find-identity -p codesigning 2>/dev/null | grep -q '"Zeron Local Signing"'; then
  IDENTITY="Zeron Local Signing"
fi

# Zeron computer use: bundle the cua-driver runtime (staged by
# scripts/provision-cua-driver.sh). The runtime is the whole CuaDriver.app
# bundle: upstream refuses to run a bare copied Mach-O, because the daemon
# needs its own bundle identity for a WindowServer connection. The bundle stays
# Developer ID signed and is left untouched here; only the theme artifact is
# added beside it.
if [[ -x "$ROOT/vendor/cua-driver/CuaDriver.app/Contents/MacOS/cua-driver" ]]; then
  mkdir -p "$CONTENTS/Resources/cua-driver"
  rm -rf "$CONTENTS/Resources/cua-driver/CuaDriver.app"
  ditto "$ROOT/vendor/cua-driver/CuaDriver.app" "$CONTENTS/Resources/cua-driver/CuaDriver.app"
  # Zeron's agent cursor theme, staged by provision-cua-driver.sh. The driver
  # scans this directory and needs the filename to match the theme id.
  if [[ -d "$ROOT/vendor/cua-driver/cursor-themes" ]]; then
    rm -rf "$CONTENTS/Resources/cua-driver/cursor-themes"
    cp -R "$ROOT/vendor/cua-driver/cursor-themes" "$CONTENTS/Resources/cua-driver/cursor-themes"
  fi
else
  echo "warning: vendor/cua-driver missing; run scripts/provision-cua-driver.sh for computer use" >&2
fi

# Computer-use helpers: capture indicator (purple pill) and Escape kill switch.
for HELPER in zeron-window-indicator zeron-escape-monitor; do
  install -m 755 "$("$ROOT/scripts/build-window-indicator.sh" "$HELPER")" "$CONTENTS/Resources/$HELPER"
  codesign --force --sign "${IDENTITY:--}" "$CONTENTS/Resources/$HELPER"
done

if [[ ! -f "$CONTENTS/Resources/zeron.icns" ]]; then
  ICONSET="$DEV_ROOT/zeron-dev.iconset"
  mkdir -p "$ICONSET"
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$ROOT/dist/macos/icon-1024.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    retina=$((size * 2))
    sips -z "$retina" "$retina" "$ROOT/dist/macos/icon-1024.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$CONTENTS/Resources/zeron.icns"
fi

if [[ -n "$IDENTITY" ]]; then
  codesign --entitlements "$ROOT/dist/macos/Zeron.entitlements" --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" "$APP"
else
  codesign --entitlements "$ROOT/dist/macos/Zeron.entitlements" --force --sign - --identifier "$BUNDLE_ID" "$APP"
  echo "warning: no stable signing identity found; macOS may ask for permissions again after a rebuild (run scripts/provision-local-signing.sh)" >&2
fi

if [[ "${ZERON_DEV_BUILD_ONLY:-0}" == "1" ]]; then
  echo "built: $APP" >&2
  exit 0
fi

echo "running Zeron Dev (bundle $BUNDLE_ID, data $DATA_DIR, IPC $IPC_PORT)" >&2
# LaunchServices must own the process. Launching Contents/MacOS/zeron directly
# makes TCC attribute Screen Recording to the terminal (Warp, Terminal, etc.).
# -W keeps the script attached until the app exits. Runtime logs remain in the
# isolated data directory (`target/macos-dev/data/logs/zeron-headed.log`).
OPEN_ENV=(
  --env "ZERON_AUTO_UPDATE=0"
  --env "ZERON_DATA_DIR=$DATA_DIR"
  --env "ZERON_IPC_PORT=$IPC_PORT"
)
if [[ -n "${CODEX_EXECUTABLE:-}" ]]; then
  OPEN_ENV+=(--env "CODEX_EXECUTABLE=$CODEX_EXECUTABLE")
fi
if [[ -n "${ZERON_OPEN_ROUTE:-}" ]]; then
  OPEN_ENV+=(--env "ZERON_OPEN_ROUTE=$ZERON_OPEN_ROUTE")
fi
exec open -W "${OPEN_ENV[@]}" "$APP" --args "$@"
