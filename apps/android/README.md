# Zeron for Android

A Jetpack Compose viewport onto the zeron mesh, built on the same Rust mobile
core as the iOS app (`crates/mobile`). **Rust decides what to paint and
where; Kotlin paints, scrolls and handles gestures.** The transcript's text
measurement, markdown layout, prefix-sum virtualization and display lists are
the exact code iOS runs — see [`docs/mobile-rewrite.md`](../../docs/mobile-rewrite.md).

The phone is a regular Zeron device: the `:runtime` module boots a proot
Linux guest running the Zeron engine, which owns the account and runs coding
agents **on the phone**, and the app is that device's viewer — it signs in
through the engine, shares its device id and takes its edge bearer. Other
devices start sessions on the phone and the phone on them; see
[`docs/android.md`](../../docs/android.md). First run offers Sign in,
Continue without an account (the phone's local workspace), and an offline
demo.

Sessions come with the desktop's developer tools, on any device's
workspace: a file tree with git markers and fuzzy find, a highlighting
editor with conflict-checked saves, Markdown / image / PDF previews, engine
terminals (the desktop's emulator, an extra-keys row), an in-app browser for
workspace HTML and dev-server previews, and Save to Downloads for a file, a
folder or a whole project — see docs/android.md § Developer tools.

The UI is Material 3 Expressive (`MaterialExpressiveTheme`, expressive motion,
flexible app bars, shape-morphing loading indicators, connected button groups,
segmented lists) themed with Zeron's own palette and Geist type.

## Build & run

Requires JDK 21, the Android SDK (platform 37, NDK 29.0.14206865), and Rust
with `cargo install cargo-ndk cargo-zigbuild`, zig, `patchelf`, and
`rustup target add aarch64-linux-android x86_64-linux-android
aarch64-unknown-linux-musl x86_64-unknown-linux-musl`.

```sh
cd apps/android
./gradlew :app:installDebug
```

The `buildCore` task runs `scripts/android/build-core.sh`, which builds
`crates/mobile` for Android (`jniLibs`) and generates its Kotlin bindings into
`target/android-core/`. `-PzeronSkipCore` reuses the last build while
iterating on Kotlin; `ZERON_ANDROID_ABIS=arm64-v8a` builds one ABI.
The on-device engine's payload — proot and its libs, the static musl engine,
the Alpine rootfs — comes from `scripts/android/fetch-proot.sh`,
`build-engine.sh` and `fetch-rootfs.sh` (into `target/android-runtime/`),
which the build runs only while their outputs are missing
(`-PzeronSkipRuntime` never runs them). `:app` packages them with
`useLegacyPackaging = true` and unstripped; see docs/android.md § Build.
`./gradlew :app:testDebugUnitTest` runs the JVM tests.
`scripts/android/gen-icons.sh` rasterizes the shared tool/file SVG icons
(needs `rsvg-convert`). The Geist fonts are read straight from the iOS app's
`Fonts/` folder, so both platforms measure and draw the same bytes.

## Layout

```
core/        AppModel (the device's CoreClient built from its engine via
             EngineLink, sign-in through the engine, snapshots as flows),
             Devices (identity key, machine grouping), Fonts +
             AndroidMeasurer (Minikin fallback measurement for glyphs Geist
             lacks), PhoneEngine (the :runtime engine as the UI sees it),
             Agents (harness install and agent sign-in over host_call),
             Notifier (local session and file-transfer notifications),
             Transfers + TransferCenter (device file transfer: polling,
             Downloads copies, the share outbox)
design/      ZeronTheme (Material 3 Expressive), transcript palette
transcript/  TranscriptState (layout engine + viewport: anchoring, follow the
             tail), Transcript (virtualized rows over LayoutFrame), RowModel
             (canvas painter for Rust display lists, streaming veil, fades),
             Widgets (copy, disclosures, tool rail, shimmer, images…)
ui/          First run, sessions, session + composer, new session, search,
             settings, This phone (engine page), coding agents, transfers,
             the Share to Zeron sheet (ShareActivity)
tools/       Developer tools: Workspace (host-RPC file API, streams),
             FilesScreen, FileScreen (editor, Markdown, images, PDF),
             TerminalScreen/TerminalView/Terminals, BrowserScreen + Browser
             (workspace pages, previews), Downloads (Save to Downloads),
             Links (transcript link routing)
```

`../runtime` (`sh.zeron.runtime`) is the on-device engine: guest bootstrap,
`RuntimeService`, health and logs, behind `ZeronRuntime.get(context)`.

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
| `--ez local true` | Skip the first-run screen: continue without an account |
| `--es server <url> --es server-token <t>` | Developer custom server (`zeron local-edge`); `--es server none` clears it |
| `--es route chat:<id>` / `new` / `search` / `settings` / `engine` / `agents` / `transfers` | Open a screen at launch |
| `--es route files:<chat>` / `terminal:<chat>` / `file:<chat>\|<path>` / `browser:<chat>\|<url>` | Open a developer tool at launch (`space:<id>` instead of a chat id for a project) |
| `--ez signedout true` | Back to the first-run screen (the engine keeps its sign-in) |
| `--es wallpaper <path>` / `none` | Set (or clear) the wallpaper from a file the app can read, e.g. `adb push art.jpg /data/local/tmp/ && adb shell run-as sh.zeron.android cp /data/local/tmp/art.jpg files/` then `--es wallpaper /data/user/0/sh.zeron.android/files/art.jpg` |
| `--es wallpaper-effect <none\|dither\|ascii\|halftone\|scanlines>` | Wallpaper effect |
