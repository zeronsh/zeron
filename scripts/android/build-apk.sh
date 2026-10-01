#!/bin/bash
# One-command debug APK (x86_64 + arm64-v8a), demo mode by default.
#
#   scripts/android/build-apk.sh
#
# Signed with the default Android debug key.
#
# Requires: Rust, cargo-ndk, the Android SDK (platforms;android-35,
# build-tools;35.0.0) and NDK r27. Set ANDROID_HOME (or ANDROID_SDK_ROOT).
# On Windows, run this from Git Bash or WSL; see apps/android/README.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
if [[ -z "$SDK" ]]; then
  echo "error: set ANDROID_HOME to the Android SDK" >&2
  exit 1
fi
export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"

export ZERON_REQUIRE_NDK=1
"$ROOT/scripts/android/build-core.sh" "$ROOT/target/android-core"

APP_JNI="$ROOT/apps/android/app/src/main/jniLibs"
rm -rf "$APP_JNI"
mkdir -p "$APP_JNI"
NDK="${ANDROID_NDK_HOME:-}"
if [[ -z "$NDK" && -d "$SDK/ndk" ]]; then
  NDK="$(find "$SDK/ndk" -mindepth 1 -maxdepth 1 -type d | sort | tail -1)"
fi
STRIP=""
if [[ -n "$NDK" ]]; then
  STRIP="$(find "$NDK/toolchains/llvm/prebuilt" -name llvm-strip \( -type f -o -type l \) 2>/dev/null | head -1 || true)"
fi
for abi in arm64-v8a x86_64; do
  src="$ROOT/target/android-core/jniLibs/$abi/libzeron_mobile.so"
  if [[ ! -f "$src" ]]; then
    echo "error: missing $src — cargo-ndk did not produce $abi" >&2
    exit 1
  fi
  mkdir -p "$APP_JNI/$abi"
  cp "$src" "$APP_JNI/$abi/libzeron_mobile.so"
  if [[ -n "$STRIP" ]]; then
    "$STRIP" --strip-all "$APP_JNI/$abi/libzeron_mobile.so" || true
  fi
done

KOTLIN_SRC="$ROOT/target/android-core/kotlin/uniffi/zeron_core/zeron_core.kt"
if [[ -f "$KOTLIN_SRC" ]]; then
  mkdir -p "$ROOT/apps/android/app/src/main/java/uniffi/zeron_core"
  cp "$KOTLIN_SRC" "$ROOT/apps/android/app/src/main/java/uniffi/zeron_core/zeron_core.kt"
  python3 "$ROOT/scripts/android/patch-uniffi-kotlin.py" "$ROOT/apps/android/app/src/main/java/uniffi/zeron_core/zeron_core.kt"
fi

printf 'sdk.dir=%s\n' "$SDK" > "$ROOT/apps/android/local.properties"
cd "$ROOT/apps/android"
if [[ ! -x ./gradlew ]]; then
  if command -v gradle >/dev/null; then
    gradle wrapper --gradle-version 8.11.1
  else
    echo "error: ./gradlew is missing and gradle is not on PATH" >&2
    exit 1
  fi
fi
chmod +x ./gradlew
./gradlew :app:assembleDebug --no-daemon
APK="$ROOT/apps/android/app/build/outputs/apk/debug/app-debug.apk"
echo "APK: $APK"
