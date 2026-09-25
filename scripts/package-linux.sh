#!/usr/bin/env bash
# Linux packaging: build the release binary and produce
#   target/package/glitch-flow-<version>-linux-<arch>.tar.gz
# containing the binary, the .desktop entry, and the icon, plus an install.sh
# that drops them into ~/.local (XDG) paths.
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
STAGE="$OUT_DIR/glitch-flow-$VERSION-linux-$ARCH"
TARBALL="$STAGE.tar.gz"

cd "$ROOT"
if [[ "$PROFILE" == "release" ]]; then
  cargo build --release -p glitch-flow
  BIN="$ROOT/target/release/glitch-flow"
else
  cargo build -p glitch-flow
  BIN="$ROOT/target/debug/glitch-flow"
fi

rm -rf "$STAGE" "$TARBALL"
mkdir -p "$STAGE"
install -m 755 "$BIN" "$STAGE/glitch-flow"
install -m 644 "$ROOT/dist/glitch-flow.desktop" "$STAGE/glitch-flow.desktop"
install -m 644 "$ROOT/dist/glitch-flow.png" "$STAGE/glitch-flow.png"
mkdir -p "$STAGE/licenses/fonts"
cp "$ROOT/crates/ui/assets/fonts/licenses/"* "$STAGE/licenses/fonts/"

cat >"$STAGE/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# Install Glitch Flow into ~/.local (no root needed).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install -Dm755 "$HERE/glitch-flow" "$HOME/.local/bin/glitch-flow"
install -Dm644 "$HERE/glitch-flow.desktop" "$HOME/.local/share/applications/glitch-flow.desktop"
install -Dm644 "$HERE/glitch-flow.png" "$HOME/.local/share/icons/hicolor/1024x1024/apps/glitch-flow.png"
command -v update-desktop-database >/dev/null 2>&1 \
  && update-desktop-database "$HOME/.local/share/applications" || true
echo "Installed. Make sure ~/.local/bin is on your PATH."
INSTALL
chmod 755 "$STAGE/install.sh"

tar -czf "$TARBALL" -C "$OUT_DIR" "$(basename "$STAGE")"
rm -rf "$STAGE"
echo "packaged: $TARBALL"
tar -tzf "$TARBALL"
