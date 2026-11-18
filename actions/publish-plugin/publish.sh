#!/usr/bin/env bash
# Publish a Zeron plugin from CI: install Zeron's Linux command line (the
# release's `zeron-engine`, checked against the release's sha256), then run
# `zeron plugin publish <folder>` with the deploy key in ZERON_DEPLOY_KEY.
# Without a key it only checks (`zeron plugin check`): what a pull request
# from a fork, which gets no secrets, can still run.
#
# Env (the action sets these from its inputs):
#   ZERON_DEPLOY_KEY   the deploy key's secret (zpk_…), or empty
#   ZERON_PLUGIN_PATH  the plugin's folder (default .)
#   ZERON_DRY_RUN      true: check, sign and pack, don't submit
#   ZERON_VERSION      latest, or a release's version
#   ZERON_RELEASES_URL where releases are (default https://zeron.sh/releases)
#   ZERON_EDGE_URL     the store's edge (default: the command line's own)
#
# Exits as `zeron plugin publish` does: 0 submitted (or checked), 1 the
# plugin doesn't meet the store's rules, 2 set-up, 3 the store refused the
# key or the account, 4 the store couldn't be reached (re-run the job).

set -euo pipefail

fail() {
  echo "::error::$1" >&2
  exit 2
}

[ "$(uname -s)" = Linux ] || fail "Publishing a Zeron plugin runs on a Linux runner (runs-on: ubuntu-latest)."
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) fail "Zeron has no command line for $(uname -m) runners." ;;
esac
for tool in curl jq sha256sum tar; do
  command -v "$tool" >/dev/null || fail "$tool is needed on the runner."
done

releases="${ZERON_RELEASES_URL:-https://zeron.sh/releases}"
version="${ZERON_VERSION:-latest}"
case "$version" in
  latest) manifest_url="$releases/manifest.json" ;;
  *[!0-9A-Za-z.+-]* | "") fail "zeron-version is latest or a version like 0.4.0." ;;
  *) manifest_url="$releases/manifest-$version.json" ;;
esac

work="${RUNNER_TEMP:-$(mktemp -d)}/zeron-cli"
rm -rf "$work"
mkdir -p "$work/bin" "$work/data"

manifest="$(curl -fsSL --retry 3 "$manifest_url")" || fail "Couldn't read Zeron's release manifest ($manifest_url)."
version="$(jq -r '.version // empty' <<<"$manifest")"
[ -n "$version" ] || fail "Zeron's release manifest names no version."
file="zeron-$version-linux-$arch.tar.gz"
sha="$(jq -r --arg f "$file" '.files[$f].sha256 // empty' <<<"$manifest")"
[ -n "$sha" ] || fail "Zeron $version has no $file."

curl -fsSL --retry 3 "$releases/$file" -o "$work/$file" || fail "Couldn't download $file."
echo "$sha  $work/$file" | sha256sum -c --quiet - || fail "$file isn't what Zeron's release manifest says (sha256)."
tar -xzf "$work/$file" -C "$work/bin" --strip-components=1 --wildcards '*/zeron-engine' || fail "$file has no zeron-engine."
zeron="$work/bin/zeron-engine"
echo "Zeron $("$zeron" --version | awk '{print $2}') ($arch)"

# Nothing of this job's is kept: Zeron's data folder is a scratch one.
export ZERON_DATA_DIR="$work/data"
folder="${ZERON_PLUGIN_PATH:-.}"

if [ -z "${ZERON_DEPLOY_KEY:-}" ]; then
  echo "No deploy key (the key input is empty): checking only."
  exec "$zeron" plugin check "$folder"
fi
args=(plugin publish "$folder")
[ "${ZERON_DRY_RUN:-false}" = true ] && args+=(--dry-run)
exec "$zeron" "${args[@]}"
