#!/usr/bin/env bash
# Install apt packages from a user-owned, cacheable .deb directory with
# stall-resistant retries and per-phase timestamps.
#   usage: APT_DEB_DIR=~/apt-debs [APT_RECOMMENDS=1] apt-install.sh pkg...
# Hosted-runner apt mirrors occasionally stall for minutes (observed 7-14 min
# for a ~400 MB install); a bounded download retry + actions/cache of the .debs
# makes the common case ~20 s and the bad case bounded.
set -uo pipefail
dir=${APT_DEB_DIR:-$HOME/apt-debs}
mkdir -p "$dir/partial"
export DEBIAN_FRONTEND=noninteractive
ts() { while IFS= read -r l; do printf '[%(%T)T] %s\n' -1 "$l"; done; }
log() { printf '[%(%T)T] apt-install: %s\n' -1 "$*"; }

# No docs/man/locales, no fsync: faster unpack.
printf '%s\n' force-unsafe-io 'path-exclude=/usr/share/doc/*' 'path-exclude=/usr/share/man/*' \
  'path-exclude=/usr/share/locale/*' 'path-exclude=/usr/share/info/*' | sudo tee /etc/dpkg/dpkg.cfg.d/90ci >/dev/null
sudo rm -f /var/lib/man-db/auto-update

APT=(-y -o Acquire::Retries=3 -o Acquire::http::Timeout=15 -o Acquire::Languages=none
     -o "Dir::Cache::archives=$dir" -o APT::Sandbox::User=root -o DPkg::Lock::Timeout=120)
[ "${APT_RECOMMENDS:-0}" = 1 ] || APT+=(--no-install-recommends)

log "cached debs: $(ls "$dir"/*.deb 2>/dev/null | wc -l)"
log "update"
# Only Ubuntu packages are needed; unrelated runner repositories can be out of sync.
sudo timeout 120 apt-get update -qq -o Acquire::Retries=3 -o Acquire::http::Timeout=15 \
  -o Dir::Etc::sourcelist="sources.list.d/ubuntu.sources" -o Dir::Etc::sourceparts="-" 2>&1 | ts

ok=0
for attempt in 1 2 3 4; do
  log "download attempt $attempt"
  sudo timeout 150 apt-get install "${APT[@]}" --download-only "$@" 2>&1 | ts
  if [ "${PIPESTATUS[0]}" = 0 ]; then ok=1; break; fi
done
[ "$ok" = 1 ] || { log "download failed"; exit 1; }
log "install"
sudo apt-get install "${APT[@]}" -qq "$@" 2>&1 | ts
rc=${PIPESTATUS[0]}
log "done rc=$rc"
exit "$rc"
