#!/usr/bin/env bash
# Native, engine-free GPUI rendering. Evidence remains outside the repository.
set -euo pipefail
output="${1:?usage: test-remote-desktop.sh OUTPUT_DIRECTORY}"
binary="${RDP_FIXTURE_BINARY:-target/release/examples/remote-desktop-fixture}"
mkdir -p "$output"
output="$(cd "$output" && pwd)"
if [[ "$(uname -s)" == Darwin ]]; then
  "$binary" "$output" >"$output/fixture.log" 2>&1
  test -f "$output/result.txt"
  exit
fi
runtime="$(mktemp -d)"
processes=()
cleanup() {
  for pid in "${processes[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$runtime"
}
trap cleanup EXIT
Xvfb -displayfd 3 -screen 0 1280x900x24 -ac -nolisten tcp 3>"$runtime/display" >"$output/xvfb.log" 2>&1 &
processes+=("$!")
number=
for _ in $(seq 1 100); do
  if IFS= read -r number <"$runtime/display" && [[ "$number" =~ ^[0-9]+$ ]]; then break; fi
  sleep .1
done
[[ "$number" =~ ^[0-9]+$ ]]
export DISPLAY=":$number" XDG_RUNTIME_DIR="$runtime" WAYLAND_DISPLAY=
openbox >"$output/openbox.log" 2>&1 &
processes+=("$!")
mkdir -p "$output/x11"
"$binary" "$output/x11" >"$output/x11/fixture.log" 2>&1
test -f "$output/x11/result.txt"
export WAYLAND_DISPLAY=zeron-rdp-test
weston --backend=x11 --renderer=pixman --shell=kiosk-shell.so --socket="$WAYLAND_DISPLAY" \
  --idle-time=0 --width=800 --height=600 >"$output/weston.log" 2>&1 &
processes+=("$!")
for _ in $(seq 1 100); do [[ -S "$runtime/$WAYLAND_DISPLAY" ]] && break; sleep .1; done
[[ -S "$runtime/$WAYLAND_DISPLAY" ]]
export ZERON_RDP_CAPTURE_WINDOW="$(xdotool search --class weston | head -1)"
mkdir -p "$output/wayland"
"$binary" "$output/wayland" >"$output/wayland/fixture.log" 2>&1
test -f "$output/wayland/result.txt"
