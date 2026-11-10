# Stream-bench measurement scripts (measurement branch only)

Set `BENCH_S` to a scratch dir. It holds `target/` (CARGO_TARGET_DIR), `fc/` (logs, `bins/`) and `fm/rec.jsonl` (the recording: `cargo run --release --features stream-bench --example stream_bench_record -- $BENCH_S/fm/rec.jsonl`). `build.sh` expects this checkout at `$BENCH_S/wt-meas` and a zgx checkout at `$BENCH_S/zgx-meas` when Cargo.toml uses a path patch; here the zgx deps are pinned by rev, so edit build.sh's `cd` to this checkout.

- `build.sh <label>`: release build with `--features stream-bench`; copies `fc/bins/<label>-win` and `-probe`.
- `win.sh <bin> <label> [ENV=..]`: one real window, scratch HOME/ZERON_DATA_DIR, ZGX_CHECK=0, replays `fm/rec.jsonl`, samples `ps cputime` each second, kills the window.
- `rounds.sh <tag> <n> <labels>...`: interleaved window rounds with ZGX_GPU_STATS=1; `gs.py` summarises.
- `probe.sh <label> [ENV=..]`: the headless probe with PROBE_ON_DEMAND=1 ZGX_PROFILE=1 (PROBE_NATIVE_TEXT=1 for real text services; PROBE_VEIL_OFF=1 turns the fade off).
