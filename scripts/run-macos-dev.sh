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
DAWN="$CONTENTS/MacOS/libwebgpu_dawn.dylib"
if [[ -f "$TARGET_DIR/debug/libwebgpu_dawn.dylib" ]]; then
  install -m 755 "$TARGET_DIR/debug/libwebgpu_dawn.dylib" "$DAWN"
fi
sed "s/__VERSION__/$VERSION/g" "$ROOT/dist/macos/Info-dev.plist" >"$CONTENTS/Info.plist"
plutil -replace CFBundleIdentifier -string "$BUNDLE_ID" "$CONTENTS/Info.plist"
plutil -replace LSEnvironment.ZERON_DATA_DIR -string "$DATA_DIR" "$CONTENTS/Info.plist"
plutil -replace LSEnvironment.ZERON_IPC_PORT -string "$IPC_PORT" "$CONTENTS/Info.plist"
if [[ -n "${CODEX_EXECUTABLE:-}" ]]; then
  plutil -insert LSEnvironment.CODEX_EXECUTABLE -string "$CODEX_EXECUTABLE" "$CONTENTS/Info.plist"
fi

mkdir -p "$CONTENTS/Resources/licenses"
cp "$ROOT/crates/voice/NOTICE.md" "$CONTENTS/Resources/licenses"/parakeet-v3.txt

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

# A real Apple Development identity gives TCC a stable signing requirement
# across rebuilds. Set ZERON_DEV_CODESIGN_IDENTITY explicitly when more than
# one identity is installed; otherwise fall back to an ad-hoc signature.
IDENTITY="${ZERON_DEV_CODESIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development:[^"]*\)".*/\1/p' | head -1)"
fi
if [[ -z "$IDENTITY" ]]; then
  echo "warning: no Apple Development signing identity found; macOS may ask for permissions again after a rebuild" >&2
fi
if [[ -f "$DAWN" ]]; then
  codesign --force --sign "${IDENTITY:--}" "$DAWN"
fi
codesign --entitlements "$ROOT/dist/macos/Dictation.entitlements" --force --sign "${IDENTITY:--}" --identifier "$BUNDLE_ID" "$APP"

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
