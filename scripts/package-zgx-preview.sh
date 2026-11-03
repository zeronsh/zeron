#!/usr/bin/env bash
# Zeron Preview packaging: the zgx app (apps/zgx, built with its `preview`
# feature) and the engine built from this checkout, in a bundle of their own
# that installs beside Zeron.app and never replaces it:
#
#   target/package-preview/Zeron Preview.app
#     Contents/MacOS/zeron-zgx         the app (CFBundleExecutable)
#     Contents/MacOS/zeron-engine      its engine (src/engine/manager.rs
#                                      starts it: port 27664, ~/.zeron-preview)
#     Contents/Resources/ui            the app's scripts (src/main.rs ui_dir)
#     Contents/Resources/plugins-builtin
#   target/package-preview/zeron-preview-<version>-macos-<arch>.dmg
#   target/package-preview/zeron-preview-<version>-macos-<arch>-app.tar.gz
#
# <version> is the workspace's (Cargo.toml) and the preview's number:
# PREVIEW_NUMBER=3 → 0.2.132-3, bundle version 0.2.132.3. The bundle's short
# version stays the workspace's, which its engine compares with its own.
#
# Signing and notarization are scripts/package-macos.sh's:
#   CODESIGN_IDENTITY="Developer ID Application: …"  sign (else ad-hoc)
#   NOTARY_KEY_PATH + NOTARY_KEY_ID + NOTARY_ISSUER_ID  notarize + staple
# Local checks:
#   ZGX_PREVIEW_ENGINE_BIN=<path>  bundle this engine instead of building one
#   ZGX_PREVIEW_ZGX_BIN=<path>     bundle this app binary (a `preview` build)
#   ZGX_TARGET_DIR=<dir>           build apps/zgx into this target dir
#   ZGX_PREVIEW_DMG=0              no dmg (dmgbuild not installed)
#   CARGO_FLAGS="--locked"         extra cargo flags (CI)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
PREVIEW_NUMBER="${PREVIEW_NUMBER:-0}"
[[ "$PREVIEW_NUMBER" =~ ^[0-9]+$ ]] || { echo "PREVIEW_NUMBER must be a number, not '$PREVIEW_NUMBER'" >&2; exit 1; }
PREVIEW_VERSION="$VERSION-$PREVIEW_NUMBER"
BUILD="$VERSION.$PREVIEW_NUMBER"
ARCH="$(uname -m)"
OUT_DIR="$ROOT/target/package-preview"
APP="$OUT_DIR/Zeron Preview.app"
NAME="zeron-preview-$PREVIEW_VERSION-macos-$ARCH"
DMG="$OUT_DIR/$NAME.dmg"
APP_TARBALL="$OUT_DIR/$NAME-app.tar.gz"
ENTITLEMENTS="$ROOT/dist/macos/Zeron.entitlements"
read -r -a CARGO_EXTRA <<<"${CARGO_FLAGS:-}"

cd "$ROOT"

# The engine: this checkout's, as Zeron.app's (scripts/package-macos.sh).
ENGINE_BIN="${ZGX_PREVIEW_ENGINE_BIN:-}"
if [[ -z "$ENGINE_BIN" ]]; then
  cargo build --release ${CARGO_EXTRA[@]+"${CARGO_EXTRA[@]}"} -p zeron --bin zeron-engine
  ENGINE_BIN="$ROOT/target/release/zeron-engine"
fi

# The app: apps/zgx is a workspace of its own (its own lock and patches).
ZGX_BIN="${ZGX_PREVIEW_ZGX_BIN:-}"
if [[ -z "$ZGX_BIN" ]]; then
  target_dir="${ZGX_TARGET_DIR:-$ROOT/apps/zgx/target}"
  cargo build --release ${CARGO_EXTRA[@]+"${CARGO_EXTRA[@]}"} --features preview \
    --manifest-path "$ROOT/apps/zgx/Cargo.toml" --target-dir "$target_dir"
  ZGX_BIN="$target_dir/release/zeron-zgx"
fi
# Only a `preview` build may go in: anything else is staging's app.
LC_ALL=C grep -q 'releases/preview/manifest.json' "$ZGX_BIN" ||
  { echo "$ZGX_BIN isn't built with --features preview" >&2; exit 1; }
# Zeron Preview is production's only: neither binary may carry staging's
# edge or sign-in client (src/auth.rs builds none into a preview build).
STAGING_MARKS='staging\.zeron\.sh|zeron-edge-staging|client_01KWD0E9SYPV2WE4QN9TP9DP0W'
for bin in "$ZGX_BIN" "$ENGINE_BIN"; do
  if LC_ALL=C grep -aqE "$STAGING_MARKS" "$bin"; then
    echo "$bin carries staging's edge or sign-in client:" >&2
    LC_ALL=C grep -aoE "$STAGING_MARKS" "$bin" | sort -u >&2
    exit 1
  fi
done

rm -rf "$APP" "$DMG" "$APP_TARBALL"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 755 "$ZGX_BIN" "$APP/Contents/MacOS/zeron-zgx"
install -m 755 "$ENGINE_BIN" "$APP/Contents/MacOS/zeron-engine"
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" \
  "$ROOT/dist/macos/Info-preview.plist" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# The scripts and Zeron's own plugins, read where they lie (never written:
# templates and plugins live in the engine's data dir). Not the type stubs
# zgx generates (.zgx/) or the scripts' tests.
rsync -a --exclude '.zgx/' --exclude '/tests/' --exclude '.lock' --exclude '.DS_Store' \
  "$ROOT/apps/zgx/ui/" "$APP/Contents/Resources/ui/"
rsync -a --exclude '.zgx/' --exclude '.DS_Store' \
  "$ROOT/apps/zgx/plugins-builtin/" "$APP/Contents/Resources/plugins-builtin/"

mkdir -p "$APP/Contents/Resources/licenses/fonts"
cp "$ROOT/apps/zgx/fonts/"*-OFL.txt "$APP/Contents/Resources/licenses/fonts/"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$ROOT/LICENSE" "$APP/Contents/Resources/licenses/"
cp "$ROOT/crates/voice/NOTICE.md" "$APP/Contents/Resources/licenses/parakeet-v3.txt"

# Icon: Zeron's with the PREVIEW band (scripts/preview-icon.py renders it).
ICONSET="$OUT_DIR/zeron-preview.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$ROOT/dist/macos/icon-preview-1024.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" "$ROOT/dist/macos/icon-preview-1024.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/zeron-preview.icns"
rm -rf "$ICONSET"

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
  # Hardened runtime + secure timestamp (notarization), inside out: the
  # engine, then the bundle that seals it and the app's executable.
  codesign --entitlements "$ENTITLEMENTS" --force --options runtime --timestamp --sign "$CODESIGN_IDENTITY" --identifier sh.zeron.preview.engine "$APP/Contents/MacOS/zeron-engine"
  codesign --entitlements "$ENTITLEMENTS" --force --options runtime --timestamp --sign "$CODESIGN_IDENTITY" "$APP"
else
  codesign --force --entitlements "$ENTITLEMENTS" --sign - --identifier sh.zeron.preview.engine "$APP/Contents/MacOS/zeron-engine"
  codesign --force --entitlements "$ENTITLEMENTS" --sign - "$APP"
fi
codesign --verify --strict --verbose=2 "$APP"

notarize() {
  xcrun notarytool submit "$1" \
    --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" \
    --issuer "$NOTARY_ISSUER_ID" --wait
}
NOTARIZE=false
[[ -n "${NOTARY_KEY_PATH:-}" && -n "${NOTARY_KEY_ID:-}" && -n "${NOTARY_ISSUER_ID:-}" ]] && NOTARIZE=true

if $NOTARIZE; then
  ZIP="$OUT_DIR/zeron-preview-notarize.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  notarize "$ZIP"
  rm -f "$ZIP"
  xcrun stapler staple "$APP"
fi

# For a later self-update (the stapled bundle, as Zeron.app's tarball).
tar -czf "$APP_TARBALL" -C "$OUT_DIR" "Zeron Preview.app"
echo "packaged: $APP_TARBALL"

if [[ "${ZGX_PREVIEW_DMG:-1}" == "0" ]]; then
  echo "packaged: $APP (no dmg)"
  exit 0
fi

python3 -c 'import dmgbuild' 2>/dev/null ||
  python3 -m pip install --quiet --user dmgbuild 2>/dev/null ||
  python3 -m pip install --quiet --user --break-system-packages dmgbuild

BG_TIFF="$OUT_DIR/dmg-background.tiff"
tiffutil -cathidpicheck "$ROOT/dist/macos/dmg-background.png" \
  "$ROOT/dist/macos/dmg-background@2x.png" -out "$BG_TIFF" >/dev/null 2>&1

APP="$APP" DMG="$DMG" BG_TIFF="$BG_TIFF" python3 - <<'PY'
import os
import dmgbuild

app = os.environ["APP"]
dmgbuild.build_dmg(
    filename=os.environ["DMG"],
    volume_name="Zeron Preview",
    settings={
        "format": "UDZO",
        "files": [app],
        "symlinks": {"Applications": "/Applications"},
        "icon": os.path.join(app, "Contents/Resources/zeron-preview.icns"),
        "background": os.environ["BG_TIFF"],
        "show_status_bar": False,
        "show_tab_view": False,
        "show_toolbar": False,
        "show_pathbar": False,
        "show_sidebar": False,
        "default_view": "icon-view",
        # Window and icon geometry as scripts/package-macos.sh's (the same art).
        "window_rect": ((200, 120), (660, 400)),
        "icon_size": 104,
        "text_size": 12,
        "icon_locations": {"Zeron Preview.app": (165, 195), "Applications": (495, 195)},
    },
)
PY
rm -f "$BG_TIFF"
if $NOTARIZE; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
fi
echo "packaged: $DMG"
