#!/usr/bin/env bash
# Desktop installation. Shares version directories with the headless installer,
# but does not create or start a service. Re-run once to migrate legacy copies.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
version="$("$HERE/zeron" --version)"
version="${version#zeron }"
[[ "$version" =~ ^[0-9]+(\.[0-9]+)*$ ]] || { echo "Invalid Zeron version" >&2; exit 1; }
app_root="$HOME/.zeron/app"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
mkdir -p "$app_root" "$HOME/.local/bin"
exec 9>"$app_root/.update.lock"
flock -n 9 || { echo "Another Zeron update is in progress; try again." >&2; exit 1; }
stage="$(mktemp -d "$app_root/.install-XXXXXXXX")"
trap 'rm -rf "$stage"' EXIT
destination="$app_root/$version"
if [[ -e "$destination" ]]; then
  # Version directories are immutable: never overwrite a running executable.
  cmp -s "$HERE/zeron" "$destination/zeron" || {
    echo "Installed version $version has different contents; leaving it intact." >&2
    exit 1
  }
else
  mkdir "$stage/version"
  cp -a "$HERE/." "$stage/version/"
  sha256sum "$stage/version/zeron" | cut -d' ' -f1 >"$stage/version/.zeron-update-sha256"
  mv "$stage/version" "$destination"
fi
ln -s "$destination" "$stage/current"
mv -Tf "$stage/current" "$app_root/current"
# A temporary symlink + rename also migrates the old ~/.local/bin regular file
# without following it or truncating a running executable.
bin_stage="$(mktemp -d "$HOME/.local/bin/.zeron-install-XXXXXXXX")"
trap 'rm -rf "$stage" "$bin_stage"' EXIT
ln -s "$app_root/current/zeron" "$bin_stage/zeron"
mv -Tf "$bin_stage/zeron" "$HOME/.local/bin/zeron"
install -Dm644 "$HERE/zeron.desktop" "$data_home/applications/zeron.desktop"
install -Dm644 "$HERE/zeron.png" "$data_home/icons/hicolor/1024x1024/apps/zeron.png"
# Desktop launchers do not necessarily inherit the user's shell PATH.
desktop_path="${HOME//\\/\\\\}"
desktop_path="${desktop_path//\"/\\\"}"
desktop_path="${desktop_path//\$/\\\$}"
desktop_path="${desktop_path//\`/\\\`}"
desktop_path="${desktop_path//%/%%}"
# Desktop-entry string unescaping happens before Exec argument unquoting.
desktop_path="${desktop_path//\\/\\\\}"
sed -i '/^Exec=/d; /^TryExec=/d' "$data_home/applications/zeron.desktop"
# A fixed executable also lets desktop launchers expand %% in the path before
# resolving the Zeron executable (GLib resolves Exec's first token earlier).
printf 'Exec=/usr/bin/env "%s/.local/bin/zeron" %%u\n' "$desktop_path" >>"$data_home/applications/zeron.desktop"
command -v update-desktop-database >/dev/null 2>&1 \
  && update-desktop-database "$data_home/applications" || true
echo "Installed Zeron $version. Future updates are available in the sidebar."
