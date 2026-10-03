#!/usr/bin/env bash
set -euo pipefail

# Finite offline scenarios exercise production stores/transports and engine
# assembly. Memcheck detects lost allocations; the allocator samples and
# Massif also expose live, reachable retention that Memcheck cannot call a leak.
output=target/memory-evidence
mkdir -p "$output"
git rev-parse HEAD > "$output/commit.txt"
rustc --version > "$output/rustc.txt"
valgrind --version > "$output/valgrind.txt"
cargo build --locked --release -p zeron-engine --example memory-audit
binary=target/release/examples/memory-audit
sha256sum "$binary" > "$output/binary.sha256"
cp "$binary" "$output/memory-audit"
failures=0
for scenario in engine docs journal outbox rpc terminal history sync sync-catchup; do
  if ! timeout 300 valgrind --error-exitcode=97 --leak-check=full \
    --show-leak-kinds=definite,indirect --errors-for-leak-kinds=definite,indirect \
    --track-origins=yes --num-callers=30 --xml=yes \
    --xml-file="$output/memcheck-$scenario.xml" \
    "$binary" "$scenario" > "$output/$scenario.jsonl" 2> "$output/$scenario.stderr"; then
    echo "Memcheck failed: $scenario" >&2
    failures=$((failures + 1))
  fi
done

# Exercise the font lifetime that previously used Box::leak.
cargo test --locked --release -p zeron-text --lib --no-run --message-format=json > "$output/text-build.jsonl"
text_binary=$(python3 -c 'import json,sys; print(next(x["executable"] for x in map(json.loads,sys.stdin) if x.get("reason")=="compiler-artifact" and x.get("profile",{}).get("test") and x.get("executable")))' < "$output/text-build.jsonl")
if ! timeout 120 valgrind --error-exitcode=97 --leak-check=full \
  --show-leak-kinds=definite,indirect --errors-for-leak-kinds=definite,indirect \
  --xml=yes --xml-file="$output/memcheck-fonts.xml" \
  "$text_binary" dropping_font_books_releases_font_bytes > "$output/fonts.txt" 2>&1; then
  echo "Memcheck failed: fonts" >&2
  failures=$((failures + 1))
fi

for scenario in docs sync sync-catchup engine; do
  timeout 300 valgrind --tool=massif --time-unit=B --stacks=no \
    --detailed-freq=1 --max-snapshots=200 --massif-out-file="$output/massif-$scenario.out" \
    "$binary" "$scenario" > "$output/massif-$scenario.jsonl" 2> "$output/massif-$scenario.stderr"
  ms_print "$output/massif-$scenario.out" > "$output/massif-$scenario.txt"
done
if (( failures > 0 )); then
  echo "$failures Memcheck scenarios failed; inspect the uploaded XML" >&2
  exit 1
fi
