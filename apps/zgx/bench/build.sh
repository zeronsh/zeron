#!/bin/bash
# usage: build.sh <label>   builds wt-meas against zgx-meas (as checked out), copies binaries to fc/bins/<label>-{win,probe}
set -o pipefail
S=${BENCH_S:?set BENCH_S to a scratch dir with target/, fc/, fm/rec.jsonl}
label=$1
df -h / | tail -1
avail=$(df -g / | tail -1 | awk '{print $4}')
if [ "$avail" -lt 8 ]; then echo "DISK LOW $avail GB, stop"; exit 1; fi
echo "zgx-meas at $(git -C $S/zgx-meas log --oneline -1)"
cd $S/wt-meas/apps/zgx
export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$S/target
nice -n 15 cargo build --release --features stream-bench 2>&1 | grep -E '^(error|warning: unused)|Finished|error\[' | tail -20 || exit 1
cp $S/target/release/zeron-zgx $S/fc/bins/$label-win || exit 1
nice -n 15 cargo test --release --features stream-bench --bin zeron-zgx --no-run 2>&1 | tee $S/fc/build-$label.test.log | grep -E '^error|Finished|Executable' | tail -5
exe=$(grep -o '/[^ ]*deps/zeron_zgx-[0-9a-f]*' $S/fc/build-$label.test.log | tail -1)
cp $exe $S/fc/bins/$label-probe && echo "probe $exe"
du -sh $S/target | tail -1
echo BUILD-DONE $label
