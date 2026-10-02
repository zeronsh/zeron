#!/usr/bin/env bash
# Offline check that both Linux installers (the curl one in edge/src/install.sh
# and the install.sh scripts/package-linux.sh puts in the tarball) write a
# launcher entry with absolute paths plus the icon, idempotently, under a
# throwaway HOME. Nothing touches the network, systemd, or the real home.
#
# Usage: scripts/test-linux-desktop-entry.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
VERSION=9.9.9
fail() { echo "FAIL: $*" >&2; exit 1; }

# A fake release: the tarball layout package-linux.sh produces, minus the real
# binary. `uname` is shimmed so the curl installer also runs on a macOS dev box.
PKG="zeron-$VERSION-linux-x86_64"
mkdir -p "$WORK/site/releases" "$WORK/pkg/$PKG/licenses/rdp" "$WORK/shim"
cp "$ROOT/LICENSE" "$WORK/pkg/$PKG/licenses/rdp/LICENSE"
printf '#!/bin/sh\nexit 0\n' >"$WORK/pkg/$PKG/zeron"
chmod 755 "$WORK/pkg/$PKG/zeron"
cp "$ROOT/dist/zeron.desktop" "$WORK/pkg/$PKG/zeron.desktop"
printf 'not-really-a-png' >"$WORK/pkg/$PKG/zeron.png"
echo "$VERSION" >"$WORK/site/releases/latest.txt"
tar -czf "$WORK/site/releases/$PKG.tar.gz" -C "$WORK/pkg" "$PKG"
printf '#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo x86_64 ;; *) exec /usr/bin/uname "$@" ;; esac\n' >"$WORK/shim/uname"
chmod 755 "$WORK/shim/uname"

# The tarball's install.sh lives in a heredoc inside package-linux.sh.
sed -n "/<<'INSTALL'/,/^INSTALL\$/p" "$ROOT/scripts/package-linux.sh" | sed '1d;$d' \
  | sed "s/__VERSION__/$VERSION/" >"$WORK/pkg/$PKG/install.sh"
chmod 755 "$WORK/pkg/$PKG/install.sh"

# The two copies of install_desktop_entry must stay identical.
fn_body() { sed -n '/^install_desktop_entry() {$/,/^}$/p' "$1"; }
[ -n "$(fn_body "$ROOT/edge/src/install.sh")" ] || fail "install_desktop_entry not found"
[ "$(fn_body "$ROOT/edge/src/install.sh")" = "$(fn_body "$ROOT/scripts/package-linux.sh")" ] \
  || fail "install_desktop_entry differs between edge/src/install.sh and scripts/package-linux.sh"

# run_curl HOME [ENV=VALUE ...] / run_tarball HOME [ENV=VALUE ...]
# Each run gets its own TMPDIR, which must be empty again afterwards (the curl
# installer's EXIT trap removes its download dir).
mkdir -p "$WORK/tmp"
run_curl() {
  local home="$1"; shift
  env -i HOME="$home" USER=tester PATH="$WORK/shim:/usr/bin:/bin" TMPDIR="$WORK/tmp" "$@" \
    ZERON_BASE_URL="file://$WORK/site" sh "$ROOT/edge/src/install.sh" >"$WORK/out.log" 2>&1 \
    || { cat "$WORK/out.log" >&2; fail "curl installer exited non-zero"; }
  [ -z "$(ls -A "$WORK/tmp")" ] || fail "curl installer left files in TMPDIR: $(ls -A "$WORK/tmp")"
}
run_tarball() {
  local home="$1"; shift
  env -i HOME="$home" USER=tester PATH="/usr/bin:/bin" TMPDIR="$WORK/tmp" "$@" \
    bash "$WORK/pkg/$PKG/install.sh" >"$WORK/out.log" 2>&1 \
    || { cat "$WORK/out.log" >&2; fail "tarball installer exited non-zero"; }
  [ -z "$(ls -A "$WORK/tmp")" ] || fail "tarball installer left files in TMPDIR: $(ls -A "$WORK/tmp")"
}

# check HOME DATA_HOME
check() {
  local home="$1" data="$2" entry="$2/applications/zeron.desktop"
  [ -f "$entry" ] || fail "missing $entry"
  [ -f "$data/icons/hicolor/1024x1024/apps/zeron.png" ] || fail "missing hicolor icon"
  if [ "$installer" = tarball ]; then
    cmp "$WORK/pkg/$PKG/licenses/rdp/LICENSE" "$data/zeron/licenses/rdp/LICENSE" \
      || fail "RDP license missing or changed"
  fi
  # `$(...)` strips nothing needed here: paths in these tests have no newlines.
  grep -qxF "TryExec=$home/.zeron/app/current/zeron" "$entry" || fail "TryExec: $(grep '^TryExec' "$entry")"
  grep -qxF "Icon=$home/.zeron/app/current/zeron.png" "$entry" || fail "Icon: $(grep '^Icon' "$entry")"
  grep -qxF "StartupWMClass=zeron" "$entry" || fail "StartupWMClass changed"
  [ "$(grep -c '^\[Desktop Entry\]' "$entry")" = 1 ] || fail "duplicated entry"
  [ "$(grep -c '^Exec=' "$entry")" = 1 ] || fail "Exec lines"
  [ -z "$(find "$data" -name '.zeron*')" ] || fail "temp files left behind"
  # A user-level icon cache is only ever refreshed, never created.
  [ ! -e "$data/icons/hicolor/icon-theme.cache" ] || fail "created a hicolor icon cache"
  if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$entry" || fail "desktop-file-validate"
  fi
}

for installer in curl tarball; do
  run() { "run_$installer" "$@"; }

  # Default XDG location, then a re-run (how updates are installed) is stable.
  home="$WORK/$installer-a/home"; mkdir -p "$home"
  run "$home"
  check "$home" "$home/.local/share"
  grep -qxF "Exec=$home/.zeron/app/current/zeron %u" "$home/.local/share/applications/zeron.desktop" \
    || fail "$installer: Exec line"
  before="$(cat "$home/.local/share/applications/zeron.desktop")"
  run "$home"
  check "$home" "$home/.local/share"
  [ "$before" = "$(cat "$home/.local/share/applications/zeron.desktop")" ] || fail "$installer: re-run changed the entry"

  # XDG_DATA_HOME wins when absolute; a relative value is ignored per the spec.
  home="$WORK/$installer-b/home"; mkdir -p "$home"
  run "$home" XDG_DATA_HOME="$WORK/$installer-b/xdg"
  check "$home" "$WORK/$installer-b/xdg"
  [ ! -e "$home/.local/share/applications" ] || fail "$installer: wrote outside XDG_DATA_HOME"
  home="$WORK/$installer-c/home"; mkdir -p "$home"
  run "$home" XDG_DATA_HOME=relative/dir
  check "$home" "$home/.local/share"

  # A home with a space and characters the Exec key must quote and escape.
  home="$WORK/$installer-d/h o\$me\"x%y"; mkdir -p "$home"
  run "$home"
  check "$home" "$home/.local/share"
  # Spec: quote the argument, `\` before " and $ (doubled again for the file's
  # string escaping), and `%%` for a literal `%`.
  want="Exec=\"$WORK/$installer-d/"'h o\\$me\\"x%%y'"/.zeron/app/current/zeron\" %u"
  grep -qxF "$want" "$home/.local/share/applications/zeron.desktop" \
    || fail "$installer: Exec quoting: $(grep '^Exec=' "$home/.local/share/applications/zeron.desktop")"
  echo "ok: $installer installer"
done
