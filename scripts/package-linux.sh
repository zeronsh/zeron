#!/usr/bin/env bash
# Linux packaging: build the release binary and produce
#   target/package/zeron-<version>-linux-<arch>.tar.gz
# containing the binary, the .desktop entry, and the icon, plus an install.sh
# that installs them into the self-updating ~/.zeron/app layout and links
# ~/.local (XDG) paths to it.
#
# Usage: scripts/package-linux.sh
# Env:   PROFILE=debug for a fast unoptimized package (CI smoke); default release.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
PROFILE="${PROFILE:-release}"
ARCH="$(uname -m)"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
OUT_DIR="$ROOT/target/package"
STAGE="$OUT_DIR/zeron-$VERSION-linux-$ARCH"
TARBALL="$STAGE.tar.gz"

cd "$ROOT"
if [[ "$PROFILE" == "release" ]]; then
  cargo build --release -p zeron
  BIN="$ROOT/target/release/zeron"
else
  cargo build -p zeron
  BIN="$ROOT/target/debug/zeron"
fi

rm -rf "$STAGE" "$TARBALL"
mkdir -p "$STAGE"
install -m 755 "$BIN" "$STAGE/zeron"
install -m 644 "$ROOT/dist/zeron.desktop" "$STAGE/zeron.desktop"
install -m 644 "$ROOT/dist/zeron.png" "$STAGE/zeron.png"
mkdir -p "$STAGE/licenses/fonts"
cp "$ROOT/crates/ui/assets/fonts/licenses/"* "$STAGE/licenses/fonts/"

cat >"$STAGE/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# Install Zeron for this user (no root needed), in the layout the in-app
# updater manages: ~/.zeron/app/<version> behind a `current` symlink — the
# same layout `curl -fsSL https://zeron.sh/install.sh | sh` uses — with
# ~/.local/bin/zeron and the desktop entry pointing through it.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION="__VERSION__"
APP_ROOT="$HOME/.zeron/app"
DEST="$APP_ROOT/$VERSION"
mkdir -p "$APP_ROOT"
if [ ! -x "$DEST/zeron" ]; then
  # Copy beside the final name, then rename: an interrupted install never
  # leaves a half-copied version the updater would trust.
  STAGE="$(mktemp -d "$APP_ROOT/.install-$VERSION-XXXXXX")"
  cp -R "$HERE/." "$STAGE/"
  rm -rf "$DEST"
  mv "$STAGE" "$DEST"
fi
ln -sfn "$DEST" "$APP_ROOT/current"
mkdir -p "$HOME/.local/bin"
ln -sfn "$APP_ROOT/current/zeron" "$HOME/.local/bin/zeron"
install -Dm644 "$HERE/zeron.desktop" "$HOME/.local/share/applications/zeron.desktop"
install -Dm644 "$HERE/zeron.png" "$HOME/.local/share/icons/hicolor/1024x1024/apps/zeron.png"
command -v update-desktop-database >/dev/null 2>&1 \
  && update-desktop-database "$HOME/.local/share/applications" || true
echo "Installed Zeron $VERSION. It updates itself from now on."
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "Add ~/.local/bin to your PATH to run \`zeron\` from a terminal." ;;
esac
INSTALL
sed -i "s/__VERSION__/$VERSION/" "$STAGE/install.sh"
chmod 755 "$STAGE/install.sh"

tar -czf "$TARBALL" -C "$OUT_DIR" "$(basename "$STAGE")"
rm -rf "$STAGE"
echo "packaged: $TARBALL"
tar -tzf "$TARBALL"
