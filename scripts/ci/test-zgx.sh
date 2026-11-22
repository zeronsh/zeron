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
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
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
# (`@account/plugins`), with `plugins.on` off until its fixture says.
plugin_tests() {
  zgx="$1"
  for tests in "$root"/apps/zgx/plugins-builtin/*/tests; do
    ls "$tests"/*.test.luau >/dev/null 2>&1 || continue
    plugin="$(dirname "$tests")"
    id="$(basename "$plugin")"
    stage="$(mktemp -d)"
    cp -R "$root/apps/zgx/ui/." "$stage/"
    rm -rf "$stage/tests"
    mkdir "$stage/tests"
    cp "$tests"/*.test.luau "$stage/tests/"
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
    "$zgx" test ui 2>&1 | tee "$log" | grep -vE '^warning: Luau was built with its asserts' || status=$?
    plugin_tests "$zgx" | tee -a "$log" | grep -vE '^warning: Luau was built with its asserts' || true
    # `FAIL <name> (<file>)`
    failed="$(sed -n 's/^FAIL \(.*\) ([^()]*)$/\1/p' "$log" | sort -u)"
    expected="$(listed luau | sort -u)"
    new="$(comm -23 <(printf '%s\n' "$failed") <(printf '%s\n' "$expected") | sed '/^$/d')"
    fixed="$(comm -13 <(printf '%s\n' "$failed") <(printf '%s\n' "$expected") | sed '/^$/d')"
    ran="$(grep -cE '^(ok|FAIL) ' "$log" || true)"
    echo "zgx test ui: $ran tests, $(printf '%s' "$failed" | grep -c . || true) failed"
    if [ -n "$fixed" ]; then
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
    ;;
  *)
    echo "usage: $0 rust | tests | ui [zgx]" >&2
    exit 2
    ;;
esac
