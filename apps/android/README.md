# Zeron for Android

A Jetpack Compose viewport onto the zeron mesh, built on the same Rust mobile
core as the iOS app (`crates/mobile`). **Rust decides what to paint and
where; Kotlin paints, scrolls and handles gestures.** The transcript's text
measurement, markdown layout, prefix-sum virtualization and display lists are
the exact code iOS runs — see [`docs/mobile-rewrite.md`](../../docs/mobile-rewrite.md).

Sessions come with the desktop's developer tools on any computer's
workspace: a file tree with git markers and fuzzy find, a highlighting
editor with conflict-checked saves, Markdown / image / PDF previews, engine
terminals (the desktop's emulator, an extra-keys row), an in-app browser for
workspace HTML and dev-server previews, and Save to Downloads. Also: a model
picker with favorites, Coding agents (install / update / uninstall / sign in
per computer), new projects cloned on a computer, and the Subagents panel.
See [`docs/android.md`](../../docs/android.md).

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
`./gradlew :app:testDebugUnitTest` runs the JVM tests.
`scripts/android/gen-icons.sh` rasterizes the shared tool/file SVG icons
(needs `rsvg-convert`). The Geist fonts are read straight from the iOS app's
`Fonts/` folder, so both platforms measure and draw the same bytes.

## Sounds and haptics

`feedback/` is the app's sensory layer: `Feedback.kt` is the vocabulary
(`Haptic`, `Cue`) every screen speaks, `AndroidFeedback` plays it (platform
haptic constants, `VibrationEffect` primitives, a preloaded `SoundPool`),
`FeedbackGate` decides whether and when (switches, system touch / silent / DND,
foreground only, rate limits), and `FeedbackCompose` holds the hooks
(`LocalFeedback`, `tapAction`, `feedbackAction`, `toggleAction`,
`OpenCloseFeedback`, `feedbackClickable`). Non-Compose code uses
`AppFeedback.current`. Settings, Sounds & haptics has the switches and previews.

```sh
adb logcat -s ZeronFeedback          # one line per haptic / cue, or why it was skipped
adb shell dumpsys vibrator_manager   # what the motor was asked to play
adb shell am broadcast -a sh.zeron.android.DEBUG_EVENT -p sh.zeron.android \
  --es kind done|input|failed [--ez background true]   # debug builds: a session event
python3 scripts/generate-android-sounds.py && python3 scripts/audit-android-sounds.py
```

Full design, tables and policy: [`docs/sound-design/android.md`](../../docs/sound-design/android.md).
All sounds, including the mastered session chimes (also the notification
channel sounds), are generated from `crates/ui/assets/sounds` and the audition set by
`scripts/generate-android-sounds.py` and committed under `app/src/main/res/raw`.

## Layout

```
core/        AppModel (owns CoreClient, republishes snapshots as flows),
             CredentialStore, Fonts + AndroidMeasurer (Minikin fallback
             measurement for glyphs Geist lacks), Agents (harness install
             and agent sign-in over host_call), Favorites, Notifier (Save
             to Downloads progress, session alerts)
feedback/    Haptics and sound: vocabulary, engine, gate, SoundPool bank,
             Compose hooks, settings, session-event policy
design/      ZeronTheme (Material 3 Expressive), transcript palette
transcript/  TranscriptState (layout engine + viewport: anchoring, follow the
             tail), Transcript (virtualized rows over LayoutFrame), RowModel
             (canvas painter for Rust display lists, streaming veil, fades),
             Widgets (copy, disclosures, tool rail, shimmer, images…)
ui/          Sign-in, sessions, session + composer, new session (model
             picker, branch, new projects), search, settings, coding
             agents, subagents
tools/       Developer tools: Workspace (host-RPC file API, streams),
             FilesScreen, FileScreen (editor, Markdown, images, PDF),
             TerminalScreen/TerminalView/Terminals, BrowserScreen + Browser
             (workspace pages, previews), Downloads (Save to Downloads),
             Links (transcript link routing)
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
| `--es route chat:<id>` / `new` / `search` / `settings` / `agents` / `sounds` | Open a screen at launch |
| `--es route subagents:<chat>` / `subagent:<chat>\|<doc>` | Open the Subagents panel / a subagent (Demo: `chat-fanout`) |
| `--es route files:<chat>` / `terminal:<chat>` / `file:<chat>\|<path>` / `browser:<chat>\|<url>` | Open a developer tool at launch (`space:<id>` instead of a chat id for a project) |
| `--es dev-edge <url> --es dev-user <u> --es dev-org <o>` | Debuggable builds: sign in to an `AUTH_MODE=dev` edge (also seven taps on the sign-in mark) |
| `--ez signedout true` | Clear stored credentials |
| `--es wallpaper <path>` / `none` | Set (or clear) the wallpaper from a file the app can read, e.g. `adb push art.jpg /data/local/tmp/ && adb shell run-as sh.zeron.android cp /data/local/tmp/art.jpg files/` then `--es wallpaper /data/user/0/sh.zeron.android/files/art.jpg` |
| `--es wallpaper-effect <none\|dither\|ascii\|halftone\|scanlines>` | Wallpaper effect |
