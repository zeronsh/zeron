#!/bin/bash
# usage: probe.sh <label> [ENV=..]...
S=${BENCH_S:?set BENCH_S to a scratch dir with target/, fc/, fm/rec.jsonl}
label=$1; shift
out=$S/fc/probe-$label.log
echo "=== $label $(date +%T) load $(sysctl -n vm.loadavg) $*" >> $out
cd $S/wt-meas/apps/zgx
env PROBE_ON_DEMAND=1 ZGX_PROFILE=1 "$@" nice -n 15 $S/fc/bins/${label%%.*}-probe --ignored --nocapture --exact chat::stream_bench::tests::stream_bench_profile 2>&1 \
  | grep -E "stream_bench_profile|phases:|instructions:|chunk frames|fade-only|per drawn|renderer|test result|panicked|rror" >> $out
