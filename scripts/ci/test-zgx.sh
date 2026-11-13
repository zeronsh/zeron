#!/usr/bin/env bash
# The apps/zgx tests CI runs (.github/workflows/zgx-tests.yml), with the known
# failures in scripts/ci/zgx-known-failures.txt let through:
#
#   scripts/ci/test-zgx.sh rust      cargo test --bin zeron-zgx (its unit tests,
#                                    src/plugins/*_tests.rs among them), the
#                                    known ones skipped by exact name
#   scripts/ci/test-zgx.sh ui [zgx]  zgx test ui (the Luau UI tests), with the
#                                    zgx CLI given or on PATH; fails on any FAIL
#                                    not listed, and names listed ones that pass
#
# apps/zgx/tests/*.rs (the app's integration tests) aren't run here.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
known="$root/scripts/ci/zgx-known-failures.txt"

listed() { sed -n "s/^$1: //p" "$known"; }

case "${1:-}" in
  rust)
    skips=()
    while IFS= read -r name; do skips+=(--skip "$name"); done < <(listed rust)
    cd "$root/apps/zgx"
    # (`${a[@]+…}`: an empty list under `set -u`, which macOS's bash 3.2
    # calls unbound.)
    exec cargo test --locked --bin zeron-zgx -- --exact ${skips[@]+"${skips[@]}"}
    ;;
  ui)
    zgx="${2:-zgx}"
    log="$(mktemp)"
    cd "$root/apps/zgx"
    status=0
    "$zgx" test ui 2>&1 | tee "$log" | grep -vE '^warning: Luau was built with its asserts' || status=$?
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
    echo "usage: $0 rust | ui [zgx]" >&2
    exit 2
    ;;
esac
