#!/bin/bash
# usage: win.sh <binary> <label> [ENV=..]...  one real window, scratch HOME/DATA, ZGX_CHECK=0
S=${BENCH_S:?set BENCH_S to a scratch dir with target/, fc/, fm/rec.jsonl}
bin=$1; label=$2; shift 2
out=$S/fc/win-$label.log; : > $out
echo "before: $(rtk proxy ps -axo pid,command | grep -i zeron-zgx | grep -v grep | grep -c "$S/fc/bins")" >> $out
T=$(mktemp -d $S/fc/winhome.XXXX); mkdir -p $T/home $T/data
echo "load $(sysctl -n vm.loadavg)" >> $out
env "$@" HOME=$T/home ZERON_DATA_DIR=$T/data ZGX_CHECK=0 ZERON_STREAM_BENCH=$S/fm/rec.jsonl ZERON_STREAM_BENCH_QUIT=1 \
  $bin > $out.app 2>&1 &
pid=$!
echo "pid $pid" >> $out
for i in $(seq 1 120); do
  kill -0 $pid 2>/dev/null || break
  echo "$(python3 -c 'import time;print(f"{time.time():.2f}")') $(ps -o cputime= -p $pid)" >> $out
  sleep 1
done
kill $pid 2>/dev/null; sleep 1; kill -9 $pid 2>/dev/null
rm -rf $T
echo "load $(sysctl -n vm.loadavg)" >> $out
echo "after: $(rtk proxy ps -axo pid,command | grep -i zeron-zgx | grep -v grep | grep -c "$S/fc/bins")" >> $out
echo done >> $out
python3 $S/fm/slope.py $out >> $out
grep 'stream-bench: end' $out.app >> $out
