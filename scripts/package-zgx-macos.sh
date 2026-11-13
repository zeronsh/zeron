#!/usr/bin/env bash
# Zeron.app, stable or beta: the zgx interface (apps/zgx, built with its
# `beta` feature, which makes it the Zeron app) as the Zeron app itself —
# the same bundle id, name, signing,
# entitlements and `zeron://` scheme as scripts/package-macos.sh's — with
# the engine it's given inside, built from the same commit, so a tester keeps
# Zeron.app's data and device:
#
#   target/package-zgx/Zeron.app
#     Contents/MacOS/zeron           the zgx interface and `zeron`'s command
#                                    line (crates/cli; the engine's commands
#                                    run in the helper beside it)
#     Contents/MacOS/Zeron Engine    the engine, signed here
#     Contents/Resources/ui, plugins-builtin, zeron.icns, licenses
#   target/package-zgx/zeron-<version>-macos-<arch>.dmg
#   target/package-zgx/zeron-<version>-macos-<arch>-app.tar.gz  (the updater's)
#
# Usage: ZGX_VERSION=0.2.139 ZGX_ENGINE=<engine> scripts/package-zgx-macos.sh
#   ZGX_VERSION         the workspace version (a stable release, release.yml)
#                       or `<workspace version>-beta.<n>` (a beta of the next
#                       release, zgx-beta.yml)
#   ZGX_ENGINE          the engine: a built binary, a Zeron.app, its app tarball, or
#                       its `Zeron Engine` binary; built with LUAU_CXXFLAGS=-DNDEBUG
#                       (checked), which a stable release from before that isn't
#   ZGX_BIN             the interface, built already (with --features beta and
#                       ZERON_BETA_VERSION=<ZGX_VERSION>); otherwise built here
#   ZGX_TARGET_DIR      where to build it (apps/zgx/target)
#   ZGX_DMG=0           no dmg
#   CARGO_FLAGS         extra cargo flags (CI: --locked)
# Signing and notarization are scripts/package-macos.sh's: CODESIGN_IDENTITY,
# NOTARY_KEY_PATH + NOTARY_KEY_ID + NOTARY_ISSUER_ID.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
die() { echo "$*" >&2; exit 1; }

WORKSPACE="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
VERSION="${ZGX_VERSION:-}"
[[ "$VERSION" =~ ^([0-9]+\.[0-9]+\.[0-9]+)(-beta\.([0-9]+))?$ ]] ||
  die "ZGX_VERSION must be <x.y.z> or <x.y.z>-beta.<n>, not '$VERSION'"
CORE="${BASH_REMATCH[1]}"
NUMBER="${BASH_REMATCH[3]}"
[[ "$CORE" == "$WORKSPACE" ]] || die "ZGX_VERSION $VERSION isn't the workspace's $WORKSPACE or a beta of it"
ARCH="$(uname -m)"
OUT_DIR="$ROOT/target/package-zgx"
APP="$OUT_DIR/Zeron.app"
DMG="$OUT_DIR/zeron-$VERSION-macos-$ARCH.dmg"
APP_TARBALL="$OUT_DIR/zeron-$VERSION-macos-$ARCH-app.tar.gz"
ENTITLEMENTS="$ROOT/dist/macos/Zeron.entitlements"
read -r -a CARGO_EXTRA <<<"${CARGO_FLAGS:-}"
mkdir -p "$OUT_DIR"

# The engine: the binary given, or the one in a bundle or tarball.
ENGINE_SRC="${ZGX_ENGINE:-}"
[[ -n "$ENGINE_SRC" ]] || die "ZGX_ENGINE: a built engine, a Zeron.app, its app tarball or its Zeron Engine"
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
[[ "$ENGINE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]] || die "$ENGINE_BIN didn't say its version"
# The interface hands `zeron headless`, `zeron login` and the engine's other
# subcommands to this engine, which must parse them with the same command
# line (crates/cli). An engine from before that runs the background engine
# for any argument instead. (Read, not run: running an old one starts it.)
LC_ALL=C grep -aq "Multi-device controller for coding agents" "$ENGINE_BIN" ||
  die "the $ENGINE_VERSION engine predates the shared command line"
# Its Luau (visuals, plugin services) without its asserts: with them, each
# LUAU_ASSERT keeps its Luau source file's path (Luau/Common.h's __FILE__),
# VM/src/l*.cpp; with NDEBUG, none.
! LC_ALL=C grep -aqE 'VM[\\/]src[\\/]l[a-z]+\.cpp' "$ENGINE_BIN" ||
  die "$ENGINE_BIN was built with Luau's asserts: build it with LUAU_CXXFLAGS=-DNDEBUG"

# The interface: apps/zgx is a workspace of its own (its own lock, patches).
ZGX_BIN="${ZGX_BIN:-}"
if [[ -z "$ZGX_BIN" ]]; then
  target_dir="${ZGX_TARGET_DIR:-$ROOT/apps/zgx/target}"
  [[ "$target_dir" == /* ]] || target_dir="$PWD/$target_dir"
  # (From its folder: cargo reads .cargo/config.toml by where it runs, not by
  # the manifest, and the build's flags are there.)
  (cd "$ROOT/apps/zgx" && ZERON_BETA_VERSION="$VERSION" cargo build --release ${CARGO_EXTRA[@]+"${CARGO_EXTRA[@]}"} \
    --features beta --target-dir "$target_dir")
  ZGX_BIN="$target_dir/release/zeron-zgx"
fi
# Only an app build of this version may go in: the updater stages it by
# `zeron --version`, and a development build is staging's.
[[ "$("$ZGX_BIN" --version)" == "zeron $VERSION" ]] ||
  die "$ZGX_BIN isn't the $VERSION app (it says '$("$ZGX_BIN" --version)')"
# Luau without its asserts (a fifth of script time): zgx-script compiles its
# warning about them in only when they're on (cfg luau_asserts).
! LC_ALL=C grep -aq "Luau was built with its asserts" "$ZGX_BIN" ||
  die "$ZGX_BIN was built with Luau's asserts: LUAU_CXXFLAGS=-DNDEBUG (apps/zgx/.cargo/config.toml) didn't reach it"
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
# Zeron.app's Info.plist, as the old app's, with this one's versions: the
# short version stays numeric (a beta's is the next release's), the engine
# its own (crates/update `bundle_engine_version`), and zgx's floor macOS 13
# (Ghostty's).
#
# The bundle version (CFBundleVersion) only ever goes up, since LaunchServices
# takes the highest of several copies with the same bundle id (a retired
# bundle can linger): `<x.y.z>.<n>` for beta n, `<x.y.z>.9999` for the
# release itself. So 0.2.139.7 (beta.7) < 0.2.139.9999 (0.2.139) <
# 0.2.140.1 (0.2.140-beta.1). A beta's number stays below 9999. (The old app
# wrote plain `<x.y.z>`, below all of these.)
STABLE_BUILD=9999
if [[ -n "$NUMBER" ]]; then
  (( 10#$NUMBER < STABLE_BUILD )) || die "beta number $NUMBER must stay below $STABLE_BUILD, the release's build"
fi
PLIST="$APP/Contents/Info.plist"
sed "s/__VERSION__/$CORE/" "$ROOT/dist/macos/Info.plist" >"$PLIST"
plutil -replace CFBundleVersion -string "$CORE.${NUMBER:-$STABLE_BUILD}" "$PLIST"
plutil -replace ZeronEngineVersion -string "$ENGINE_VERSION" "$PLIST"
plutil -replace ZeronVersion -string "$VERSION" "$PLIST"
plutil -replace LSMinimumSystemVersion -string "13.0" "$PLIST"
plutil -convert xml1 "$PLIST"
plutil -lint "$PLIST" >/dev/null

# The scripts and Zeron's own plugins (read where they lie: templates and
# plugins live in the data dir). Not zgx's generated types or the tests.
rsync -a --exclude '.zgx/' --exclude '/tests/' --exclude '.lock' --exclude '.DS_Store' \
  "$ROOT/apps/zgx/ui/" "$APP/Contents/Resources/ui/"
rsync -a --exclude '.zgx/' --exclude '.DS_Store' \
  "$ROOT/apps/zgx/plugins-builtin/" "$APP/Contents/Resources/plugins-builtin/"
# Each ships the canonical SDK, whatever copy sits in its folder (examples_tests
# `sdk_copies_match_canonical` keeps those equal too).
for dir in "$APP/Contents/Resources/plugins-builtin/"*/; do
  cp "$ROOT/apps/zgx/plugins-sdk/zeron.luau" "$dir/zeron.luau"
done

mkdir -p "$APP/Contents/Resources/licenses/fonts"
cp "$ROOT/crates/ui/assets/fonts/licenses/"* "$ROOT/apps/zgx/fonts/"*-OFL.txt "$APP/Contents/Resources/licenses/fonts/"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$ROOT/LICENSE" "$APP/Contents/Resources/licenses/"
cp "$ROOT/crates/voice/NOTICE.md" "$APP/Contents/Resources/licenses/parakeet-v3.txt"

# Zeron's icon, as scripts/package-macos.sh's.
"$ROOT/scripts/macos-app-icon.sh" "$APP/Contents/Resources"

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

if [[ "${ZGX_DMG:-1}" == "0" ]]; then
  exit 0
fi

python3 -c 'import dmgbuild' 2>/dev/null ||
  python3 -m pip install --quiet --user dmgbuild 2>/dev/null ||
  python3 -m pip install --quiet --user --break-system-packages dmgbuild

BG_TIFF="$OUT_DIR/dmg-background.tiff"
tiffutil -cathidpicheck "$ROOT/dist/macos/dmg-background.png" \
  "$ROOT/dist/macos/dmg-background@2x.png" -out "$BG_TIFF" >/dev/null 2>&1

VOLUME="Zeron${NUMBER:+ Beta}" APP="$APP" DMG="$DMG" BG_TIFF="$BG_TIFF" python3 - <<'PY'
import os
import dmgbuild

app = os.environ["APP"]
dmgbuild.build_dmg(
    filename=os.environ["DMG"],
    volume_name=os.environ["VOLUME"],
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
