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
  cargo build --release --locked -p zeron
  BIN="$ROOT/target/release/zeron"
else
  cargo build --locked -p zeron
  BIN="$ROOT/target/debug/zeron"
fi

rm -rf "$STAGE" "$TARBALL"
mkdir -p "$STAGE"
install -m 755 "$BIN" "$STAGE/zeron"
install -m 644 "$ROOT/dist/zeron.desktop" "$STAGE/zeron.desktop"
install -m 644 "$ROOT/dist/zeron.png" "$STAGE/zeron.png"
mkdir -p "$STAGE/licenses/fonts"
cp "$ROOT/crates/ui/assets/fonts/licenses/"* "$STAGE/licenses/fonts/"
cp "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$STAGE/licenses/"
python3 "$ROOT/scripts/collect-rdp-licenses.py" "$STAGE/licenses/rdp"
cp "$ROOT/crates/voice/NOTICE.md" "$STAGE/licenses/parakeet-v3.txt"

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
if ! "$DEST/zeron" --version >/dev/null; then
  echo "Zeron could not start; see the loader error above. Install the missing runtime libraries (including ALSA, libasound.so.2), then retry." >&2
  exit 1
fi
ln -sfn "$DEST" "$APP_ROOT/current"
mkdir -p "$HOME/.local/bin"
ln -sfn "$APP_ROOT/current/zeron" "$HOME/.local/bin/zeron"

# Launchers list Zeron through a per-user .desktop entry. The one in the tarball
# says `Exec=zeron` and `TryExec=zeron`, which only resolve when ~/.local/bin is
# on the PATH of the desktop session (often not, e.g. a bare Wayland + fuzzel
# setup) and TryExec then hides the entry outright. So write it with absolute
# paths through the `current` symlink, which keeps working across updates. The
# icon is referenced by path too: the only artwork is 1024x1024, a size the
# hicolor theme doesn't index, so a name lookup alone can come up empty.
# (Duplicated in edge/src/install.sh, the curl installer; keep the two in sync.)
install_desktop_entry() {
  src="$1"
  app="$2"
  [ -f "$src/zeron.desktop" ] && [ -f "$src/zeron.png" ] || return 1
  case "${XDG_DATA_HOME:-}" in
    /*) data_home="$XDG_DATA_HOME" ;;
    *) data_home="$HOME/.local/share" ;;
  esac
  apps_dir="$data_home/applications"
  icon_dir="$data_home/icons/hicolor/1024x1024/apps"
  bin="$app/current/zeron"
  icon="$app/current/zeron.png"
  # Desktop Entry `Exec` quoting: double-quote an argument with reserved
  # characters, backslash-escape ", `, $ and \ inside, then double every
  # backslash again for the file's own string escaping. `%` must be `%%`.
  case "$bin" in
    *[!A-Za-z0-9_./-]*)
      exec_bin="\"$(printf '%s' "$bin" | sed -e 's/\\/\\\\\\\\/g' -e 's/["`$]/\\\\&/g' -e 's/%/%%/g')\""
      ;;
    *) exec_bin="$bin" ;;
  esac
  try_bin="$(printf '%s' "$bin" | sed 's/\\/\\\\/g')"
  icon_val="$(printf '%s' "$icon" | sed 's/\\/\\\\/g')"

  mkdir -p "$apps_dir" "$icon_dir" || return 1
  # Write beside the final name, then rename, so a launcher watching the
  # directory never reads a half-written entry (a leading dot is ignored).
  # Not `tmp`: sh has no `local`, and the curl installer's EXIT trap removes
  # its download dir through `$tmp`.
  entry_tmp="$apps_dir/.zeron.desktop.$$"
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      Exec=*) printf 'Exec=%s %%u\n' "$exec_bin" ;;
      TryExec=*) printf 'TryExec=%s\n' "$try_bin" ;;
      Icon=*) printf 'Icon=%s\n' "$icon_val" ;;
      *) printf '%s\n' "$line" ;;
    esac
  done <"$src/zeron.desktop" >"$entry_tmp" || { rm -f "$entry_tmp"; return 1; }
  mv -f "$entry_tmp" "$apps_dir/zeron.desktop" || { rm -f "$entry_tmp"; return 1; }
  cp "$src/zeron.png" "$icon_dir/.zeron.png.$$" \
    && mv -f "$icon_dir/.zeron.png.$$" "$icon_dir/zeron.png" || return 1

  # Best-effort cache refresh; both tools are optional. The icon cache is only
  # refreshed, never created: a user-level hicolor cache nobody else maintains
  # would hide icons other apps later install there, and the entry above
  # references the icon by path anyway.
  command -v update-desktop-database >/dev/null 2>&1 \
    && update-desktop-database "$apps_dir" >/dev/null 2>&1 || true
  [ -f "$data_home/icons/hicolor/icon-theme.cache" ] \
    && command -v gtk-update-icon-cache >/dev/null 2>&1 \
    && gtk-update-icon-cache -q -t -f "$data_home/icons/hicolor" >/dev/null 2>&1 || true
  return 0
}
install_desktop_entry "$HERE" "$APP_ROOT" \
  || echo "warn: could not install the desktop entry — Zeron won't appear in application launchers"

case "${XDG_DATA_HOME:-}" in
  /*) license_data_home="$XDG_DATA_HOME" ;;
  *) license_data_home="$HOME/.local/share" ;;
esac
mkdir -p "$license_data_home/zeron/licenses"
cp -R "$HERE/licenses/." "$license_data_home/zeron/licenses/"

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
