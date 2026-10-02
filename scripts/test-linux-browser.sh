#!/usr/bin/env bash
# Run the real shell browser fixture with native mouse input on X11 and Wayland.
# Captures stay outside the repository and can be uploaded as PR attachments.
set -euo pipefail
output="${1:?usage: test-linux-browser.sh OUTPUT_DIRECTORY}"
binary="${BROWSER_FIXTURE_BINARY:-target/release/examples/browser-fixture}"
mkdir -p "$output"
output="$(realpath "$output")"
runtime="$(mktemp -d)"
chmod 700 "$runtime"
processes=()
diagnostics() {
  {
    date --iso-8601=ns
    printf 'DISPLAY=%s WAYLAND_DISPLAY=%s XDG_RUNTIME_DIR=%s\n' "${DISPLAY:-}" "${WAYLAND_DISPLAY:-}" "$runtime"
    for pid in "${processes[@]}"; do
      ps -p "$pid" -o pid,ppid,stat,etime,args || true
    done
    timeout 2s xprop -root _NET_SUPPORTING_WM_CHECK _NET_CLIENT_LIST || true
    timeout 2s xwininfo -root -tree || true
  } >>"$output/startup-diagnostics.log" 2>&1
}
cleanup() {
  local result=$?
  if [ "$result" -ne 0 ]; then diagnostics; fi
  for pid in "${processes[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$runtime"
}
trap cleanup EXIT
{
  date --iso-8601=ns
  git -C "$(dirname "$0")" rev-parse HEAD || true
  uname -a
  cat /etc/os-release
  printf 'binary=%s VK_DRIVER_FILES=%s\n' "$binary" "${VK_DRIVER_FILES:-}"
} >"$output/environment.log" 2>&1
Xvfb -displayfd 3 -screen 0 1280x800x24 -ac -nolisten tcp 3>"$runtime/display" >"$output/xvfb.log" 2>&1 &
xvfb_pid=$!
processes+=("$xvfb_pid")
display_number=
for _ in $(seq 1 300); do
  if IFS= read -r display_number <"$runtime/display"; then break; fi
  if ! kill -0 "$xvfb_pid" 2>/dev/null; then
    echo "Xvfb exited before publishing a display" >&2
    cat "$output/xvfb.log" >&2
    exit 1
  fi
  sleep .1
done
if [[ ! "$display_number" =~ ^[0-9]+$ ]]; then
  echo "Xvfb did not publish a valid display within 30 seconds" >&2
  cat "$output/xvfb.log" >&2
  exit 1
fi
export DISPLAY=":$display_number"
export XDG_RUNTIME_DIR="$runtime"
export ZERON_BROWSER_NATIVE_POINTER=1
openbox >"$output/openbox.log" 2>&1 &
openbox_pid=$!
processes+=("$openbox_pid")
# Openbox must own the display before the application requests window mapping.
wm_ready=false
deadline=$((SECONDS + 10))
while (( SECONDS < deadline )); do
  kill -0 "$openbox_pid" 2>/dev/null || { echo 'Openbox exited during startup' >&2; exit 1; }
  wm_window="$(timeout 2s xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | sed -n 's/.*window id # \(0x[0-9a-fA-F]*\).*/\1/p')"
  if [ -n "$wm_window" ] && [ "$wm_window" != 0x0 ]; then
    wm_check="$(timeout 2s xprop -id "$wm_window" _NET_SUPPORTING_WM_CHECK 2>/dev/null || true)"
    if [[ "$wm_check" == *"window id # $wm_window" ]]; then wm_ready=true; break; fi
  fi
  sleep .1
done
if [ "$wm_ready" != true ]; then echo 'Openbox did not become ready within 10 seconds' >&2; exit 1; fi
record_fixture() {
  local mode="$1" capture_window= fixture_pid video_pid
  mkdir -p "$output/$mode"
  # A reused output directory must not acknowledge an earlier fixture process.
  rm -f "$output/$mode/capture-window.txt" "$output/$mode/capture-target.txt"
  "$binary" "$output/$mode" >"$output/$mode/fixture.log" 2>&1 &
  fixture_pid=$!
  processes+=("$fixture_pid")
  local deadline=$((SECONDS + 30))
  while (( SECONDS < deadline )); do
    if [ -f "$output/$mode/capture-window.txt" ]; then
      read -r capture_window <"$output/$mode/capture-window.txt"
      break
    fi
    kill -0 "$fixture_pid" 2>/dev/null || { cat "$output/$mode/fixture.log"; return 1; }
    sleep .1
  done
  if [[ ! "$capture_window" =~ ^[1-9][0-9]*$ ]]; then
    echo "Fixture did not publish a ready capture window within 30 seconds ($mode, pid=$fixture_pid)" >&2
    cat "$output/$mode/fixture.log"
    return 1
  fi
  local codec="${BROWSER_FIXTURE_VIDEO_CODEC:-libx264}"
  local -a encoding=(-c:v "$codec")
  if [ "$codec" = libx264 ]; then
    encoding+=(-preset veryfast -crf 20)
  else
    encoding+=(-q:v 4)
  fi
  ffmpeg -nostdin -loglevel error -y -f x11grab -framerate 30 -window_id "$capture_window" -i "$DISPLAY" \
    "${encoding[@]}" -pix_fmt yuv420p -movflags +faststart \
    "$output/$mode/browser.mp4" >"$output/$mode/video.log" 2>&1 &
  video_pid=$!
  processes+=("$video_pid")
  local result=0
  wait "$fixture_pid" || result=$?
  kill -INT "$video_pid" 2>/dev/null || true
  wait "$video_pid" || true # X11 grab may end when the fixture closes its window.
  cat "$output/$mode/fixture.log"
  [ "$result" -eq 0 ]
  test -f "$output/$mode/result.txt"
  test -f "$output/$mode/linux-results.json"
  ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of json "$output/$mode/browser.mp4"
}
export WAYLAND_DISPLAY= GDK_BACKEND=x11
unset ZERON_BROWSER_CAPTURE_WINDOW
record_fixture x11
export WAYLAND_DISPLAY=zeron-browser-test GDK_BACKEND=wayland
weston --backend=x11 --renderer=pixman --shell=kiosk-shell.so --socket="$WAYLAND_DISPLAY" \
  --idle-time=0 --width=1000 --height=680 >"$output/weston.log" 2>&1 &
weston_pid=$!
processes+=("$weston_pid")
deadline=$((SECONDS + 10))
while (( SECONDS < deadline )); do
  kill -0 "$weston_pid" 2>/dev/null || { echo 'Weston exited during startup' >&2; exit 1; }
  # Weston does not consistently publish _NET_WM_PID. This isolated display
  # has one compositor, so retain its WM_CLASS lookup and require visibility.
  weston_windows="$(timeout 2s xdotool search --onlyvisible --class weston 2>>"$output/weston-discovery.log" || true)"
  capture_window="${weston_windows%%$'\n'*}"
  if [ -S "$runtime/$WAYLAND_DISPLAY" ] && [ -n "$capture_window" ]; then break; fi
  sleep .1
done
if [ ! -S "$runtime/$WAYLAND_DISPLAY" ] || [ -z "$capture_window" ]; then
  echo 'Weston did not publish its socket and visible X11 window within 10 seconds' >&2
  exit 1
fi
export ZERON_BROWSER_CAPTURE_WINDOW="$capture_window"
record_fixture wayland
