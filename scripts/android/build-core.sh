#!/bin/bash
# Build the Rust mobile core (crates/mobile) for Android and generate its
# Kotlin bindings — the same library the iOS app links.
#
#   scripts/android/build-core.sh [out_dir]
#
# The .so build needs the Android NDK + cargo-ndk (`cargo install cargo-ndk`,
# `rustup target add aarch64-linux-android x86_64-linux-android`).
# `ZERON_ANDROID_ABIS` picks the ABIs (default arm64-v8a x86_64);
# `ZERON_ANDROID_PROFILE` the core's Cargo profile (default `mobile`;
# `mobile-dist` for published builds: thin LTO, stripped, as iOS archives).
# Run by the app's `buildCore` Gradle task (apps/android).
#
# The Kotlin bindings are read out of the Android library just built, by
# UniFFI's generator on its own (crates/mobile-bindgen): the core isn't
# compiled for the host at all. Without the NDK only the bindings are made,
# from a host build of the core.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/android-core}"
mkdir -p "$OUT/kotlin" "$OUT/jniLibs"
cd "$ROOT"

LIB=""
if command -v cargo-ndk >/dev/null && [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  ABIS=()
  for abi in ${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}; do ABIS+=(-t "$abi"); done
  # A published core (mobile-dist) runs Luau (zeron-visual's) without its
  # asserts, a fifth of script time; a development one keeps them.
  PROFILE="${ZERON_ANDROID_PROFILE:-mobile}"
  LUAU=()
  if [[ "$PROFILE" == "mobile-dist" ]]; then LUAU=(LUAU_CXXFLAGS=-DNDEBUG); fi
  # The core links C++ (the terminal emulator and GPU paths): ship the NDK's
  # libc++_shared.so beside it.
  env ${LUAU[@]+"${LUAU[@]}"} cargo ndk "${ABIS[@]}" --link-libcxx-shared -o "$OUT/jniLibs" \
    build --locked -p zeron-mobile --lib --profile "$PROFILE"
  echo "jniLibs: $OUT/jniLibs"
  # With its asserts, each LUAU_ASSERT keeps its Luau source file's path
  # (Luau/Common.h's __FILE__), VM/src/l*.cpp; with NDEBUG, none.
  # (The ABIs just built: another left in jniLibs from an earlier build isn't
  # packaged.)
  if [[ "$PROFILE" == "mobile-dist" ]]; then
    for abi in ${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}; do
      so="$OUT/jniLibs/$abi/libzeron_mobile.so"
      [[ -f "$so" ]] || { echo "error: no $so after the build" >&2; exit 1; }
      if LC_ALL=C grep -aqE 'VM[\\/]src[\\/]l[a-z]+\.cpp' "$so"; then
        echo "error: $so was built with Luau's asserts: build it with LUAU_CXXFLAGS=-DNDEBUG" >&2
        exit 1
      fi
    done
  fi
  first="$(set -- ${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}; echo "$1")"
  LIB="$OUT/jniLibs/$first/libzeron_mobile.so"
else
  echo "note: cargo-ndk / ANDROID_NDK_HOME not found — skipped the .so build"
  cargo build --locked -p zeron-mobile --lib --profile mobile
  LIB="$ROOT/target/mobile/libzeron_mobile.$([[ "$(uname)" == Darwin ]] && echo dylib || echo so)"
fi

cargo build --locked -p zeron-mobile-bindgen --profile mobile
"$ROOT/target/mobile/uniffi-bindgen" generate --library "$LIB" --language kotlin \
  --no-format --out-dir "$OUT/kotlin"
echo "kotlin bindings: $OUT/kotlin"
