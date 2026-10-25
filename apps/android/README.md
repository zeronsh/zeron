# Zeron for Android

A Jetpack Compose viewport onto the zeron mesh, built on the same Rust mobile
core as the iOS app (`crates/mobile`). **Rust decides what to paint and
where; Kotlin paints, scrolls and handles gestures.** The transcript's text
measurement, markdown layout, prefix-sum virtualization and display lists are
the exact code iOS runs — see [`docs/mobile-rewrite.md`](../../docs/mobile-rewrite.md).

The UI is Material 3 Expressive (`MaterialExpressiveTheme`, expressive motion,
flexible app bars, shape-morphing loading indicators, connected button groups,
segmented lists) themed with Zeron's own palette and Geist type.

## Build & run

Requires JDK 21, the Android SDK (platform 37, NDK 29.0.14206865), and Rust
with `cargo install cargo-ndk` and
`rustup target add aarch64-linux-android x86_64-linux-android`.

```sh
cd apps/android
./gradlew :app:installDebug
```

The `buildCore` task runs `scripts/android/build-core.sh`, which builds
`crates/mobile` for Android (`jniLibs`) and generates its Kotlin bindings into
`target/android-core/`. `-PzeronSkipCore` reuses the last build while
iterating on Kotlin; `ZERON_ANDROID_ABIS=arm64-v8a` builds one ABI.
`scripts/android/gen-icons.sh` rasterizes the shared tool/file SVG icons
(needs `rsvg-convert`). The Geist fonts are read straight from the iOS app's
`Fonts/` folder, so both platforms measure and draw the same bytes.

## Releases

`.github/workflows/android.yml` builds a release APK (arm64, the core at the
`mobile-dist` profile) whenever the app or the Rust core changes: pull requests
get it as a workflow artifact, and pushes to `main` publish it to
`zeron.sh/releases/zeron-android-arm64.apk` — the website's Android download —
with a copy per build (`zeron-<version>-<build>-android-arm64.apk`) and `.sha256`s.

The version is the workspace's (`Cargo.toml`); the build number grows with each
run. Publishing needs the upload key in repository secrets
(`ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`,
`ANDROID_KEY_PASSWORD`) so each build installs over the last; without them the
APK is debug-signed and not published. Locally, `ZERON_ANDROID_KEYSTORE` (and
the three variables beside it) signs `assembleRelease` the same way.

## Layout

```
core/        AppModel (owns CoreClient, republishes snapshots as flows),
             CredentialStore, Fonts + AndroidMeasurer (Minikin fallback
             measurement for glyphs Geist lacks)
design/      ZeronTheme (Material 3 Expressive), transcript palette
transcript/  TranscriptState (layout engine + viewport: anchoring, follow the
             tail), Transcript (virtualized rows over LayoutFrame), RowModel
             (canvas painter for Rust display lists, streaming veil, fades),
             Widgets (copy, disclosures, tool rail, shimmer, images…)
terminal/    TerminalView (a shell on the chat's host: paints the Rust
             emulator's frames, IME and hardware keys, scrollback fling,
             pinch to resize, selection), TerminalGlyphs (box drawing, blocks)
ui/          Sign-in and the welcome pages, sessions (a Material 3 bottom
             app bar with the New session FAB), session + composer,
             companions (subagents, side chats, terminals: chips above the
             composer and a sheet), subagent transcripts, terminal + key bar,
             new session (Cloud too), search, settings, and Cloud: Providers,
             an agent's accounts, Cloud, Billing, GitHub, Cloud projects.
             TabletShell: large screens (≥ 700dp wide) put the sessions
             sidebar beside the open session and its companions in an
             inspector column (iPad's split); Settings is a dialog there.
```

The core links C++ (`libc++_shared.so` ships beside it: `build-core.sh` passes
`--link-libcxx-shared` to cargo-ndk).

## Launch extras

Mirrors the iOS launch arguments:

```sh
adb shell am start -n sh.zeron.android/.MainActivity \
  --ez demo true --es route chat:chat-veil
```

| Extra | Effect |
| --- | --- |
| `--ez demo true` | Offline demo workspace (Rust `DemoHost`) |
| `--ez fast true` / `--ez longreply true` | Demo stream speed / reply length |
| `--ez big true` / `--ez huge true` | Demo transcripts with 120 / 600 turns |
| `--es route chat:<id>` / `new` / `search` / `settings` (`more`) / `archived` | Open a screen at launch (with nobody signed in, on the demo) |
| `--es route cloud` / `cloud:billing` / `cloud:repos` / `billing` / `providers[:cloud\|:<device id>]` / `accounts:cloud:<harness>` / `accounts:device:<id>:<harness>` / `new:cloud` / `welcome[:<step>]` | Cloud pages, a new session set to Cloud, the welcome pages |
| `--ez cloud true` | Offer Cloud in the demo (its pages read the demo's Cloud) |
| `--es companion companions` / `terminal` / `side-chat:<id>` / `subagent:<tool id>` / `model-picker` | With `route chat:<id>`, open the companions sheet, a terminal, a side chat, a subagent or the model picker over it (`model-picker` also works with `route new`) |
| `--ez terminal-demo true` | The opened terminal types a short script (screenshots) |
| `--ez signedout true` | Clear stored credentials |
| `--es wallpaper <path>` / `none` | Set (or clear) the wallpaper from a file the app can read, e.g. `adb push art.jpg /data/local/tmp/ && adb shell run-as sh.zeron.android cp /data/local/tmp/art.jpg files/` then `--es wallpaper /data/user/0/sh.zeron.android/files/art.jpg` |
| `--es wallpaper-effect <none\|dither\|ascii\|halftone\|scanlines>` | Wallpaper effect |
