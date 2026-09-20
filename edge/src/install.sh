#!/bin/sh
# Zeron (native) headless installer.
#
#   curl -fsSL https://zeron.sh/install.sh | sh
#
# Installs the self-contained native binary (no runtime deps) to
# ~/.zeron/app, puts `zeron` on PATH, and runs it as a local-only
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
if [ -e /etc/NIXOS ] && [ -z "${ZERON_INSTALL_FORCE:-}" ]; then
  echo "zeron install: NixOS cannot run the prebuilt binary; install from the flake instead:" >&2
  echo "  nix profile install github:zeronsh/zeron" >&2
  echo "(set ZERON_INSTALL_FORCE=1 to install anyway, e.g. with nix-ld configured)" >&2
  exit 1
fi

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
