#!/usr/bin/env bash
# Zeron.app's beta: the zgx interface (apps/zgx, built with its `beta`
# feature) as the Zeron app itself — the same bundle id, name, signing,
# entitlements and `zeron://` scheme as scripts/package-macos.sh's — with
# the stable release's own engine inside, so a tester keeps Zeron.app's
# engine, data and device:
#
#   target/package-beta/Zeron.app
#     Contents/MacOS/zeron           the zgx interface and `zeron`'s command
#                                    line (crates/cli; the engine's commands
#                                    run in the helper beside it)
#     Contents/MacOS/Zeron Engine    the stable release's engine, as signed
#     Contents/Resources/ui, plugins-builtin, zeron.icns, licenses
#   target/package-beta/zeron-<beta>-macos-<arch>.dmg
#   target/package-beta/zeron-<beta>-macos-<arch>-app.tar.gz  (the updater's)
#
# Usage: BETA_VERSION=0.2.136-beta.1 ZGX_BETA_ENGINE=<engine> scripts/package-zgx-beta.sh
#   BETA_VERSION        `<workspace version>-beta.<n>`; the workspace version
#                       is the next release's (newer than the stable whose
#                       engine goes in)
#   ZGX_BETA_ENGINE     the engine: a stable Zeron.app, its app tarball, or
#                       its `Zeron Engine` binary
#   ZGX_BETA_ZGX_BIN    the interface, built already (with --features beta and
#                       ZERON_BETA_VERSION); otherwise built here
#   ZGX_TARGET_DIR      where to build it (apps/zgx/target)
#   ZGX_BETA_DMG=0      no dmg
#   CARGO_FLAGS         extra cargo flags (CI: --locked)
# Signing and notarization are scripts/package-macos.sh's: CODESIGN_IDENTITY,
# NOTARY_KEY_PATH + NOTARY_KEY_ID + NOTARY_ISSUER_ID.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
die() { echo "$*" >&2; exit 1; }

WORKSPACE="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
BETA_VERSION="${BETA_VERSION:-}"
[[ "$BETA_VERSION" =~ ^([0-9]+\.[0-9]+\.[0-9]+)-beta\.([0-9]+)$ ]] ||
  die "BETA_VERSION must be <x.y.z>-beta.<n>, not '$BETA_VERSION'"
CORE="${BASH_REMATCH[1]}"
NUMBER="${BASH_REMATCH[2]}"
[[ "$CORE" == "$WORKSPACE" ]] || die "BETA_VERSION $BETA_VERSION isn't a beta of the workspace's $WORKSPACE"
ARCH="$(uname -m)"
OUT_DIR="$ROOT/target/package-beta"
APP="$OUT_DIR/Zeron.app"
DMG="$OUT_DIR/zeron-$BETA_VERSION-macos-$ARCH.dmg"
APP_TARBALL="$OUT_DIR/zeron-$BETA_VERSION-macos-$ARCH-app.tar.gz"
ENTITLEMENTS="$ROOT/dist/macos/Zeron.entitlements"
read -r -a CARGO_EXTRA <<<"${CARGO_FLAGS:-}"
mkdir -p "$OUT_DIR"

# The engine: the stable release's, from its bundle, tarball or binary.
ENGINE_SRC="${ZGX_BETA_ENGINE:-}"
[[ -n "$ENGINE_SRC" ]] || die "ZGX_BETA_ENGINE: a stable Zeron.app, its app tarball or its Zeron Engine"
ENGINE_BIN="$ENGINE_SRC"
if [[ "$ENGINE_SRC" == *.tar.gz ]]; then
  UNPACK="$OUT_DIR/.stable"
  rm -rf "$UNPACK" && mkdir -p "$UNPACK"
  tar -xzf "$ENGINE_SRC" -C "$UNPACK"
  ENGINE_SRC="$UNPACK/Zeron.app"
fi
[[ -d "$ENGINE_SRC" ]] && ENGINE_BIN="$ENGINE_SRC/Contents/MacOS/Zeron Engine"
[[ -x "$ENGINE_BIN" ]] || die "no engine at $ENGINE_BIN"
ENGINE_VERSION="$("$ENGINE_BIN" --version | sed -n 's/^zeron-engine //p')"
[[ "$ENGINE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$ENGINE_BIN didn't say its version"
# The interface hands `zeron headless`, `zeron login` and the engine's other
# subcommands to this engine, which must parse them with the same command
# line (crates/cli). An engine from before that runs the background engine
# for any argument instead. (Read, not run: running an old one starts it.)
LC_ALL=C grep -aq "Multi-device controller for coding agents" "$ENGINE_BIN" ||
  die "the $ENGINE_VERSION engine predates the shared command line: the beta needs a stable release with it"

# The interface: apps/zgx is a workspace of its own (its own lock, patches).
ZGX_BIN="${ZGX_BETA_ZGX_BIN:-}"
if [[ -z "$ZGX_BIN" ]]; then
  target_dir="${ZGX_TARGET_DIR:-$ROOT/apps/zgx/target}"
  ZERON_BETA_VERSION="$BETA_VERSION" cargo build --release ${CARGO_EXTRA[@]+"${CARGO_EXTRA[@]}"} \
    --features beta --manifest-path "$ROOT/apps/zgx/Cargo.toml" --target-dir "$target_dir"
  ZGX_BIN="$target_dir/release/zeron-zgx"
fi
# Only a beta build of this version may go in: the updater stages it by
# `zeron --version`, and a development build is staging's.
[[ "$("$ZGX_BIN" --version)" == "zeron $BETA_VERSION" ]] ||
  die "$ZGX_BIN isn't the $BETA_VERSION beta (it says '$("$ZGX_BIN" --version)')"
# Production's only: neither binary may carry staging's edge or sign-in.
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
install -m 755 "$ZGX_BIN" "$APP/Contents/MacOS/zeron"
install -m 755 "$ENGINE_BIN" "$APP/Contents/MacOS/Zeron Engine"
# Zeron.app's Info.plist, as the stable app's, with the beta's versions: the
# short version stays numeric (the next release's), the bundle version is
# its build, the engine its own (crates/update `bundle_engine_version`), and
# zgx's floor macOS 13 (Ghostty's).
PLIST="$APP/Contents/Info.plist"
sed "s/__VERSION__/$CORE/" "$ROOT/dist/macos/Info.plist" >"$PLIST"
plutil -replace CFBundleVersion -string "$CORE.$NUMBER" "$PLIST"
plutil -replace ZeronEngineVersion -string "$ENGINE_VERSION" "$PLIST"
plutil -replace ZeronVersion -string "$BETA_VERSION" "$PLIST"
plutil -replace LSMinimumSystemVersion -string "13.0" "$PLIST"
plutil -convert xml1 "$PLIST"
plutil -lint "$PLIST" >/dev/null

# The scripts and Zeron's own plugins (read where they lie: templates and
# plugins live in the data dir). Not zgx's generated types or the tests.
rsync -a --exclude '.zgx/' --exclude '/tests/' --exclude '.lock' --exclude '.DS_Store' \
  "$ROOT/apps/zgx/ui/" "$APP/Contents/Resources/ui/"
rsync -a --exclude '.zgx/' --exclude '.DS_Store' \
  "$ROOT/apps/zgx/plugins-builtin/" "$APP/Contents/Resources/plugins-builtin/"

mkdir -p "$APP/Contents/Resources/licenses/fonts"
cp "$ROOT/crates/ui/assets/fonts/licenses/"* "$ROOT/apps/zgx/fonts/"*-OFL.txt "$APP/Contents/Resources/licenses/fonts/"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$ROOT/LICENSE" "$APP/Contents/Resources/licenses/"
cp "$ROOT/crates/voice/NOTICE.md" "$APP/Contents/Resources/licenses/parakeet-v3.txt"

# Zeron's icon, as scripts/package-macos.sh makes it.
ICONSET="$OUT_DIR/zeron.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$ROOT/dist/macos/icon-1024.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" "$ROOT/dist/macos/icon-1024.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/zeron.icns"
rm -rf "$ICONSET"

# Signed as scripts/package-macos.sh signs Zeron.app: the engine (its
# identifier), then the bundle.
if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
  codesign --entitlements "$ENTITLEMENTS" --force --options runtime --timestamp --sign "$CODESIGN_IDENTITY" --identifier sh.zeron.app.engine "$APP/Contents/MacOS/Zeron Engine"
  codesign --entitlements "$ENTITLEMENTS" --force --options runtime --timestamp --sign "$CODESIGN_IDENTITY" "$APP"
else
  codesign --force --entitlements "$ENTITLEMENTS" --sign - --identifier sh.zeron.app.engine "$APP/Contents/MacOS/Zeron Engine"
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
  ZIP="$OUT_DIR/zeron-notarize.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  notarize "$ZIP"
  rm -f "$ZIP"
  xcrun stapler staple "$APP"
fi

# The updater's artifact (crates/update `stage_mac_app` unpacks Zeron.app).
tar -czf "$APP_TARBALL" -C "$OUT_DIR" Zeron.app
echo "packaged: $APP_TARBALL"
echo "size: $(du -sh "$APP" | cut -f1) ($(du -sk "$APP" | cut -f1) KB) · interface $(du -h "$APP/Contents/MacOS/zeron" | cut -f1) · engine $(du -h "$APP/Contents/MacOS/Zeron Engine" | cut -f1)"

if [[ "${ZGX_BETA_DMG:-1}" == "0" ]]; then
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
    volume_name="Zeron Beta",
    settings={
        "format": "UDZO",
        "files": [app],
        "symlinks": {"Applications": "/Applications"},
        "icon": os.path.join(app, "Contents/Resources/zeron.icns"),
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
        "icon_locations": {"Zeron.app": (165, 195), "Applications": (495, 195)},
    },
)
PY
rm -f "$BG_TIFF"
if $NOTARIZE; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
fi
echo "packaged: $DMG"
