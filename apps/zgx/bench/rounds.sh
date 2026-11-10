#!/bin/bash
# usage: rounds.sh <tag> <rounds> <label>...   alternating window runs with GPU stats
S=${BENCH_S:?set BENCH_S to a scratch dir with target/, fc/, fm/rec.jsonl}
tag=$1; rounds=$2; shift 2
for r in $(seq 1 $rounds); do
  for l in "$@"; do
    $S/fc/win.sh $S/fc/bins/$l-win $l-$tag$r ZGX_GPU_STATS=1
    echo "$l r$r $(grep -o 'load { [^}]*}' $S/fc/win-$l-$tag$r.log | head -1) | $(grep -o 'outside CPU.*' $S/fc/win-$l-$tag$r.log) | $(grep -o 'CPU main.*' $S/fc/win-$l-$tag$r.log)" >> $S/fc/rounds-$tag.txt
    python3 $S/fc/gs.py $S/fc/win-$l-$tag$r.log.app >> $S/fc/rounds-$tag.txt
  done
done
echo done >> $S/fc/rounds-$tag.txt
