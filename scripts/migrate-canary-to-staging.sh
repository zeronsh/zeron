#!/usr/bin/env bash
# Carry Crew Canary chat history into the running Crew Staging (one-time cutover).
#
# Prerequisites: quit Crew Canary (every window and its background engine), and
# keep a Crew Staging build that has `import-native-profile` running and signed in.
# The script never deletes or edits the Canary profile; it imports a consistent
# copy. Re-running is safe: chats already imported are skipped.
#
# Usage: scripts/migrate-canary-to-staging.sh [--yes]
# Overrides: CANARY_DATA_DIR, STAGING_DATA_DIR, STAGING_COMET, CANARY_PROFILE_KEY,
#            STAGING_IPC_PORT (default 27957).
set -euo pipefail

ASSUME_YES=0
[[ "${1:-}" == "--yes" ]] && ASSUME_YES=1

SOURCE_DATA="${CANARY_DATA_DIR:-$HOME/.crew-upstream-canary}"
STAGING_DATA="${STAGING_DATA_DIR:-$HOME/.crew-staging}"
STAGING_PORT="${STAGING_IPC_PORT:-27957}"

die() { echo "error: $*" >&2; exit 1; }

# 1. The Canary writer must be stopped: no Canary process, no held engine lock.
if pgrep -f "Crew Canary.app/Contents/MacOS/" >/dev/null 2>&1; then
  die "Crew Canary is still running. Quit it (including its background engine) first."
fi
if [[ -e "$SOURCE_DATA/engine.lock" ]] && lsof "$SOURCE_DATA/engine.lock" >/dev/null 2>&1; then
  die "$SOURCE_DATA/engine.lock is held by a running engine."
fi

# 2. Exactly one Canary Scaffold profile, unless selected explicitly.
if [[ -n "${CANARY_PROFILE_KEY:-}" ]]; then
  SOURCE_PROFILE="$SOURCE_DATA/profiles/scaffold/$CANARY_PROFILE_KEY"
else
  shopt -s nullglob
  candidates=("$SOURCE_DATA"/profiles/scaffold/*/docs.sqlite3)
  shopt -u nullglob
  (( ${#candidates[@]} == 1 )) || die "expected one Canary profile, found ${#candidates[@]}; set CANARY_PROFILE_KEY"
  SOURCE_PROFILE="$(dirname "${candidates[0]}")"
fi
[[ -f "$SOURCE_PROFILE/docs.sqlite3" ]] || die "no docs.sqlite3 in $SOURCE_PROFILE"

# 3. The running Crew Staging executable must support the importer.
if [[ -z "${STAGING_COMET:-}" ]]; then
  STAGING_COMET="$(ps -axo command= | sed -n 's#^\(.*Crew Staging\.app/Contents/MacOS/comet\)$#\1#p' | head -1)"
fi
[[ -x "${STAGING_COMET:-}" ]] || die "Crew Staging is not running; start it (or set STAGING_COMET)"
"$STAGING_COMET" import-native-profile --help >/dev/null 2>&1 \
  || die "this Crew Staging build lacks import-native-profile; update Crew Staging first"
# Scrubbed environment: a shell started inside a Crew agent inherits that engine's
# COMET_*/ZERON_*/CREW_* coordinates, which would override the Staging profile's.
comet() {
  env -i HOME="$HOME" USER="${USER:-}" PATH="/usr/bin:/bin:/usr/sbin:/sbin" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}" \
    "$STAGING_COMET" --data-dir "$STAGING_DATA" --ipc-port "$STAGING_PORT" "$@"
}

OUT="$HOME/.crew-migrations/canary-to-staging-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT"
chmod 700 "$HOME/.crew-migrations" "$OUT"
echo "working directory: $OUT"

# 4. Roll-back evidence for the target (online-safe SQLite backup).
for db in "$STAGING_DATA"/profiles/scaffold/*/docs.sqlite3; do
  key="$(basename "$(dirname "$db")")"
  mkdir -p "$OUT/staging-before/$key"
  sqlite3 "$db" ".backup '$OUT/staging-before/$key/docs.sqlite3'"
done
cp "$STAGING_DATA/device-id" "$OUT/staging-before/device-id"

# 5. Consistent source copy: one database file with the WAL folded in.
mkdir -p "$OUT/source"
sqlite3 "$SOURCE_PROFILE/docs.sqlite3" ".backup '$OUT/source/docs.sqlite3'"
[[ -d "$SOURCE_PROFILE/journals" ]] && cp -Rp "$SOURCE_PROFILE/journals" "$OUT/source/journals"
original="$(sqlite3 "$SOURCE_PROFILE/docs.sqlite3" 'select count(*), sum(length(bytes)) from snapshots')"
copied="$(sqlite3 "$OUT/source/docs.sqlite3" 'select count(*), sum(length(bytes)) from snapshots')"
[[ "$original" == "$copied" ]] || die "source copy differs from original ($original vs $copied)"
echo "source copy verified: $copied (documents|bytes)"

UPLOADS_ARGS=()
[[ -d "$SOURCE_PROFILE/uploads" ]] && UPLOADS_ARGS=(--source-uploads "$SOURCE_PROFILE/uploads")
run_import() {
  comet import-native-profile --source-copy "$OUT/source" "${UPLOADS_ARGS[@]}" --writers-stopped "$@"
}
summary() {
  python3 -I -c 'import json,sys
r=json.load(open(sys.argv[1]))
keys=("chatsConsidered","rehomedChats","remoteHostedChats","skippedExisting","importedChats",
      "publishedDocuments","pendingCommandsNeutralized","journalsCopied","spacesImported","spacesMapped")
print("  "+"\n  ".join(f"{k}: {r.get(k)}" for k in keys))
print(f"  unacknowledged: {len(r.get(\"unacknowledged\") or [])}, errors: {len(r.get(\"errors\") or [])}")
for e in (r.get("errors") or [])[:20]: print("   -", e)' "$1"
}

# 6. Plan, confirm, import, then re-plan to prove completeness.
run_import --validate-only >"$OUT/plan.json"
echo "plan:"; summary "$OUT/plan.json"
if (( ! ASSUME_YES )); then
  read -r -p "Import into Crew Staging now? [y/N] " answer
  [[ "$answer" == [yY] ]] || { echo "stopped before writing; plan kept in $OUT"; exit 0; }
fi
status=0
run_import >"$OUT/import.json" || status=$?
echo "import:"; summary "$OUT/import.json"
run_import --validate-only >"$OUT/verify.json"
echo "verify (every chat should now be skipped as existing):"; summary "$OUT/verify.json"
python3 -I -c 'import json,sys
r=json.load(open(sys.argv[1]))
sys.exit(0 if r["skippedExisting"]==r["chatsConsidered"] else 1)' "$OUT/verify.json" \
  || { echo "some chats are not imported yet; fix the errors above and re-run" >&2; status=1; }

cat <<EOF

Done. Reports: $OUT/{plan,import,verify}.json
Do not relaunch Crew Canary: its copies of migrated chats would diverge from Staging.
Worktrees under $SOURCE_DATA/worktrees stay where they are; migrated chats keep using them.
EOF
exit "$status"
