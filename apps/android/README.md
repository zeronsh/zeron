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
ui/          Sign-in, sessions, session + composer, new session, search,
             settings
```

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
| `--es route chat:<id>` / `new` / `search` / `settings` | Open a screen at launch |
| `--ez signedout true` | Clear stored credentials |
| `--es wallpaper <path>` / `none` | Set (or clear) the wallpaper from a file the app can read, e.g. `adb push art.jpg /data/local/tmp/ && adb shell run-as sh.zeron.android cp /data/local/tmp/art.jpg files/` then `--es wallpaper /data/user/0/sh.zeron.android/files/art.jpg` |
| `--es wallpaper-effect <none\|dither\|ascii\|halftone\|scanlines>` | Wallpaper effect |
