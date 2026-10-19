# Zeron for iOS

A UIKit viewport onto the zeron mesh, built on the Rust mobile core. The phone
is a **peer device**: it mirrors the workspace registry, joins per-chat session
rooms, and drives remote engines through the durable command ledger. No agent
runs on the phone.

**Rust decides what to paint and where; Swift paints, scrolls and handles
gestures.** Everything in Rust is platform-neutral — the future Android app
links the same library. See [`docs/mobile-rewrite.md`](../../docs/mobile-rewrite.md).

## Build & run

Requires Xcode 26+ and a Rust toolchain with the iOS targets:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cd apps/ios
xcodebuild -project Zeron.xcodeproj -scheme Zeron \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' \
  CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- build
```

Keep signing enabled for simulator builds. The app and widget extension
need their App Group entitlements to share widget snapshots. An unsigned
build can open and sign in while every widget remains empty or signed out.

The Zeron target's **Rust core** build phase runs `scripts/ios/build-core.sh`,
which builds `crates/mobile` for the active platform (always optimized — the
`mobile` cargo profile) and refreshes the committed UniFFI bindings in
`Zeron/Core/Generated/`. `ZERON_SKIP_CORE=1` reuses the last built library
when iterating on Swift only.

## Layout

```
Zeron/
  App/         AppDelegate, SceneDelegate, AppModel (owns CoreClient; maps
               workspace snapshots to view models; Keychain credentials)
  Core/        Generated UniFFI bindings; Fonts (the exact bytes Rust measures,
               registered with CoreText) + CoreText fallback measurer
  Design/      Palette (Zeron color roles → light/dark), Glass helpers,
               cell-grid status glyph, harness brand marks
  Transcript/  TranscriptListView (virtualized scroll host over LayoutFrame),
               RowView/RowModel (CoreText painter for Rust display lists,
               streaming veil), FrameRelay
  Session/     SessionViewController, SessionSource (Core/Fixture), new-session
               canvas
  Composer/    ComposerBar (glass capsule ⇄ card with inline context chips;
               send/queue/steer/stop), question and queue panels, attachments
  Threads/     Sessions (foldable sidebar sections)/folder/search lists,
               cells, new-project folder browser
  Shell/       Tab bar (Sessions, Settings, search; "New session" accessory
               with live summary), sign-in
  Debug/       Transcript lab + hitch meter
ZeronLiveActivity/  Widget extension: the Sessions home/Lock Screen widget and
               the voice call's Live Activity (Dynamic Island + Lock
               Screen), with the orb's states exported from zeron-orb as
               vector assets
ZeronShared/   Compiled into the app and the extension: the activity's
               attributes, its mute/end App Intents, and the widget snapshot
               (app group file + `zeron://` links)
```

### Transcript pipeline

1. `zeron-client` applies session-doc updates incrementally (O(changed
   entries)) and publishes immutable snapshots.
2. `TranscriptView.attach(client, chatId)` subscribes **in Rust**; the layout
   thread turns entries into rows (one per markdown block / user message / tool
   group), reparses streaming markdown incrementally, and measures every row
   with `zeron-text` at the viewport width.
3. Each pass publishes a `LayoutFrame`: exact heights + prefix-sum offsets.
   `rowsIn(y0, y1)` is a binary search; `display(i)` builds a display list
   (text runs at exact positions, boxes, links, scrollers, widgets).
4. `TranscriptListView` positions reusable `RowView`s at those offsets and
   paints runs with CoreText at the Rust coordinates — measurement and
   rendering can't disagree. Display models for rows beyond the viewport are
   prefetched off the main thread.
5. Agent visuals (`show_visual` Luau scripts) get a worker thread each
   (`crates/mobile/src/visual`): it runs the script, lays the node tree out,
   draws canvases and charts as vector paint, renders scenes and shaders with
   wgpu, and reports its height back to the row. `VisualView` paints what
   arrives (rasterized on the worker thread) and forwards touches; the demo's
   "Agent visuals" chat (`-demo -route chat:chat-visuals`) has one of each.

## Launch arguments

| Arg | Effect |
| --- | --- |
| `-demo` | Offline demo workspace (Rust `DemoHost`: registry, docs, streaming replies) |
| `-fast` / `-longreply` | Demo stream speed / reply length |
| `-big` / `-huge` | Demo transcripts with 120 / 600 turns |
| `-route chat:<id>` / `new` / `more` / `search` | Open a screen at launch |
| `-signedout` | Clear stored credentials |
| `-dev <userId> <orgId> [-edge <url>]` | Dev bearer against an `AUTH_MODE=dev` edge (e.g. `wrangler dev`); not persisted |
| `-harness <id>` | Default harness for new sessions (`mock` for live-stack tests) |
| `-bench` (with `-lab`) | Display-link fling benchmark; writes `Documents/bench.json` |
| `-measureopen` | Session open latency; writes `Documents/open.json` |
| `-lab [-turns N] [-autostream] [-autoscroll] [-top] [-meter]` | Transcript lab over fixture markdown, with an on-screen hitch meter |

## Tests

```sh
# Rust
cargo test -p zeron-text -p zeron-markdown -p zeron-client -p zeron-mobile
cargo test --release -p zeron-mobile --lib bench_layout -- --ignored --nocapture

# Line-break accuracy vs CoreText (same font bytes), flows, hitch benchmarks
xcodebuild … -only-testing:ZeronTests/LineBreakAccuracyTests test
xcodebuild … -only-testing:ZeronUITests/SessionFlowTests test
xcodebuild … -only-testing:ZeronUITests/ScrollPerformanceTests test
```

Live stack (real edge + headless engine; see `ZeronUITests/LiveStackTests.swift`):

```sh
(cd edge && npx wrangler dev --port 27650 --var AUTH_MODE:dev) &
ZERON_DATA_DIR=/tmp/e2e ZERON_IPC_PORT=27811 ZERON_EDGE_URL=http://localhost:27650 \
  ZERON_EDGE_TOKEN=alice@org1 ZERON_ORG_ID=org1 ZERON_HARNESS=mock target/debug/zeron headless &
TEST_RUNNER_ZERON_LIVE_EDGE=http://localhost:27650 xcodebuild … -only-testing:ZeronUITests/LiveStackTests test
```

The test launches the app with `-harness mock`: a chat's configured harness
wins over the engine's `ZERON_HARNESS` default, so without it a real agent
would run.

## Session notifications

The edge sends them (`edge/src/push-notify.ts`, `registry-room.ts`): when a
session's registry row changes it applies the desktop's rule (run finished,
waiting on your input, run failed) and pushes to every phone that registered
with `POST /registry/:org/push-target`. The app asks for permission the first
time a session is started from the phone (or from Settings → Notifications),
registers its APNs token with the choices from Settings, stays quiet while
open, and opens the session a notification is tapped for.

Setup once per deployment: an APNs auth key (Apple Developer → Keys →
Apple Push Notifications service), then `wrangler secret put APNS_KEY_P8` (the
`.p8` contents) and `wrangler secret put APNS_KEY_ID` in `edge/`. The
TestFlight workflow enables the Push capability on the App ID itself.
`/registry/:org/stats` shows registered phones and recent deliveries.

## Widgets

| Widget | Families | Shows |
| --- | --- | --- |
| Sessions | small, medium, large, Lock Screen | waiting on you, failed, running, finished-unseen |
| Usage Limits | small, medium, large, Lock Screen | agent plan windows (used %, resets in) per host login |
| Devices | small, medium | execution hosts, online state, session counts |
| Quick Actions | small, Lock Screen circular | new session; Talk to Codex when a host offers voice |
| Controls | Control Center, Lock Screen, Action button | New Session, Talk to Codex (`LaunchIntents.swift`) |

The extension never links the Rust core. On every workspace change the app
writes a small `WidgetSnapshot` to `widget-sessions.json` in the
`group.sh.zeron.ios` app group, and reloads the timelines only when it
changed (or every 10 minutes while the app is open, to refresh its
`publishedAt`). Running sessions tick their elapsed time on their own. Taps
open `zeron://chat/<id>`, `zeron://new`, `zeron://call` or
`zeron://sessions` (`SceneDelegate`); the Quick Actions tiles are intent
buttons (`NewSessionIntent`, `TalkToCodexIntent`), since a small widget has
one tap URL. Controls and intents route through `LinkRouter`, which holds a
route until the scene connects; the scene holds it again while signed out
or launching, until the signed-in shell is up, and `call` waits up to 5s for
the registry so the host picker doesn't report no online device. Signing out
deletes the file; the demo never writes it.

Usage comes from each online host's `ListAgentAccounts` (relayed;
`forceUsage`), polled by `UsagePoller` when the app opens and every 5
minutes while it runs. Offline hosts keep their last windows, marked
"Updated … ago" after an hour. The usage widget schedules entries every 15
minutes so "resets in" counts down and a passed window reads "Reset" without
the app.

Widget freshness is renewed only while the registry is connected and has
synced. Disconnected snapshots keep their last confirmed timestamp and
become stale normally. Removed hosts are pruned from usage as soon as a
connected registry confirms their removal, even if no hosts remain online
or the next usage poll is not due yet.

Each Usage Limits widget is configurable (Edit Widget → Agents,
`UsageConfiguration.swift`): pick agents and it shows only those, in the
order picked; leave it empty for the automatic pick (signed-in logins
first, then the most used). Agents that keep a login per model provider are
listed per provider ("Pi · Claude", "Pi · OpenAI").
The automatic pick skips such a provider login while it has no meters,
only an error (an unused login whose token expired), as long as anything
else has limits to show. Cursor and OpenCode are temporarily excluded from
the usage widget and its picker, including cached snapshots and previous
selections. Widgets that selected only those agents return to automatic
selection. This version uses the existing desktop usage endpoint and
requires only an iOS app update; no desktop rebuild is needed.
Selected agents keep their chosen order. Every agent keeps its own card,
even when two of them are signed in to the same ChatGPT account.

The widgets only update while the app runs (or was just backgrounded). 30
minutes after the last publish their timeline turns stale: running sessions
stop ticking and show their start time, the headline and device list read
"as of <time>", and online dots go neutral. `UsagePoller` keeps its
per-host replies in `UserDefaults` (tagged with the user and org, cleared on
sign-out) so a relaunch while a host is offline doesn't drop its usage.
Session titles, projects and device names are `privacySensitive`, redacted
on a locked device.

Setup once per team: create the `group.sh.zeron.ios` App Group in Apple
Developer → Identifiers, and enable it on both `sh.zeron.ios` and
`sh.zeron.ios.LiveActivity`. Xcode's automatic signing does this for local
device builds; the TestFlight export needs it on the App IDs.

## TestFlight release

Run the **TestFlight** workflow from GitHub Actions on `main`. It installs the
Rust iOS targets, selects the next build number from App Store Connect,
archives (device, Release — the Rust core builds in the archive) with
automatic signing, and uploads a TestFlight build (internal, or external with
beta review). Secrets: `AC_API_KEY_P8`, `AC_API_KEY_ID`,
`AC_API_ISSUER_ID`.
