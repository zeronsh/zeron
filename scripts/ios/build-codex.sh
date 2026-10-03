#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PLATFORM="${1:-${PLATFORM_NAME:-iphonesimulator}}"
case "$PLATFORM" in
  iphonesimulator) TARGET=aarch64-apple-ios-sim ;;
  iphoneos) TARGET=aarch64-apple-ios ;;
  *) echo "Unsupported Codex platform: $PLATFORM" >&2; exit 1 ;;
esac
OUT="$ROOT/target/ios-core/$PLATFORM"
mkdir -p "$OUT"
if [[ "${ZERON_SKIP_CODEX:-}" == 1 && -f "$OUT/libzeron_codex_mobile.a" ]]; then exit 0; fi
REV=804d6306e84f570393cdd1eec94c41464f503b1a
CHECKOUT="$ROOT/target/native-agent-spike/codex"
if [[ ! -d "$CHECKOUT/.git" ]]; then
  mkdir -p "$CHECKOUT"
  git -C "$CHECKOUT" init
  git -C "$CHECKOUT" remote add origin https://github.com/openai/codex.git
  git -C "$CHECKOUT" fetch --depth 1 origin "$REV"
  git -C "$CHECKOUT" checkout --detach FETCH_HEAD
fi
[[ "$(git -C "$CHECKOUT" rev-parse HEAD)" == "$REV" ]] || { echo "Wrong Codex revision" >&2; exit 1; }
# Keep this narrow iOS host adaptation reproducible when preparing a fresh checkout.
PATCH="$ROOT/scripts/ios/native-agent/patches/ios-direct-tools.patch"
if git -C "$CHECKOUT" apply --check "$PATCH" 2>/dev/null; then
  git -C "$CHECKOUT" apply "$PATCH"
else
  git -C "$CHECKOUT" apply --reverse --check "$PATCH"
fi
PROFILE=dev
DIRECTORY=debug
if [[ "${CONFIGURATION:-Debug}" == Release ]]; then PROFILE=release; DIRECTORY=release; fi
env -i HOME="$HOME" USER="${USER:-}" TERM="${TERM:-dumb}" \
  PATH="$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  IPHONEOS_DEPLOYMENT_TARGET=26.0 \
  cargo +stable build --locked --manifest-path "$ROOT/crates/codex-mobile/Cargo.toml" \
  --lib --profile "$PROFILE" --target "$TARGET"
cp -p "$ROOT/crates/codex-mobile/target/$TARGET/$DIRECTORY/libzeron_codex_mobile.a" "$OUT/"
