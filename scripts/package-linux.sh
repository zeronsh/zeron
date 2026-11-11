#!/usr/bin/env bash
# Linux packaging: build the release binary and produce
#   target/package/zeron-<version>-linux-<arch>.tar.gz
# containing the binaries (the app and its background engine), the .desktop
# entry, and the icon, plus an install.sh
# that installs them into the self-updating ~/.zeron/app layout and links
# ~/.local (XDG) paths to it.
#
# Usage: scripts/package-linux.sh
# Env:   PROFILE=debug for a fast unoptimized package (CI smoke); default release.
#
# The zgx app (release.yml's stable, zgx-beta.yml's beta), with both of these
# set: the zgx interface (apps/zgx, its `beta` feature) as `zeron`, its
# scripts beside it, and the `zeron-engine` it's given, built from the same
# commit:
#   ZGX_VERSION=<workspace version>, or <workspace version>-beta.<n>
#   ZGX_ENGINE=<a zeron-engine binary, or a release's linux tarball>
# Without them: the old app (apps/zeron), kept for a release that has to go
# back to it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
PROFILE="${PROFILE:-release}"
ARCH="$(uname -m)"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
ZGX_VERSION="${ZGX_VERSION:-}"
if [[ -n "$ZGX_VERSION" ]]; then
  [[ "$ZGX_VERSION" == "$VERSION" || "$ZGX_VERSION" =~ ^"$VERSION"-beta\.[0-9]+$ ]] ||
    { echo "ZGX_VERSION $ZGX_VERSION isn't the workspace's $VERSION or a beta of it" >&2; exit 1; }
  [[ -f "${ZGX_ENGINE:-}" ]] || { echo "ZGX_ENGINE: a zeron-engine binary, or a release's linux tarball" >&2; exit 1; }
  VERSION="$ZGX_VERSION"
fi
OUT_DIR="$ROOT/target/package"
STAGE="$OUT_DIR/zeron-$VERSION-linux-$ARCH"
TARBALL="$STAGE.tar.gz"

cd "$ROOT"
if [[ -n "$ZGX_VERSION" ]]; then
  # The engine: the binary given, or the one in a release's tarball.
  if [[ "$ZGX_ENGINE" == *.tar.gz ]]; then
    UNPACK="$OUT_DIR/.stable"
    rm -rf "$UNPACK" && mkdir -p "$UNPACK"
    tar -xzf "$ZGX_ENGINE" -C "$UNPACK" --strip-components=1
    ENGINE_BIN="$UNPACK/zeron-engine"
  else
    ENGINE_BIN="$ZGX_ENGINE"
  fi
  [[ -x "$ENGINE_BIN" ]] || { echo "no zeron-engine in $ZGX_ENGINE" >&2; exit 1; }
  # The interface hands `zeron headless` and the engine's other subcommands to
  # this engine, as scripts/package-zgx-macos.sh requires of macOS's. (Read,
  # not run: an engine from before the shared command line starts on any
  # argument.)
  LC_ALL=C grep -aq "Multi-device controller for coding agents" "$ENGINE_BIN" ||
    { echo "the engine predates the shared command line" >&2; exit 1; }
  # A workspace of its own (its own lock, patches).
  ZERON_BETA_VERSION="$ZGX_VERSION" cargo build --release --locked --features beta \
    --manifest-path "$ROOT/apps/zgx/Cargo.toml" --target-dir "$ROOT/apps/zgx/target"
  BIN="$ROOT/apps/zgx/target/release/zeron-zgx"
  [[ "$("$BIN" --version)" == "zeron $ZGX_VERSION" ]] ||
    { echo "$BIN isn't the $ZGX_VERSION app (it says '$("$BIN" --version)')" >&2; exit 1; }
  # Production's only: neither binary may carry staging's edge or sign-in.
  STAGING_MARKS='staging\.zeron\.sh|zeron-edge-staging|client_01KWD0E9SYPV2WE4QN9TP9DP0W'
  for bin in "$BIN" "$ENGINE_BIN"; do
    if LC_ALL=C grep -aqE "$STAGING_MARKS" "$bin"; then
      echo "$bin carries staging's edge or sign-in client:" >&2
      LC_ALL=C grep -aoE "$STAGING_MARKS" "$bin" | sort -u >&2
      exit 1
    fi
  done
elif [[ "$PROFILE" == "release" ]]; then
  # Fat LTO in one codegen unit: `zeron` goes from 205 to 172 MB (16% less), with
  # the same start-up (median engine boot 387 ms both ways, n=20 each), at about
  # 60% more build time. A Cloud machine copies the binary back in on every wake,
  # so its size is wake time. Only here: macOS and Windows keep their build times.
  # The release was already stripped, with no debug info to split.
  CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    cargo build --release -p zeron
  BIN="$ROOT/target/release/zeron"
else
  cargo build -p zeron
  BIN="$ROOT/target/debug/zeron"
fi
# The background engine the app starts: sessions keep running with the app
# closed, in a process of its own.
ENGINE_BIN="${ENGINE_BIN:-$(dirname "$BIN")/zeron-engine}"

rm -rf "$STAGE" "$TARBALL"
mkdir -p "$STAGE"
install -m 755 "$BIN" "$STAGE/zeron"
install -m 755 "$ENGINE_BIN" "$STAGE/zeron-engine"
install -m 644 "$ROOT/dist/zeron.desktop" "$STAGE/zeron.desktop"
install -m 644 "$ROOT/dist/zeron.png" "$STAGE/zeron.png"
install -m 644 "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$STAGE/"
mkdir -p "$STAGE/licenses/fonts"
cp "$ROOT/crates/ui/assets/fonts/licenses/"* "$STAGE/licenses/fonts/"
cp "$ROOT/crates/voice/NOTICE.md" "$STAGE/licenses/parakeet-v3.txt"
if [[ -n "$ZGX_VERSION" ]]; then
  # The scripts and Zeron's own plugins, as scripts/package-zgx-macos.sh: read
  # beside the binary. Not zgx's generated types or the tests.
  rsync -a --exclude '.zgx/' --exclude '/tests/' --exclude '.lock' --exclude '.DS_Store' \
    "$ROOT/apps/zgx/ui/" "$STAGE/ui/"
  rsync -a --exclude '.zgx/' --exclude '.DS_Store' \
    "$ROOT/apps/zgx/plugins-builtin/" "$STAGE/plugins-builtin/"
  # Each ships the canonical SDK, as scripts/package-zgx-macos.sh.
  for dir in "$STAGE/plugins-builtin/"*/; do
    cp "$ROOT/apps/zgx/plugins-sdk/zeron.luau" "$dir/zeron.luau"
  done
  cp "$ROOT/apps/zgx/fonts/"*-OFL.txt "$STAGE/licenses/fonts/"
  rm -rf "$OUT_DIR/.stable"
fi

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
# A beta's directory isn't named as a version: crates/update `managed_dir`.
case "$VERSION" in
  *-beta.*) DEST="$APP_ROOT/beta-$VERSION" ;;
  *) DEST="$APP_ROOT/$VERSION" ;;
esac
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
