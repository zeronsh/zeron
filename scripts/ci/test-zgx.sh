#!/usr/bin/env bash
# The apps/zgx tests CI runs (.github/workflows/zgx-tests.yml), with the known
# failures in scripts/ci/zgx-known-failures.txt let through:
#
#   scripts/ci/test-zgx.sh rust      cargo test --bin zeron-zgx (its unit tests,
#                                    src/plugins/*_tests.rs among them), the
#                                    known ones skipped by exact name
#   scripts/ci/test-zgx.sh tests     cargo test --test '*' (apps/zgx/tests/*.rs,
#                                    the app's integration tests), every binary
#                                    run however others fail (--no-fail-fast),
#                                    the known ones skipped by exact name
#   scripts/ci/test-zgx.sh ui [zgx]  zgx test ui (the Luau UI tests), with the
#                                    zgx CLI given or on PATH, then each of
#                                    Zeron's own plugins' (plugins-builtin/<id>/
#                                    tests, `plugin_tests` below); fails on any
#                                    FAIL not listed, and names listed ones that
#                                    pass
#   scripts/ci/test-zgx.sh plugins [zgx]
#                                    the plugins' Luau UI tests alone, judged
#                                    the same way
#
# For a targeted run, `ui` and `plugins` take (space-separated):
#   ZGX_TESTS="control_open orch_map"   only these test files (stems of
#                                       *.test.luau), in ui/ and the plugins
#   ZGX_PLUGINS="orchestrators"         only these plugins' tests
set -euo pipefail
# Its real path: a plugin's files are where the window finds them only by
# that (zgx resolves a replacement's folder through links).
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
known="$root/scripts/ci/zgx-known-failures.txt"

listed() { sed -n "s/^$1: //p" "$known"; }

# The Luau UI tests of each of Zeron's own plugins that has some
# (apps/zgx/plugins-builtin/<id>/tests/*.test.luau), run by `zgx test` in a
# copy of the app made for them: ui/ without its own tests, the plugin's
# tests in tests/, and the other files of its tests/ (fixtures, worlds)
# beside app.luau, where `mount()` finds fixtures. The plugin is loaded as
# the window loads a bundled built-in, trusted with the app modules its
# manifest names (`app_modules`): its overrides.luau, put in by a zgx.json.
# A test then reaches the plugin's app-side modules as the app does
# (`@account/plugins`), with `plugins.on` off until its fixture says. A
# plugin with an app/ folder (its modules run as the app's code) gets an
# alias of its id naming it, so its tests and fixtures require those
# modules as `@<id>/…`, the very files its index.luau loads (its
# tests/.luaurc says the same to `zgx check`).
plugin_tests() {
  zgx="$1"
  for tests in "$root"/apps/zgx/plugins-builtin/*/tests; do
    ls "$tests"/*.test.luau >/dev/null 2>&1 || continue
    plugin="$(dirname "$tests")"
    id="$(basename "$plugin")"
    if [ -n "${ZGX_PLUGINS:-}" ] && ! grep -qw -- "$id" <<<"$ZGX_PLUGINS"; then
      continue
    fi
    stage="$(mktemp -d)"
    cp -R "$root/apps/zgx/ui/." "$stage/"
    rm -rf "$stage/tests"
    mkdir "$stage/tests"
    copy_tests "$tests" "$stage/tests"
    if ! ls "$stage/tests"/*.test.luau >/dev/null 2>&1; then
      rm -rf "$stage"
      continue
    fi
    if [ -d "$plugin/app" ]; then
      python3 - "$stage/.luaurc" "$id" "$plugin/app" <<'PY'
import json, sys
path, alias, app = sys.argv[1], sys.argv[2], sys.argv[3]
config = json.load(open(path))
config.setdefault("aliases", {})[alias] = app
json.dump(config, open(path, "w"), indent="\t")
PY
    fi
    for file in "$tests"/*.luau; do
      case "$file" in *.test.luau) continue ;; esac
      if [ -e "$stage/$(basename "$file")" ]; then
        echo "FAIL $(basename "$file") is the app's too: name $id's fixture its own ($file)"
        continue
      fi
      cp "$file" "$stage/"
    done
    if [ -f "$plugin/overrides.luau" ]; then
      python3 - "$plugin" "$stage/zgx.json" <<'PY'
import json, sys
plugin, out = sys.argv[1], sys.argv[2]
manifest = json.load(open(f"{plugin}/zeron-plugin.json"))
modules = manifest.get("app_modules") or []
json.dump({
    "expose": {"hooks": {m: "override" for m in modules}},
    "plugins": {manifest["id"]: {
        "path": f"{plugin}/overrides.luau",
        "allow": [f"override-module:{m}" for m in modules],
    }},
}, open(out, "w"), indent=2)
PY
    fi
    echo "== plugins-builtin/$id/tests"
    out="$("$zgx" test "$stage" 2>&1)" || true
    printf '%s\n' "$out"
    # None ran (a zgx.json it refused, a crash): a failure of its own.
    if ! grep -qE '^(ok|FAIL) ' <<<"$out"; then
      echo "FAIL $id's tests didn't run (plugins-builtin/$id/tests)"
    fi
    rm -rf "$stage"
  done
}

# The *.test.luau of folder $1 into $2: those ZGX_TESTS names, or all.
copy_tests() {
  for file in "$1"/*.test.luau; do
    [ -e "$file" ] || continue
    name="$(basename "$file" .test.luau)"
    if [ -n "${ZGX_TESTS:-}" ] && ! grep -qw -- "$name" <<<"$ZGX_TESTS"; then
      continue
    fi
    cp "$file" "$2/"
  done
}

# ui/'s own tests: all of them in place, or ZGX_TESTS's in a copy of ui/.
app_tests() {
  zgx="$1"
  if [ -z "${ZGX_TESTS:-}" ]; then
    "$zgx" test ui
    return
  fi
  stage="$(mktemp -d)"
  cp -R "$root/apps/zgx/ui/." "$stage/"
  rm -rf "$stage/tests"
  mkdir "$stage/tests"
  copy_tests "$root/apps/zgx/ui/tests" "$stage/tests"
  status=0
  if ls "$stage/tests"/*.test.luau >/dev/null 2>&1; then
    "$zgx" test "$stage" || status=$?
  fi
  rm -rf "$stage"
  return "$status"
}

# Judges the log $1 of a run that exited $2 against the known failures.
judge() {
  log="$1"
  status="$2"
  # `FAIL <name> (<file>)`
  failed="$(sed -n 's/^FAIL \(.*\) ([^()]*)$/\1/p' "$log" | sort -u)"
  expected="$(listed luau | sort -u)"
  new="$(comm -23 <(printf '%s\n' "$failed") <(printf '%s\n' "$expected") | sed '/^$/d')"
  fixed="$(comm -13 <(printf '%s\n' "$failed") <(printf '%s\n' "$expected") | sed '/^$/d')"
  ran="$(grep -cE '^(ok|FAIL) ' "$log" || true)"
  echo "zgx test ui: $ran tests, $(printf '%s' "$failed" | grep -c . || true) failed"
  # A targeted run doesn't reach every known failure.
  if [ -n "$fixed" ] && [ -z "${ZGX_TESTS:-}${ZGX_PLUGINS:-}" ]; then
    echo "::notice::known zgx UI failures that passed (delete them from scripts/ci/zgx-known-failures.txt):"
    printf '  %s\n' "$fixed"
  fi
  if [ -n "$new" ]; then
    echo "::error::zgx UI tests that failed and aren't known failures:"
    printf '  %s\n' "$new"
    exit 1
  fi
  # Failed for something other than tests (no tests ran, a crash).
  if [ "$ran" -eq 0 ] || { [ "$status" -ne 0 ] && [ -z "$failed" ]; }; then
    echo "::error::zgx test ui exited $status with no test failures to show for it"
    exit 1
  fi
}

case "${1:-}" in
  rust)
    skips=()
    while IFS= read -r name; do skips+=(--skip "$name"); done < <(listed rust)
    cd "$root/apps/zgx"
    # (`${a[@]+…}`: an empty list under `set -u`, which macOS's bash 3.2
    # calls unbound.)
    exec cargo test --locked --bin zeron-zgx -- --exact ${skips[@]+"${skips[@]}"}
    ;;
  tests)
    skips=()
    while IFS= read -r name; do skips+=(--skip "$name"); done < <(listed tests)
    cd "$root/apps/zgx"
    exec cargo test --locked --no-fail-fast --test '*' -- --exact ${skips[@]+"${skips[@]}"}
    ;;
  ui)
    zgx="${2:-zgx}"
    log="$(mktemp)"
    cd "$root/apps/zgx"
    status=0
    # (The filter's exit kept out: a targeted run whose tests are all the
    # plugins' prints nothing here.)
    app_tests "$zgx" 2>&1 | tee "$log" | { grep -vE '^warning: Luau was built with its asserts' || true; } || status=$?
    plugin_tests "$zgx" | tee -a "$log" | grep -vE '^warning: Luau was built with its asserts' || true
    judge "$log" "$status"
    rm -f "$log"
    ;;
  plugins)
    zgx="${2:-zgx}"
    log="$(mktemp)"
    cd "$root/apps/zgx"
    plugin_tests "$zgx" | tee "$log" | grep -vE '^warning: Luau was built with its asserts' || true
    judge "$log" 0
    rm -f "$log"
    ;;
  *)
    echo "usage: $0 rust | tests | ui [zgx] | plugins [zgx]" >&2
    exit 2
    ;;
esac
