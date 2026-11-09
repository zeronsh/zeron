#!/usr/bin/env bash
# zgx-bump: move apps/zgx's zgx pin to <rev> (plus the carried commits in
# apps/zgx/zgx-carried.txt) and run the checks that matter after a bump,
# comparing each test failure with the old pin's.
#
#   scripts/zgx-bump.sh main            # zgx main HEAD (+ carried commits)
#   scripts/zgx-bump.sh <sha>           # a zgx commit
#   scripts/zgx-bump.sh --skip ui,unit main
#
# Checks (names for --skip):
#   metadata  cargo metadata --locked for macOS arm64, Windows x64, Linux x64/arm64
#   ui        the zgx CLI built at the pin, then `zgx test ui` (Luau UI suite)
#   unit      cargo test --bin zeron-zgx
#   v2        cargo test --bin zeron-zgx zeron_v2_is_in_step (re-blessed with
#             ZERON_V2_BLESS=1 when it fails; the report names what moved)
#
# The old pin's failures are the baseline: a check passes when the new pin
# fails no test the old one passed. Bumping to the current pin changes
# nothing and runs every check once.
#
# Options:
#   --skip LIST     comma-separated checks to leave out
#   --no-baseline   don't run the checks at the old pin (every failure counts)
#   --no-carried    ignore apps/zgx/zgx-carried.txt
#   --push          push a pin with carried commits to zeronsh/zgx as the tag
#                   zeron-pin/<sha12> (cargo can only fetch what GitHub has);
#                   ZGX_PUSH_URL overrides the URL pushed to
#   --plan-only     resolve the pin and the carried commits, change nothing else
#   --report FILE   write the results as Markdown (the nightly PR's body)
#   --out-env FILE  append key=value results (for $GITHUB_OUTPUT)
#   --work DIR      clones, CLIs and logs (default ~/.cache/zgx-bump, or $ZGX_BUMP_WORK)
#
# Exit status: 0 no regressions, 1 a check regressed, 2 usage or setup error.
set -euo pipefail
export LC_ALL=C

ZGX_URL=https://github.com/zeronsh/zgx
TARGETS="aarch64-apple-darwin x86_64-pc-windows-msvc x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu"
MAX_CARRY_DAYS=2

die() { echo "zgx-bump: $*" >&2; exit 2; }
say() { echo "==> $*" >&2; }

skip=""; baseline=1; use_carried=1; push=0; plan_only=0; report=""; out_env=""
work="${ZGX_BUMP_WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/zgx-bump}"
rev=""
while [ $# -gt 0 ]; do
  case "$1" in
    --skip) skip="$2"; shift 2 ;;
    --no-baseline) baseline=0; shift ;;
    --no-carried) use_carried=0; shift ;;
    --push) push=1; shift ;;
    --plan-only) plan_only=1; shift ;;
    --report) report="$2"; shift 2 ;;
    --out-env) out_env="$2"; shift 2 ;;
    --work) work="$2"; shift 2 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    -*) die "unknown option $1 (--help)" ;;
    *) [ -z "$rev" ] || die "one <rev> only"; rev="$1"; shift ;;
  esac
done
[ -n "$rev" ] || die "usage: scripts/zgx-bump.sh [options] <main|sha> (--help)"
wants() { case ",$skip," in *",$1,"*) return 1 ;; *) return 0 ;; esac; }

root="$(git rev-parse --show-toplevel)"
app="$root/apps/zgx"
carried_file="$app/zgx-carried.txt"
[ -f "$app/Cargo.toml" ] || die "no apps/zgx/Cargo.toml under $root"
[ -z "$report" ] || report="$(cd "$(dirname "$report")" && pwd)/$(basename "$report")"
[ -z "$out_env" ] || touch "$out_env"
mkdir -p "$work"
work="$(cd "$work" && pwd)"

# --- the current pin: every zeronsh/zgx line names the same rev -------------
old_revs="$(grep -o 'github.com/zeronsh/zgx", rev = "[0-9a-f]\{40\}"' "$app/Cargo.toml" | grep -o '[0-9a-f]\{40\}' | sort -u)"
[ -n "$old_revs" ] || die "no zeronsh/zgx rev in apps/zgx/Cargo.toml"
[ "$(printf '%s\n' "$old_revs" | wc -l | tr -d ' ')" = 1 ] || die "the zgx pin lines disagree: $(echo $old_revs)"
old="$old_revs"
pin_lines="$(grep -c "github.com/zeronsh/zgx\", rev = \"$old\"" "$app/Cargo.toml")"

# --- zgx: a blobless clone kept in the work dir ------------------------------
z="$work/zgx"
if [ ! -d "$z/.git" ]; then
  say "cloning zeronsh/zgx into $z"
  git clone -q --filter=blob:none --no-checkout "$ZGX_URL" "$z"
fi
git -C "$z" fetch -q origin main
fetch_rev() { git -C "$z" cat-file -e "$1^{commit}" 2>/dev/null || git -C "$z" fetch -q origin "$1" 2>/dev/null || true; }
if [ "$rev" = main ]; then
  base="$(git -C "$z" rev-parse origin/main)"
else
  fetch_rev "$rev"
  base="$(git -C "$z" rev-parse --verify -q "$rev^{commit}")" || die "zgx has no commit $rev"
fi
fetch_rev "$old"
base_date="$(git -C "$z" log -1 --format=%cs "$base")"
on_main=1; git -C "$z" merge-base --is-ancestor "$base" origin/main || on_main=0

# --- carried commits, replayed onto the base ---------------------------------
# Each line: <sha> #<zgx PR> <YYYY-MM-DD carried since> [note]. A commit zgx
# main already has, whose PR merged, or that no longer applies is dropped
# from the file; the report says which and why.
carry_rows=""; carried_n=0; dropped_n=0; overdue=0; today="$(date -u +%F)"
pin="$base"
if [ "$use_carried" = 1 ] && [ -f "$carried_file" ] && grep -qv '^[[:space:]]*\(#\|$\)' "$carried_file"; then
  # Deterministic replays: the same commits onto the same base give the same
  # sha, so an unchanged night pins the same commit again.
  export GIT_COMMITTER_NAME="zgx-bump" GIT_COMMITTER_EMAIL="zgx-bump@zeron.sh"
  GIT_COMMITTER_DATE="$(git -C "$z" log -1 --format=%cI "$base")"; export GIT_COMMITTER_DATE
  git -C "$z" checkout -q --detach "$base"
  kept="$(mktemp)"; grep '^[[:space:]]*\(#\|$\)' "$carried_file" >"$kept" || true
  while read -r sha pr since note <&3; do
    case "$sha" in ''|'#'*) continue ;; esac
    num="${pr#\#}"
    git -C "$z" fetch -q origin "refs/pull/$num/head" 2>/dev/null || true
    fetch_rev "$sha"
    age="$(python3 -c "import datetime as d,sys; print((d.date.fromisoformat(sys.argv[2])-d.date.fromisoformat(sys.argv[1])).days)" "$since" "$today" 2>/dev/null || echo '?')"
    state=""
    if ! full="$(git -C "$z" rev-parse --verify -q "$sha^{commit}")"; then
      status="dropped: not found in zeronsh/zgx"
    elif git -C "$z" merge-base --is-ancestor "$full" "$base"; then
      status="dropped: zgx main has it"
    elif command -v gh >/dev/null && state="$(gh pr view "$num" -R zeronsh/zgx --json state -q .state 2>/dev/null)" && [ "$state" = MERGED ]; then
      status="dropped: #$num merged"
    elif git -C "$z" cherry HEAD "$full" "$full^" | grep -q '^-'; then
      status="dropped: zgx main has the same change"
    elif git -C "$z" cherry-pick --allow-empty "$full" >/dev/null 2>&1; then
      status="carried"; carried_n=$((carried_n + 1))
      printf '%s %s %s%s\n' "$sha" "$pr" "$since" "${note:+ $note}" >>"$kept"
      if [ "$age" != '?' ] && [ "$age" -gt "$MAX_CARRY_DAYS" ]; then overdue=1; status="carried, **over $MAX_CARRY_DAYS days**"; fi
    else
      git -C "$z" cherry-pick --abort >/dev/null 2>&1 || true
      status="dropped: doesn't apply on zgx main"
    fi
    case "$status" in dropped*) dropped_n=$((dropped_n + 1)) ;; esac
    carry_rows="$carry_rows| \`${sha:0:12}\` | [#$num](https://github.com/zeronsh/zgx/pull/$num) | $since | ${age} d | $status |"$'\n'
    say "carried ${sha:0:12} #$num: $status"
  done 3<"$carried_file" </dev/null
  cp "$kept" "$carried_file"; rm -f "$kept"
  pin="$(git -C "$z" rev-parse HEAD)"
  unset GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL GIT_COMMITTER_DATE
fi

pushed=""
if [ "$pin" != "$base" ]; then
  tag="zeron-pin/${pin:0:12}"
  if git ls-remote --exit-code -q "$ZGX_URL" "refs/tags/$tag" >/dev/null 2>&1; then
    pushed="$tag (already there)"
  elif [ "$push" = 1 ]; then
    git -C "$z" push -q "${ZGX_PUSH_URL:-$ZGX_URL}" "$pin:refs/tags/$tag"
    pushed="$tag (pushed)"
  elif [ "$plan_only" = 1 ]; then
    pushed="$tag (not pushed: --plan-only without --push)"
  else
    die "the pin with carried commits, ${pin:0:12}, isn't on GitHub: rerun with --push (write access to zeronsh/zgx) or --no-carried"
  fi
fi

changed=0; [ "$pin" = "$old" ] || changed=1
if [ "$on_main" = 1 ]; then title="zgx: pin ${pin:0:12} (zgx main $base_date)"; else title="zgx: pin ${pin:0:12} (zgx $base_date, not on main)"; fi
src="zgx $( [ "$on_main" = 1 ] && echo main || echo commit)"
# How far the old pin drifted: its commits zgx main lacks, and main's it lacks.
old_own="$(git -C "$z" rev-list --count "origin/main..$old")"
old_behind="$(git -C "$z" rev-list --count "$old..origin/main")"
say "old pin ${old:0:12}; zgx ${rev} = ${base:0:12} ($base_date); new pin ${pin:0:12} ($carried_n carried, $dropped_n dropped)"

emit() { [ -z "$out_env" ] || printf '%s=%s\n' "$1" "$2" >>"$out_env"; }
emit old "$old"; emit base "$base"; emit base_date "$base_date"; emit pin "$pin"
emit changed "$changed"; emit carried "$carried_n"; emit dropped "$dropped_n"
emit overdue "$overdue"; emit on_main "$on_main"; emit old_own "$old_own"; emit old_behind "$old_behind"; emit title "$title"

plan_md() {
  echo "## $title"
  echo
  if [ "$changed" = 1 ]; then
    echo "Old pin \`${old:0:12}\` → new pin \`${pin:0:12}\`: $src \`${base:0:12}\` ($base_date)$( [ "$carried_n" -gt 0 ] && echo " + $carried_n carried")."
    echo "$pin_lines pin lines in apps/zgx/Cargo.toml and the matching Cargo.lock entries move together."
  else
    echo "The pin stays \`${old:0:12}\` ($src \`${base:0:12}\`, $base_date): nothing to change."
  fi
  [ -z "$pushed" ] || echo "Carried pin tag: \`$pushed\`."
  if [ "$old_own" -gt 0 ]; then
    echo
    echo "> [!WARNING]"
    echo "> The old pin \`${old:0:12}\` was not zgx main: $old_own commits of its own, $old_behind behind main.$( [ "$changed" = 1 ] && echo " Commits of its own that zgx main lacks are dropped by this pin unless carried.")"
  fi
  if [ "$on_main" = 0 ]; then
    echo
    echo "> [!WARNING]"
    echo "> \`${base:0:12}\` is not on zgx main. Zeron pins zgx main (apps/zgx/README.md, \"The zgx pin\")."
  fi
  if [ -n "$carry_rows" ]; then
    echo
    echo "### Carried commits"
    echo
    if [ "$overdue" = 1 ]; then
      echo "> [!CAUTION]"
      echo "> **A carried commit is older than $MAX_CARRY_DAYS days.** Merge its zgx PR (or drop it) so the pin is zgx main again."
      echo
    fi
    echo "| Commit | zgx PR | Carried since | Age | Status |"
    echo "|---|---|---|---|---|"
    printf '%s' "$carry_rows"
  fi
}

if [ "$plan_only" = 1 ]; then
  [ -z "$report" ] || plan_md >"$report"
  plan_md >&2
  exit 0
fi

# --- checks ------------------------------------------------------------------
logs="$work/logs/$(date -u +%Y%m%dT%H%M%S)"
mkdir -p "$logs"
say "logs in $logs"

# Test temp files (zgx test ui leaves a storage folder per test) stay in the work dir.
scratch_tmp() { local d="$work/tmp/$1"; rm -rf "$d"; mkdir -p "$d"; echo "$d"; }

# install_cli <sha>: the zgx CLI built at <sha>, in a root of its own.
install_cli() {
  local sha="$1" cli="$work/cli/$1"
  if [ -x "$cli/bin/zgx" ] && [ -f "$cli/.complete" ]; then echo "$cli/bin"; return 0; fi
  rm -rf "$cli"
  say "building the zgx CLI at ${sha:0:12} (about 10 minutes the first time)"
  # `inspect` only: `i18n` changes number formatting, which the suite reads.
  if ! CARGO_PROFILE_DEV_OPT_LEVEL=2 cargo install -q --debug --locked --git "$ZGX_URL" --rev "$sha" \
      zgx-script --bin zgx --features inspect --root "$cli" --target-dir "$work/cli-target" \
      >"$logs/cli-${sha:0:12}.log" 2>&1; then
    return 1
  fi
  # Usage text and exit 2 means it runs.
  local rc=0; "$cli/bin/zgx" >/dev/null 2>&1 || rc=$?
  [ "$rc" = 2 ] || return 1
  touch "$cli/.complete"; echo "$cli/bin"
}

# Each check writes <dir>/<name>.{status,failures,summary}.
# status: pass | fail | error (didn't run to the end) | skipped
record() { echo "$3" >"$1/$2.status"; echo "$4" >"$1/$2.summary"; }

run_ui() {
  local dir="$1" sha="$2" bin tmp rc=0
  if ! bin="$(install_cli "$sha")"; then
    record "$dir" ui error "zgx CLI didn't build (logs/cli-${sha:0:12}.log)"; : >"$dir/ui.failures"; return
  fi
  tmp="$(scratch_tmp ui)"
  (cd "$app" && ZGX_CHECK=0 TMPDIR="$tmp" PATH="$bin:$PATH" nice zgx test ui) >"$dir/ui.log" 2>&1 || rc=$?
  rm -rf "$tmp"
  perl -ne 'print "$1\n" if /^FAIL (.*) \([^()]*\)$/' "$dir/ui.log" | sort -u >"$dir/ui.failures"
  local line; line="$(grep -E '^[0-9]+ passed, [0-9]+ failed$' "$dir/ui.log" | tail -1 || true)"
  case "$rc" in
    0) record "$dir" ui pass "$line" ;;
    1) record "$dir" ui fail "$line" ;;
    *) record "$dir" ui error "zgx test ui exited $rc" ;;
  esac
}

run_unit() {
  local dir="$1" tmp rc=0
  tmp="$(scratch_tmp unit)"
  (cd "$app" && TMPDIR="$tmp" nice cargo test --locked --bin zeron-zgx) >"$dir/unit.log" 2>&1 || rc=$?
  rm -rf "$tmp"
  perl -ne 'print "$1\n" if /^test (\S+) \.\.\. FAILED/' "$dir/unit.log" | sort -u >"$dir/unit.failures"
  local line; line="$(grep -E '^test result:' "$dir/unit.log" | tail -1 | sed 's/^test result: [A-Z]*\. //; s/; finished.*//' || true)"
  if [ "$rc" = 0 ]; then record "$dir" unit pass "$line"
  elif [ -n "$line" ]; then record "$dir" unit fail "$line"
  else record "$dir" unit error "didn't build or run (unit.log)"; fi
}

run_v2() {
  local dir="$1" bless="$2" rc=0
  : >"$dir/v2.failures"
  (cd "$app" && nice cargo test --locked --bin zeron-zgx zeron_v2_is_in_step) >"$dir/v2.log" 2>&1 || rc=$?
  if [ "$rc" = 0 ]; then record "$dir" v2 pass "in step"; return; fi
  echo zeron_v2_is_in_step_with_the_default >"$dir/v2.failures"
  if [ "$bless" = 1 ]; then
    (cd "$app" && ZERON_V2_BLESS=1 nice cargo test --locked --bin zeron-zgx zeron_v2_is_in_step) >>"$dir/v2.log" 2>&1 || true
    local moved; moved="$(git -C "$root" status --porcelain -- apps/zgx/ui | awk '{print $2}')"
    if [ "$moved" = "apps/zgx/ui/templates/zeron-v2/.base.json" ]; then
      local files; files="$(perl -ne 'print "$1 " if /The default.s \[(.*?)\] changed/' "$dir/v2.log" | tr -d '"')"
      record "$dir" v2 pass "re-blessed: only .base.json moved (${files:-see v2.log})"; : >"$dir/v2.failures"; return
    fi
  fi
  record "$dir" v2 fail "out of step (v2.log)"
}

run_metadata() {
  local dir="$1" failed=""
  for t in $TARGETS; do
    (cd "$app" && cargo metadata --locked --format-version 1 --filter-platform "$t") >/dev/null 2>>"$dir/metadata.log" || failed="$failed $t"
  done
  printf '%s\n' $failed >"$dir/metadata.failures"
  if [ -z "$failed" ]; then record "$dir" metadata pass "4/4 targets resolve"
  else record "$dir" metadata fail "fails for:$failed"; fi
}

run_checks() { # run_checks <dir> <sha> <bless v2?> <with metadata?>
  local dir="$1"; mkdir -p "$dir"
  if [ "$4" = 1 ] && wants metadata; then say "cargo metadata --locked (4 targets)"; run_metadata "$dir"; fi
  if wants ui; then say "zgx test ui at ${2:0:12}"; run_ui "$dir" "$2"; fi
  if wants unit; then say "cargo test --bin zeron-zgx"; run_unit "$dir"; fi
  if wants v2; then say "zeron_v2_is_in_step"; run_v2 "$dir" "$3"; fi
}

if [ "$changed" = 1 ] && [ "$baseline" = 1 ]; then
  say "baseline: the checks at the old pin ${old:0:12}"
  run_checks "$logs/old" "$old" 0 0
fi

if [ "$changed" = 1 ]; then
  say "bumping ${old:0:12} -> ${pin:0:12}"
  perl -pi -e "s/$old/$pin/g" "$app/Cargo.toml" "$app/Cargo.lock"
  # Let cargo settle the lock for whatever the new zgx changed (new or
  # renamed crates, versions); the --locked checks below then hold it to that.
  (cd "$app" && cargo metadata --format-version 1 >/dev/null 2>"$logs/lock.log") || die "cargo can't resolve the new pin (logs: $logs/lock.log)"
  [ "$(grep -c "github.com/zeronsh/zgx\", rev = \"$pin\"" "$app/Cargo.toml")" = "$pin_lines" ] || die "not every pin line moved"
fi
lock_entries="$(grep -c "zeronsh/zgx?rev=$pin#$pin" "$app/Cargo.lock" || true)"
run_checks "$logs/new" "$pin" 1 1
# The same rev is its own baseline.
[ "$changed" = 1 ] || ln -s new "$logs/old"

# --- the report --------------------------------------------------------------
regressed=0
row() { # row <check> <label>
  local c="$1" label="$2" n="$logs/new" o="$logs/old" ns os verdict new_f fixed
  [ -f "$n/$c.status" ] || { echo "| $label | | skipped | – |"; return; }
  ns="$(cat "$n/$c.status")"
  if [ -f "$o/$c.status" ]; then os="$(cat "$o/$c.status"): $(cat "$o/$c.summary")"; else os="–"; fi
  new_f="$( [ -f "$o/$c.failures" ] && comm -13 "$o/$c.failures" "$n/$c.failures" || cat "$n/$c.failures")"
  fixed="$( [ -f "$o/$c.failures" ] && comm -23 "$o/$c.failures" "$n/$c.failures" || true)"
  new_f="$(echo "$new_f" | sed '/^$/d')"; fixed="$(echo "$fixed" | sed '/^$/d')"
  if [ "$ns" = error ] || [ -n "$new_f" ] || { [ "$c" = metadata ] && [ "$ns" != pass ]; }; then
    regressed=1; verdict="❌"
    [ -z "$new_f" ] || verdict="$verdict new failures: $(echo "$new_f" | sed 's/^/`/; s/$/`/' | paste -sd, - | sed 's/,/, /g')"
  elif [ "$ns" = pass ]; then verdict="✅"
  else verdict="✅ no new failures (the same $(wc -l <"$n/$c.failures" | tr -d ' ') as the old pin)"
  fi
  [ -z "$fixed" ] || verdict="$verdict; fixed: $(echo "$fixed" | sed 's/^/`/; s/$/`/' | paste -sd, - | sed 's/,/, /g')"
  echo "| $label | $os | $ns: $(cat "$n/$c.summary") | $verdict |"
}

{
  plan_md
  echo
  echo "### Checks"
  echo
  [ "$changed" = 1 ] || echo "Same rev: the checks ran once, on the current pin."
  [ "$changed" = 1 ] || echo
  echo "| Check | Old pin | New pin | Verdict |"
  echo "|---|---|---|---|"
  row metadata "\`cargo metadata --locked\` (macOS arm64, Windows x64, Linux x64 + arm64)"
  row ui "\`zgx test ui\` (CLI at the pin)"
  row unit "\`cargo test --bin zeron-zgx\`"
  row v2 "\`zeron_v2_is_in_step\`"
  echo
  echo "Lock entries at the pin: $lock_entries. Ran on $(uname -sm)$( [ -n "${GITHUB_RUN_ID:-}" ] && echo ", [run]($GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID)")."
  for c in ui unit; do
    if [ -s "$logs/new/$c.failures" ]; then
      echo
      echo "<details><summary>$c failures at the new pin</summary>"
      echo
      sed 's/^/- `/; s/$/`/' "$logs/new/$c.failures"
      echo
      echo "</details>"
    fi
  done
} >"$logs/report.md"

emit regressed "$regressed"; emit logs "$logs"
[ -z "$report" ] || cp "$logs/report.md" "$report"
cat "$logs/report.md"
exit "$regressed"
