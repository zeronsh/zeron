#!/usr/bin/env bash
# Zeron's own template files by hash, for each release: what the store's
# reviewers read a template's files against. One <version>.json a tag
# (`zeron template stock`), made from that tag's own apps/zgx/ui, for the
# store's bucket as stock/<version>.json. It writes files here and uploads
# nothing.
#
# Usage: scripts/template-stock-backfill.sh <out folder> [tag …]
#   (no tags: every v0.3.0 and later)
# Env:
#   ZERON   the command line to run (default: `zeron` on the PATH)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ZERON="${ZERON:-zeron}"
OUT="${1:?Say where to write them: scripts/template-stock-backfill.sh <out folder> [tag …]}"
shift
mkdir -p "$OUT"

if [ "$#" -gt 0 ]; then
  tags=("$@")
else
  mapfile -t tags < <(git -C "$ROOT" tag --list 'v[0-9]*' | sort -V | awk -F'[v.]' '($2 > 0) || ($2 == 0 && $3 >= 3)')
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/zeron-stock.XXXXXX")"
trap 'rm -rf "$work"' EXIT
for tag in "${tags[@]}"; do
  version="${tag#v}"
  rm -rf "$work/tree"
  mkdir "$work/tree"
  git -C "$ROOT" archive "$tag" apps/zgx/ui | tar -x -C "$work/tree"
  # (Whole or not at all: never half a file where one is.)
  "$ZERON" template stock "$work/tree/apps/zgx/ui" --version "$version" > "$work/$version.json"
  mv "$work/$version.json" "$OUT/$version.json"
  echo "$tag  $(wc -c < "$OUT/$version.json") bytes"
done
