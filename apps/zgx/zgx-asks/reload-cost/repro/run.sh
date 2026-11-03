#!/bin/zsh
# run.sh [zgx-run]: each case below, and how many times the app loaded for it.
R=${0:a:h}; RUN=${1:-zgx-run}; LOG=$R/run.log
[ -d $R/app ] || python3 $R/make_app.py
cd $R && ZGX_CHECK=0 ZGX_RENDER_WHEN_COVERED=1 "$RUN" app/app.luau > $LOG 2>&1 &
pid=$!; sleep 6
loads() { grep -c '^LOADED' $LOG; }
step() { local before=$(loads); eval "$2"; sleep 3; echo "$(( $(loads) - before )) reload(s)  $1"; }
step "touch mods/m7.luau (same bytes)"            "touch app/mods/m7.luau"
step "a new file in mods/ (not required)"          "echo x > app/mods/zz_new.txt"
step "control: m7.luau's bytes change"             "echo '-- changed' >> app/mods/m7.luau"
rm -f app/mods/zz_new.txt
# Where a reload's time goes: 10 same-byte touches, sampled.
( for i in $(seq 1 10); do touch app/mods/m7.luau; sleep 2; done ) &
loop=$!
sample $pid 21 1 -file $R/sample.txt >/dev/null 2>&1; wait $loop
total=$(grep -E '[0-9]+ .*reload_if_changed' $R/sample.txt | sed -E 's/^[ +!:|]*([0-9]+).*/\1/' | sort -rn | head -1)
compile=$(grep -E '[0-9]+ luau_compile' $R/sample.txt | sed -E 's/^[ +!:|]*([0-9]+).*/\1/' | awk '{s+=$1} END{print s+0}')
echo "sampled: reload_if_changed ${total:-0} samples, luau_compile ${compile} of them"
kill $pid
