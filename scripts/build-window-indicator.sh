#!/usr/bin/env bash
# Build Zeron's computer-use helpers:
#   zeron-window-indicator (dist/macos/window-indicator/main.swift) keeps a
#     ScreenCaptureKit stream on the window computer use is driving, so macOS
#     shows its capture indicator there;
#   zeron-escape-monitor (dist/macos/escape-monitor/main.swift) is the
#     physical-Escape kill switch for agent input.
# Output: target/computer-helpers/<name>. Prints the binary path of the helper
# named by $1 (default zeron-window-indicator). Rebuilds only stale helpers.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT/target/computer-helpers"
mkdir -p "$OUT_DIR"

build() {
  local name="$1" src="$2"; shift 2
  local out="$OUT_DIR/$name"
  if [[ ! -x "$out" || "$src" -nt "$out" ]]; then
    # desktopIndependentWindow filters need macOS 12.3; the app's floor is
    # 12.0, and the bridge simply skips a helper that cannot run.
    xcrun swiftc -O -target "$(uname -m)-apple-macos12.3" "$@" "$src" -o "$out"
  fi
}

build zeron-window-indicator "$ROOT/dist/macos/window-indicator/main.swift" \
  -framework ScreenCaptureKit -framework CoreMedia -framework AppKit
build zeron-escape-monitor "$ROOT/dist/macos/escape-monitor/main.swift" \
  -framework AppKit -framework CoreGraphics

echo "$OUT_DIR/${1:-zeron-window-indicator}"
