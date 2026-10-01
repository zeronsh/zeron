#!/bin/sh
# Zeron (native) headless installer.
#
#   curl -fsSL https://zeron.sh/install.sh | sh
#
# Installs the self-contained native binary (no runtime deps) to
# ~/.zeron/app, puts `zeron` on PATH, adds a launcher entry and icon under
# $XDG_DATA_HOME (default ~/.local/share), and runs it as a local-only
# systemd user service that survives reboots. Signing in is optional and
# enables sync after a restart. Re-running
# upgrades in place; ~/.zeron state is preserved.
#
# The binary ships with production endpoints baked in: no ZERON_EDGE_URL or
# client-id configuration needed. Overrides (if any) go in ~/.zeron/env.
set -eu

BASE="${ZERON_BASE_URL:-https://zeron.sh}"

# --- platform ---------------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux) plat=linux ;;
  Darwin)
    echo "zeron install: on macOS, download the desktop app instead:" >&2
    echo "  $BASE/releases/latest.txt → $BASE/releases/zeron-<version>-macos-arm64.dmg" >&2
    exit 1
    ;;
  *)
    echo "zeron install: unsupported OS '$os' — only Linux for now." >&2
    exit 1
    ;;
esac
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *)
    echo "zeron install: unsupported architecture '$arch'." >&2
    exit 1
    ;;
esac

# --- download ----------------------------------------------------------------
ver="$(curl -fsSL "$BASE/releases/latest.txt" | tr -d '[:space:]')"
[ -n "$ver" ] || { echo "zeron install: could not resolve latest version" >&2; exit 1; }
file="zeron-$ver-$plat-$arch.tar.gz"
data_root="$HOME/.zeron"
app_root="$data_root/app"
dest="$app_root/$ver"

if [ -x "$dest/zeron" ]; then
  echo "zeron $ver already downloaded — relinking."
else
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  echo "downloading zeron $ver ($plat-$arch)…"
  curl -fSL --progress-bar "$BASE/releases/$file" -o "$tmp/$file"
  mkdir -p "$dest"
  tar -xzf "$tmp/$file" -C "$dest" --strip-components=1
fi

ln -sfn "$dest" "$app_root/current"
mkdir -p "$HOME/.local/bin"
ln -sf "$app_root/current/zeron" "$HOME/.local/bin/zeron"

# --- desktop entry -------------------------------------------------------------
# Launchers list Zeron through a per-user .desktop entry. The one in the tarball
# says `Exec=zeron` and `TryExec=zeron`, which only resolve when ~/.local/bin is
# on the PATH of the desktop session (often not, e.g. a bare Wayland + fuzzel
# setup) and TryExec then hides the entry outright. So write it with absolute
# paths through the `current` symlink, which keeps working across updates. The
# icon is referenced by path too: the only artwork is 1024x1024, a size the
# hicolor theme doesn't index, so a name lookup alone can come up empty.
# (Duplicated in scripts/package-linux.sh's install.sh; keep the two in sync.)
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
# A missing desktop entry must never fail an otherwise good install.
install_desktop_entry "$dest" "$app_root" \
  || echo "warn: could not install the desktop entry — Zeron won't appear in application launchers"

# --- service -----------------------------------------------------------------
# The daemon is useful before auth: without a saved session it serves the local
# profile. Login only changes which profile the next daemon start selects.

service=manual
if command -v systemctl >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
  mkdir -p "$HOME/.config/systemd/user"
  cat >"$HOME/.config/systemd/user/zeron.service" <<'UNIT'
[Unit]
Description=Zeron native headless engine
After=network-online.target
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
ExecStart=%h/.zeron/app/current/zeron headless
Restart=on-failure
RestartSec=5
EnvironmentFile=-%h/.zeron/env

[Install]
WantedBy=default.target
UNIT
  systemctl --user daemon-reload
  systemctl --user enable zeron
  systemctl --user restart zeron
  service=running
  # Keep the user manager (and the engine) running without an active login.
  loginctl enable-linger "$USER" 2>/dev/null \
    || sudo -n loginctl enable-linger "$USER" 2>/dev/null \
    || echo "warn: could not enable linger — the engine stops when you log out (run: sudo loginctl enable-linger $USER)"
else
  echo "warn: systemd user session not available — run the engine manually with: zeron headless"
fi

# --- agent CLIs ---------------------------------------------------------------
command -v claude >/dev/null 2>&1 || \
  echo "note: Claude Code CLI not found — install it with: curl -fsSL https://claude.ai/install.sh | bash"

case ":$PATH:" in
  *":$HOME/.local/bin:"*) path_hint="" ;;
  *) path_hint=' (add ~/.local/bin to your PATH)' ;;
esac

echo ""
echo "✓ zeron $ver installed$path_hint"
echo ""
case "$service" in
  running)
    echo "the engine is running with the new version (local-only unless sync is enabled)."
    echo "  systemctl --user status zeron    check the service"
    echo ""
    echo "optional sync (local sessions stay local):"
    echo "  systemctl --user stop zeron"
    echo "  zeron login"
    echo "  systemctl --user restart zeron"
    ;;
  manual)
    echo "next: run the local-only engine with \`zeron headless\`."
    echo "optional sync: run \`zeron login\` before starting the engine."
    ;;
esac
