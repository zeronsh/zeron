#!/usr/bin/env bash
# Real X11 windows and an isolated local engine. Never touches a user's GUI.
set -euo pipefail
if [[ "${1:-}" != --inside-xvfb ]]; then
    exec xvfb-run -a -s '-screen 0 1920x1080x24' bash "$0" --inside-xvfb "${1:-target/release/zeron}"
fi
binary=$(realpath "$2")
probe_dir=$(mktemp -d "${TMPDIR:-/tmp}/zeron-multi-window-XXXXXX")
app_pid=
wm_pid=
cleanup() {
    if [[ -n "$app_pid" ]]; then kill "$app_pid" 2>/dev/null || true; fi
    if [[ -n "$wm_pid" ]]; then kill "$wm_pid" 2>/dev/null || true; fi
}
trap cleanup EXIT
export WAYLAND_DISPLAY=
export GDK_BACKEND=x11
export XDG_RUNTIME_DIR="$probe_dir/runtime"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CONFIG_HOME="$probe_dir/config"
export XDG_DATA_HOME="$probe_dir/share"
export XDG_CACHE_HOME="$probe_dir/cache"
export ZERON_DATA_DIR="$probe_dir/data"
export ZERON_IPC_PORT=0
export ZERON_EDGE_URL=http://127.0.0.1:1
export ZERON_WORKOS_CLIENT_ID=
unset ZERON_EDGE_TOKEN ZERON_ORG_ID
openbox >"$probe_dir/wm.log" 2>&1 &
wm_pid=$!
"$binary" >"$probe_dir/app.log" 2>&1 &
app_pid=$!
windows() { xdotool search --all --onlyvisible --pid "$app_pid" --name Zeron 2>/dev/null || true; }
wait_windows() {
    local expected=$1
    for ((attempt=0; attempt<200; attempt++)); do
        mapfile -t current < <(windows)
        if [[ ${#current[@]} == "$expected" ]]; then return; fi
        if ! kill -0 "$app_pid" 2>/dev/null; then cat "$probe_dir/app.log"; return 1; fi
        sleep 0.1
    done
    echo "Expected $expected windows, got ${#current[@]}; logs: $probe_dir" >&2
    return 1
}
wait_windows 1
first=$(windows)
for ((attempt=0; attempt<200; attempt++)); do
    if rg -q 'engine core assembled' "$probe_dir/app.log"; then break; fi
    sleep 0.1
done
rg -q 'engine core assembled' "$probe_dir/app.log"
timeout 20 "$binary" --new-window >"$probe_dir/forward-new.log" 2>&1
wait_windows 2
xdotool windowactivate --sync "$first"
xdotool key --clearmodifiers ctrl+alt+n
wait_windows 3
timeout 20 "$binary" >"$probe_dir/forward-activate.log" 2>&1
wait_windows 3
[[ $(rg -c 'engine core assembled' "$probe_dir/app.log") == 1 ]]
xdotool windowactivate --sync "$first"
xdotool key --clearmodifiers alt+F4
wait_windows 2
kill -0 "$app_pid"
mapfile -t remaining < <(windows)
for window in "${remaining[@]}"; do
    xdotool windowactivate --sync "$window"
    xdotool key --clearmodifiers alt+F4
    sleep 0.2
done
for ((attempt=0; attempt<150; attempt++)); do
    if ! kill -0 "$app_pid" 2>/dev/null; then break; fi
    sleep 0.1
done
if kill -0 "$app_pid" 2>/dev/null; then echo "Last window did not exit; logs: $probe_dir" >&2; exit 1; fi
wait "$app_pid"
app_pid=
echo "PASS: CLI forwarding, Ctrl+Alt+N, three windows, one engine, peer close and last-window exit. Logs: $probe_dir"
