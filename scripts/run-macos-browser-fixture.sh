#!/bin/bash
# Use the release Info.plist: bare executables do not enforce the app's ATS policy.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="$1"
shift
BUNDLE="$(mktemp -d "${TMPDIR:-/tmp}/zeron-browser-fixture.XXXXXX")/Fixture.app"
trap 'rm -r "$(dirname "$BUNDLE")"' EXIT
mkdir -p "$BUNDLE/Contents/MacOS"
cp "$BINARY" "$BUNDLE/Contents/MacOS/fixture"
DAWN="$(dirname "$BINARY")/libwebgpu_dawn.dylib"
if [ -f "$DAWN" ]; then
  cp "$DAWN" "$BUNDLE/Contents/MacOS/"
fi
sed 's/__VERSION__/1.0.0/g' "$ROOT/dist/macos/Info.plist" > "$BUNDLE/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleExecutable fixture' "$BUNDLE/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleIdentifier sh.zeron.browser-fixture' "$BUNDLE/Contents/Info.plist"
"$BUNDLE/Contents/MacOS/fixture" "$@"
