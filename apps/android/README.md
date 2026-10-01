# Zeron for Android

A Jetpack Compose client of the Rust mobile core (`crates/mobile`), laid out to follow the iOS app: Rust decides what the transcript paints and where; Android draws that display list, scrolls it, and handles gestures. Like iOS, the phone is a viewport — no agent runs on it.

It connects three ways, chosen under **Settings → Machines**:

- **Zeron Cloud** — the synced workspace, signed in through the same WorkOS flow as iOS (`zeron://callback`).
- **Your computers (SSH)** — SSH direct mode: the phone tunnels to an engine's loopback IPC on your own machine, no account or relay. See [`docs/ssh-direct.md`](../../docs/ssh-direct.md).
- **Demo** — the Rust `DemoHost`, the same offline workspace as the iOS `-demo` launch. Debug builds start here.

## One-command build

From the repo root, with the Android SDK and NDK installed:

```bash
export ANDROID_HOME="$HOME/Android/Sdk"   # or %LOCALAPPDATA%\Android\Sdk on Windows
scripts/android/build-apk.sh
```

The script builds `libzeron_mobile.so` for **arm64-v8a** and **x86_64**, generates the UniFFI Kotlin bindings, applies the small Kotlin 2 compatibility patch, and runs `./gradlew :app:assembleDebug`.

The debug APK is:

```text
apps/android/app/build/outputs/apk/debug/app-debug.apk
```

Install it on an emulator or device:

```bash
adb install -r apps/android/app/build/outputs/apk/debug/app-debug.apk
```

x86_64 is the ABI the Windows Android Studio emulator uses. arm64-v8a is for devices and Apple Silicon emulators.

### What you need

- Rust stable (see `rust-toolchain.toml`) and targets `aarch64-linux-android`, `x86_64-linux-android`
- [`cargo-ndk`](https://github.com/bbqsrc/cargo-ndk) (`cargo install cargo-ndk`)
- Android SDK: `platforms;android-35`, `build-tools;35.0.0`, platform-tools
- Android NDK r27 (`ndk;27.2.12479018` is what this tree was built with)
- JDK 17

`ANDROID_HOME` or `ANDROID_SDK_ROOT` must point at the SDK. The script finds the newest NDK under `$ANDROID_HOME/ndk` when `ANDROID_NDK_HOME` is unset.

### Windows

Use Git Bash or WSL so `scripts/android/build-apk.sh` can run. Point `ANDROID_HOME` at the SDK Android Studio installed, typically `%LOCALAPPDATA%\Android\Sdk`. Install the NDK from Android Studio’s SDK Manager (NDK side by side), then:

```bash
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
export ANDROID_HOME="$LOCALAPPDATA/Android/Sdk"   # Git Bash
scripts/android/build-apk.sh
```

Open `apps/android` in Android Studio and run the `app` configuration on an x86_64 emulator if you would rather not use the script. The native library has to be produced first; `build-apk.sh` copies the stripped `.so` files into `app/src/main/jniLibs/` (that directory is gitignored).

### Deep links for screenshots

The activity reads intent extras, the same idea as the iOS `-route` argument:

```bash
adb shell am start -n sh.zeron.android/.MainActivity \
  --es route settings --es theme dark
```

`route` is `settings`, `search`, `new`, `spaces` (space-filter menu), `session` (with `--es chat <id>`), `machines`, or `signin`. `theme` is `light`, `dark`, or `system`. A fresh install defaults to dark.

## Approximations

iOS uses system Liquid Glass. Recording the Compose hierarchy into a `RenderNode` and blurring it with `RenderEffect` crashes the emulator GPU. Drawing that same hierarchy into a software canvas from the app process is not used either. Capsules sample a 1/8-scale `PixelCopy` of the window, box-blurred on the CPU. If that copy fails, or a previous attempt died before writing its success file in the app cache, the capsule stays a frosted `#1E1E1E` fill with a hairline. On the flat demo backdrop the blurred sample and the fill measure almost the same.

The front page matches the iOS sessions chrome: an “All” space filter, new-session and profile capsules, and no tab bar. Profile opens Settings; Search is a row in Settings. System back pops the session stack, the new-session sheet, sign-in, and Settings. The composer uses the installed IME only. `adjustResize` plus `WindowInsets.ime` unioned with the navigation bar keeps the field and the transcript above the keyboard, including a second IME height. There is no in-app keyboard.

### Fonts and CJK

`design/FontChain.kt` builds one fallback chain per face role: the bundled Geist / Geist Mono asset (Gradle packages `apps/ios/Zeron/Fonts/` directly, so both apps ship the same bytes), then the system Noto Sans CJK SC (looked up through `SystemFonts`, `wght` axis set per weight), then `sans-serif`. The same `Typeface` objects feed the Rust core's text measurer (`AndroidMeasurer`), the transcript canvas, and Compose (`ZeronType` wraps them in an `AndroidFont`), so line breaks and row heights agree with what is drawn. In monospace runs every East Asian Wide / Fullwidth cluster (UAX #11, plus wide emoji) measures and draws as exactly two cells of the mono `0` advance. The demo's CJK session (`chat-cjk`) exercises this.

Chinese input was checked with fcitx5-android (Pinyin) as the system IME: the composer rides on `WindowInsets.ime`, the candidate bar sits inside the IME inset, and a two-line draft grows the field upward.
