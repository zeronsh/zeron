#!/bin/bash
# Build the Rust mobile core (crates/mobile) for Android and generate its
# Kotlin bindings — the same library the iOS app links.
#
#   scripts/android/build-core.sh [out_dir]
#
# Kotlin generation needs only the host toolchain. The .so build needs the
# Android NDK + cargo-ndk (`cargo install cargo-ndk`,
# `rustup target add aarch64-linux-android x86_64-linux-android`); it is
# skipped with a note when they are missing. (Not yet exercised in CI.)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/android-core}"
mkdir -p "$OUT/kotlin" "$OUT/jniLibs"
cd "$ROOT"

# Prefer an explicit NDK, then the newest SDK ndk/ directory.
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
  if [[ -n "$SDK" && -d "$SDK/ndk" ]]; then
    ANDROID_NDK_HOME="$(find "$SDK/ndk" -mindepth 1 -maxdepth 1 -type d | sort | tail -1)"
    export ANDROID_NDK_HOME
  fi
fi
if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  export ANDROID_NDK_HOME
  export ANDROID_NDK_ROOT="${ANDROID_NDK_ROOT:-$ANDROID_NDK_HOME}"
fi

HOST_PROFILE_DIR="$ROOT/target/mobile"
# Always (re)build the host library: bindings are generated from its metadata,
# and a stale copy silently drops new FFI items. Incremental, so cheap.
cargo build --locked -p zeron-mobile --lib --profile mobile
cargo build --locked -p zeron-mobile --bin uniffi-bindgen --features bindgen --profile mobile
# Windows hosts produce zeron_mobile.dll (no lib prefix), not libzeron_mobile.so.
case "$(uname)" in
  Darwin) HOST_LIB="$HOST_PROFILE_DIR/libzeron_mobile.dylib" ;;
  MINGW*|MSYS*|CYGWIN*) HOST_LIB="$HOST_PROFILE_DIR/zeron_mobile.dll" ;;
  *) HOST_LIB="$HOST_PROFILE_DIR/libzeron_mobile.so" ;;
esac
"$HOST_PROFILE_DIR/uniffi-bindgen" generate --library "$HOST_LIB" --language kotlin \
  --no-format --out-dir "$OUT/kotlin"
echo "kotlin bindings: $OUT/kotlin"

if command -v cargo-ndk >/dev/null && [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  # x86_64 is the Windows emulator ABI; arm64-v8a is the device ABI.
  # ZERON_ANDROID_ABIS narrows the list (build-apk.sh: arm64-v8a unless
  # ZERON_WITH_X86_64=1).
  ABI_ARGS=()
  for abi in ${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}; do ABI_ARGS+=(-t "$abi"); done
  cargo ndk "${ABI_ARGS[@]}" -o "$OUT/jniLibs" \
    build --locked -p zeron-mobile --lib --profile mobile
  echo "jniLibs: $OUT/jniLibs"
else
  echo "note: cargo-ndk / ANDROID_NDK_HOME not found — skipped the .so build"
fi
