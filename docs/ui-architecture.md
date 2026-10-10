# UI architecture

*Document status: generated from a code audit of this checkout (October 2026). Every
claim below was checked against the source; corrections welcome.*

This is a developer's guide to the desktop UI: how the gpui viewport is structured, how
state and data flow through it, and where to look when you need to change something.
It assumes you know Rust and nothing about this codebase. For the system around the UI,
read `ARCHITECTURE.md` first; for colors and surfaces specifically, `docs/theme-system.md`
is the companion document.

## 1. Big picture

The UI is the gpui *viewport*; the engine is the backend. The crate `crates/ui`
(package `zeron-ui`, roughly 177k lines across 53 top-level modules) never touches
storage or the network directly. Instead, one root entity — `AppState` in
`crates/ui/src/state.rs` — is fed by long-lived typed-RPC subscriptions, and every
view renders from it. `apps/zeron` is thin boot glue on top: CLI dispatch, daemon
management, environment configuration collected into a `UiConfig`
(`crates/ui/src/lib.rs`), and `run_app`.

**Connect-or-embed.** `EngineHandle::bootstrap` in `crates/ui/src/state.rs` probes the
localhost IPC port — `ws://127.0.0.1:{ipc_port}`, default 27654 (`ZERON_IPC_PORT`,
read in `apps/zeron/src/main.rs`). If an engine daemon answers, the app attaches to it
(`RemoteEngine`); otherwise it embeds an in-process engine (`InProcessEngine` over
`EngineCore::assemble`) and serves that same engine on the same port, so a second
viewport can attach to a headed app. Ownership of the data dir is decided by an
`InstanceLock` (`crates/engine/src/instance_lock.rs`), an exclusive OS lock on
`{data_dir}/engine.lock` taken before any store opens or the port binds. The probe
and the lock together close the classic races: two engines on one data dir, or a probe
that misses a daemon still starting.

**One protocol, two transports.** The in-process transport is
`zeron_rpc::memory_client` (`crates/rpc/src/lib.rs`): two bounded mpsc channels
carrying JSON strings through the same framing and dispatch loop as the WebSocket
path, deliberately with zero serialization shortcuts so an in-process engine cannot
mask protocol bugs. The wire format is ndjson envelopes. "Typed RPC" here means typed
in the serde sense: method name constants live in `zeron_rpc::methods` (109 of them),
and params/results are `serde_json` values deserialized into `zeron_proto` structs.

**Subscriptions.** `RpcClient` in `crates/rpc/src/client.rs` offers `subscribe`,
`subscribe_scoped`, and `subscribe_checked`. Each stream is an mpsc channel with
`STREAM_QUEUE_CAP` 256 — a slow consumer backpressures the engine task rather than
growing an unbounded queue. Dropping the receiver sends a cancel frame so the
server-side task stops; `subscribe_scoped` guarantees cancellation even for a silent
server stream. The transcript stream is delta-based (`crates/doc/src/transcript_delta.rs`):
`TranscriptFrame::Reset` carries the full list, `TranscriptFrame::Delta` carries
upserts/appends/removes with a full-reset fallback whenever a diff would approach the
size of the transcript itself.

**Mobile is a different product surface.** `crates/client` is an engine-free thin
client (the "viewer device": registry mirror, chat2 rooms, remote driving) behind the
UniFFI facade in `crates/mobile`, consumed by `apps/ios` (UIKit shell with a Rust
layout engine producing display lists — see `docs/mobile-rewrite.md`). The desktop RPC
described in this document is never on that path.

## 2. Module map of `crates/ui`

All top-level modules are declared in `crates/ui/src/lib.rs`. The giants matter more
than the count: a handful of files hold most of the behavior, and the module docs at
the top of each file are unusually good — read them before reading the code.

| Module | What lives there |
| --- | --- |
| `shell.rs` + `shell/` | The whole workspace layout and navigation (~16.7k lines plus submodules: `spaces.rs` sidebar, `tabs.rs` session navigation, `command_palette.rs`, `sidebar_sections.rs`, `navigation_focus.rs`) |
| `composer.rs` | The composer feature shell (~15.2k): input, mentions, slash commands, drafts, send |
| `transcript.rs` | Virtualized conversation view (~14.2k) |
| `pickers.rs` | Composer pickers (~8.8k) |
| `changes.rs` | Diff viewer + the shared patch parser (~6.5k) |
| `state.rs` | `AppState`, `EngineHandle`, the subscription pumps (~5.4k) |
| `history.rs` | Git history pane (commit graph, paged rows) |
| `settings.rs` + `settings/` | `UiSettings`, the settings store, and the full settings UI |
| `files/` | File explorer surface (~18k across 20 files: tree, editor, previews, search, git status) |
| `markdown/` | The UI's own markdown stack over `crates/markdown` (parse → `BlockTree` → gpui render + tree-sitter paint) |
| `terminal/` | `alacritty_terminal`-backed emulator, custom grid paint, terminal panel |
| `browser/` | Device-local browser tabs: wry/WebKit host, GPUI-owned chrome |
| `dictation/` + `crates/voice` | On-device dictation (local Parakeet v3 STT; audio never leaves the device) |
| `motion.rs`, `popover.rs`, `queue.rs`, `attachments.rs`, `theme.rs`, `typography.rs` | Cross-cutting primitives (see the sections below) |

## 3. How views are built

There is no component framework. A view is a plain struct holding `Entity<T>` fields
plus `impl Render for …`, building UI through gpui's `div()`/`Styled` builder chain.
Three idioms coexist, in rough order of frequency:

1. **Free functions returning `Div` or `AnyElement`.** The dominant style. The
   settings widget kit in `crates/ui/src/settings/widgets.rs` (`page_column`,
   `section_card`, `toggle_switch`, `select_*`) and the popover row builders in
   `crates/ui/src/popover.rs` are the reference implementations.
2. **`#[derive(IntoElement)]` + `RenderOnce`** for composed one-offs (a handful).
3. **Custom `gpui::Element` impls** where measurement or paint needs direct control:
   `ComposerTextElement` in `crates/ui/src/composer.rs` (its own wrapped-line shaping,
   caret and selection quads), `TerminalElement` in `crates/ui/src/terminal/view.rs`
   (grid paint with a per-frame font probe; ligatures are force-disabled — `liga`,
   `calt`, `dlig` off — so every cell advance is exactly one column), `Frosted` and
   `Layered` in `crates/ui/src/frost.rs` (one scene layer per floating card, backdrop
   blur painted first), and `EdgeFaded` in `crates/ui/src/edge_fade.rs`.

State lives as `Entity<T>` fields observed with `cx.observe`/`cx.subscribe` and
invalidated with `cx.notify()`. Eighteen gpui `Global`s carry app-wide singletons:
`Theme`, `SettingsStore`, `PulseClock`, `MotionState`, `AppearanceState`, `ReopenState`,
`ThemeLibraryState`, `TypographyState`, and others — grep `impl Global for` in
`crates/ui` for the full list. The house convention (stated in module docs
throughout): pure logic lives in free functions with unit tests, and rendering just
reads them.

## 4. Theme and styling

Two layers. `crates/theme` is a toolkit-agnostic model: complete `ThemeVariant`
values (semantic roles, syntax map, terminal palette) grouped into `ThemeFamily`s —
19 families, 30 built-in variants — plus a VS Code importer (`crates/theme/src/vscode.rs`)
that *hardens* imported contrast instead of rejecting the theme, and a custom-theme
library persisted as `theme-library.json` (`crates/theme/src/library.rs`).
`crates/ui/src/theme.rs` is the gpui runtime: tokens as `Hsla`, spacing and radius
constants, and the glass recipes.

The doctrine, from the header of `crates/ui/src/theme.rs`: **numbers drive layout,
colors are paint.** Layout constants are plain numbers that never depend on which
color is painted. Light mode is designed, not inverted — surface order flips meaning
(content goes white, chrome goes grey), elevation reverses (white lift + border +
shadow instead of a black wash), and accents step down to 600-level siblings to keep
contrast ratios. Read that header before touching any color.

`Theme` is a `Global`, read with `Theme::of(cx)`, and passed explicitly as `&Theme`
through roughly 300 functions rather than reached through globals mid-paint. The
context-free paint helpers — `ink`, `hairline`, `wash`, `scrim` — quote alphas in
dark-mode terms and read the current appearance from an atomic, so free-function
element builders need no `cx`. Fonts are bundled Geist / Geist Mono
(`crates/ui/src/typography.rs`); interface text is rem-based (`ui_rems`, a configurable
base size), while code and terminal text are absolute px. Render caches key off
`theme::style_generation()`, a monotonic counter bumped on theme and font-size
changes.

Platform compositing: the main window blurs the desktop behind it on macOS (vibrancy)
and Windows (Acrylic) but stays opaque on Linux, where compositor blur is not
guaranteed (`Theme::window_background_appearance`, `Theme::GLASS_ALPHA`). Floating
surfaces (popovers, palettes, the composer) frost everywhere via in-app scene blur,
which needs no compositor support (`Theme::is_frost`, `crates/ui/src/frost.rs`). The
titlebar is custom chrome on all three: macOS traffic lights at 14,14 with a hidden
inset titlebar, a Windows caption cluster, and Linux CSD caption buttons whose side
follows the GNOME `button-layout` gsetting — all set up in
`crates/ui/src/lib.rs` and drawn in `Shell::render_windows_caption_controls` /
`render_linux_caption_controls` (`crates/ui/src/shell.rs`).

## 5. State and data flow

`AppState` mirrors engine state — devices, spaces, chats, sessions, transcript,
queue, transfers, connectivity, auth — as plain fields updated by `apply_*` reducer
functions (`apply_chats`, `apply_transcript_frame`, `apply_queue`, …). The reducers
are deliberately pure-ish and unit-tested; the view layer only reads.

View state lives on `Shell` as struct fields: lazy `Option<Entity<…>>` panes, per-chat
panel flags that are deliberately *not* persisted (`SessionPanels` in
`crates/ui/src/shell.rs` — a fresh app starts with everything closed), and a
`NavHistory` for back/forward through sessions.

The subscription pumps are self-resubscribing loops: a failed subscribe or a dead
stream retries after 2 seconds (`RETRY_DELAY` in `crates/ui/src/state.rs`), with
comments in the code recording that this is incident-driven hardening. The transcript
pump decodes and prepares frames on the background executor and treats a parse failure
or delta desync as a resync trigger — it resubscribes and lets the fresh stream's reset
frame heal the copy, because that watch is the only transcript delivery path.

**Optimistic UI.** Sends echo into the transcript immediately with the client-minted
id (`AppState::echoes`), shown for up to `UNDELIVERED_GRACE_MS` (120s) before being
flagged as undelivered with an explicit retry affordance; a doc frame carrying the
same id supersedes the echo.

**Persistence.** `ui-settings.json` (`FILE_NAME` in `crates/ui/src/settings.rs`; ~79
fields on `UiSettings`, 12 of them never serialized) has a single in-process writer,
`SettingsStore`, which debounces saves by `SAVE_DEBOUNCE_MS` (400ms) and writes
atomically (temp file + rename). A three-way merge (`UiSettings::merge_changes`) exists
so a settings page holding a stale working copy can never revert a setting another
surface changed. Sticky composer picks (last harness/model/reasoning) persist in a
separate `composer-defaults.json` (`crates/ui/src/settings/composer.rs`), written
synchronously on every pick — separate
file and separate cadence precisely to avoid save races with the shell's store.

**Async.** Engine bootstrap runs on tokio through `gpui_tokio::Tokio::spawn`; once a
client exists, the RPC futures are runtime-agnostic, so the pumps run on gpui's
executor with `cx.spawn` and fold frames in via `this.update(…)`. Heavy decode work
(transcript preparation, patch parsing) uses `BackgroundExecutor` with explicit
ownership rules so a cancelled watch never frees a whale's object graph on the UI
thread (see `WatchPreparation` in `state.rs`).

## 6. Navigation model

The sidebar *is* the session list. Rows sort by pure recency — `sort_active` in
`crates/proto/src/view.rs`, with `SESSION_STALE_MS` 45s gating live status — and
attention drives only the status dots (`attention_rank` buckets), never position,
because bucketing rows made them jump under the pointer. Pins promote rows above the
recency projection; custom sections and an archived shelf organize further. The
spaces dropdown filters the list (it is not a navigation spine) and hosts space
management via row context menus; adding a space opens a palette with device tabs and
a filtered folder browser (`crates/ui/src/shell/spaces.rs`).

Horizontal session tabs are gone (removed 2026-08-10, per the module doc of
`crates/ui/src/shell/tabs.rs`): the titlebar shows a `+` new-session button, the
session title, and a `project @ device` tag. `UiSettings.open_tabs` is a legacy field
— it survives for file compatibility but nothing reads or writes it. `tabs.rs` still
holds session *navigation* logic: cycling between sessions (wrapping), session
controls widths, and the right-pane tab strip.

The new-session canvas (`mod-n`) targets the sidebar filter when one is set, else the
composer's last-picked project, else `last_space_id`, with an explicit "no project"
opt-out (`no_project`); the space picker is device-scoped (a project pick implies its
host device).

The right pane is a per-chat surface host (`RightSurface` in `crates/ui/src/shell.rs`):
`File`, `Browser`, `Diff`, `Terminal`, `Subagent`, `SideChat`, or a `Picker` when
empty. It has its own tab strip (drag-reorder, unsaved dots, a `+` menu), per-type
teardown through the normal close lifecycle (unsaved-file prompts included), and a
takeover mode that expands to the conversation floor. Column widths, from
`crates/ui/src/settings.rs` constants: sidebar 224–400px, conversation minimum 300px,
right pane minimum 360px, file explorer 220–440px, terminal drawer 160px to 55vh.
Resize seams are zero-width with 10px half-width hitboxes, and layout width changes
animate through `WidthTween` (200ms ease-out, driven manually — see §9).

## 7. Composer and transcript

`ComposerInput` (`crates/ui/src/composer.rs`) is a hand-rolled multiline gpui editor:
IME composition, undo, drag selection, and its own `ComposerTextElement` for wrapped
shaping plus caret/selection paint. `Composer` is the feature shell around it: staged
attachments and appshots (uploaded via `pending://` refs understood by newer engines),
slash commands including the builtin `WorkspaceCommand` catalog (`model`, `new`,
`resume`, `settings`, `diff`, `files`, `terminal`, `rename`, `stop`), `@` file mentions
rendered as chips, push-to-talk dictation ("hold to talk, release to transcribe",
`crates/ui/src/dictation`), a queue panel where editing leases a row from the chat
host, a compact↔expanded height flip with hysteresis, and per-chat drafts.

Pickers (`crates/ui/src/pickers.rs`) are anchored popovers (`PickerKind`: Branch,
Checkout, HarnessModel, Space, Device) sharing fuzzy ranking (`popover::match_rank`)
and `popover::Loadable` slots for async catalogs. Keyboard navigation is readline-like
(`popover::classify_key` maps ctrl-n/ctrl-p to up/down), and ⌘1–⌘9 jump-pick the Nth
visible model row. Sticky picks land in `composer-defaults.json` (§5).

Transcript (`crates/ui/src/transcript.rs`, design in `docs/research/mugen-pretext.md`)
is a gpui `list()` virtualized, bottom-aligned stream with a 320px overscan
(`OVERDRAW_PX`). Rows are block-granular — a user message is one bubble row, assistant
messages split into one row per markdown top-level block plus folded tool-group and
chip rows — with stable ids (`{msgId}#{partId}.{blockIx}` and `{msgId}#g{groupIx}`)
and per-entry content-fingerprint caching, so streaming
touches only changed rows. Markdown is parsed incrementally by `crates/markdown`
(O(tail) appends off the last stable block boundary); hanging inline markers are
"mended" (`crates/markdown/src/mend.rs`) in the display-only tree so late closers never
reflow painted text. Tree-sitter highlighting (`crates/syntax` via
`crates/ui/src/syntax_cache.rs`) is paint-only background recoloring with an
LRU document cache. Streaming gets a fade veil (`crates/ui/src/markdown/veil.rs`:
`ElemVeil`, per-chunk durations from an EMA of the stream cadence), stick-to-bottom is
`StickSpring` (velocity spring with feed-forward, released only by user input), scroll
anchoring plus a "runway" tail reservation keeps the active turn scrollable, and tool
calls fold into `ToolGroup` rows whose detail heights are analytic (capped output,
thought wrap, diff and stats lines) and whose diffs share `changes::parse_patch` with
the Changes pane.

## 8. Right-pane surfaces

**Terminal** (`crates/ui/src/terminal/`): a pure `alacritty_terminal` emulator
(`emulator.rs` — bytes in, grid out; selection lives in the `Term` so it tracks live
output for free), a custom grid-painting `TerminalElement` with a per-frame font probe
(`view.rs`, `COALESCE_MS` 12ms key coalescing, `RESIZE_DEBOUNCE_MS` 80ms resize
debounce), and a per-chat tab panel over terminal RPCs (`panel.rs`:
`OpenTerminal`/`SubscribeTerminal`/`WriteTerminal`/`ResizeTerminal`/`CloseTerminal`) —
tabs survive navigation because they are server-side PTYs.

**Changes** (`crates/ui/src/changes.rs`): a pure unified-patch parser
(`parse_patch`: `diff --git` sections → file/hunk/line rows) rendered with
line-granular `list()` virtualization, unified and split layouts over the same parse
(`DiffMode`), and scopes `DiffScope::WorkingTree | Branch | LatestTurn | History |
Commit` (History and Commit are the `crates/ui/src/history.rs` pane's entries).
Patches parse in the background; row updates splice minimally to keep scroll anchors;
highlighting runs in two phases (excerpt then full source). Review comments anchor to
diff lines and fold into the next prompt as plain text (`crates/ui/src/comments.rs`).

**Files** (`crates/ui/src/files/`): a tree model with paged directory loads
(`model.rs` `apply_page`), git color decorations with per-directory rollup computed
from a shared metadata-only status subscription (`git_status.rs`), VS Code "Seti"
identity icons (`crates/ui/src/file_icons.rs`), and a preview pane with LRU eviction
(`preview.rs`): image, markdown, and a code editor with optimistic-concurrency saves
(`document.rs` revision/hash checks). Search, drag, rename, and delete live beside
it (`search.rs`, `drag.rs`, `rename.rs`, `mutations.rs`) and stamp mutations with
revisions and interaction generations so stale loads can't clobber newer state.

## 9. Windows, overlays, motion

One window in production: 1320×880 default, 900×600 minimum, geometry restored
per-display (`restored_main_window_bounds` in `crates/ui/src/lib.rs`). No popout
windows, no tray icon.

Overlays build on `crates/ui/src/popover.rs`: `deferred(anchored(…))` for floating
cards, a `Popup` lifecycle with an exit phase (gpui unmounts elements eagerly, so
closing state is kept alive to play out the fade), and `modal` (dim scrim + centered
card). The command palette (`mod-k`, `crates/ui/src/shell/command_palette.rs`) mixes
actions with conversation search. Dialogs follow one pattern — an `Option<…Dialog>`
struct field wrapping a `ComposerInput` where text is needed, with escape escalation
(escape closes the top layer first — see `wizard_escape_goes_back` and
`dismiss_on_escape` in `crates/ui/src/composer.rs` and `crates/ui/src/settings`).

Notifications: `NSUserNotification` on macOS (deprecated but shippable), `notify-send`
on Linux, no-op on Windows where a toast needs a registered AppUserModelID
(`crates/ui/src/notify.rs`).

Motion is a catalog of `MotionSpec` constants in `crates/ui/src/motion.rs` with exact
CSS cubic-bezier evaluation (the animation inventory is `docs/research/feature-inventory.md`
§1.12). Three drivers, each with a reason: gpui `with_animation` for entrances (with
documented debt — gpui divs have no scale transform, so menu/dialog scale is
approximated as fade + translate); manual wall-clock tweens for layout widths
(`WidthTween` in `shell.rs`, hover color fades in `motion.rs` — `with_animation`'s
element-id-keyed clock replays from zero on remount, which would re-animate live
layout); and one shared 30fps `PulseClock` for repeating loaders, which fixed a real
regression where a per-element `with_animation` spinner pinned the window at 120Hz
and burned ~36% CPU. Sidebar reorders glide with a FLIP-style offset map
(`shell.rs` `resort_offsets`, `RESORT` 260ms). Reduced motion is first-class
(`ReduceMotion::System | On | Off`), and `ZERON_MOTION_SCALE` stretches every timeline
for inspection.

## 10. Settings and keymap

`Route::Settings(SettingsSection)` replaces the workspace content. Pages are lazy
`Option<Entity<…>>` fields on `Shell`, each keeping a working copy and emitting typed
events (`ShortcutsEvent`, `FilesSettingsEvent`, `AppearanceSettingsEvent`, …) that the
shell folds into `UiSettings` — one write path, one store, one save cadence. Visible
sections (from `SettingsSection::ALL`): General, Appearance, Notifications, Voice,
Shortcuts, Harnesses ("Providers" — includes the Accounts page), Devices, Files,
Appshots (macOS/Linux only), Archived. Pages share the widget kit in
`settings/widgets.rs`.

The keymap (`crates/ui/src/settings/shortcuts.rs`) binds 15 named `ShortcutId`s plus
`JumpSession(n)` slots, all rebindable by recording a combo. Conflicts are detected at
record time through `cx.intercept_keystrokes` and refused (never persisted);
`mod-enter` is reserved for composer send, and ⌘K is fixed to the palette.

## 11. Testing and conventions

`cargo nextest run -p zeron-ui --lib` is the suite (`scripts/ci/nextest.toml`): ~1,480
declared tests — 1,170 `#[test]` plus 310 `#[gpui::test]`. Interaction
coverage is strong — `VisualTestContext` keystroke simulation, a `navigation_focus`
suite (`crates/ui/src/shell/navigation_focus.rs`), composer and dialog interaction
tests. Windows CI runs the full lib suite in release mode with real DirectWrite
metrics (`native_diff_font_geometry` in `crates/ui/src/changes.rs` demands a real
proportional font); the macOS job drives real WebKit fixtures and uploads screenshots
as evidence artifacts (`scripts/run-macos-browser-fixture.sh`), not assertions. There
are no accessibility tests; a11y annotations are partial (147 `role(...)` and 116
`aria_label(...)` call sites in `crates/ui`).

Known debt, for orientation rather than alarm: the god-files (`shell.rs`, `composer.rs`,
`transcript.rs`) hold tens of thousands of lines each including inline tests; the
scrollbar rail has two implementations (`popover::rail` and
`settings::widgets::rail`, same `ScrollRailHost` shape); `unwrap()` appears in
hundreds of production call sites with hot spots in `markdown/selection.rs` (global
mutex) and `changes.rs` (cached-parse assumptions); and `AppState` carries ~115
methods across its impl blocks.

`crates/client` is **not** the desktop data path — it is the engine-free thin client
behind mobile (`crates/mobile` UniFFI). The iOS app is UIKit plus a Rust layout
engine (Rust owns row geometry and display lists, Swift paints runs); Android is
designed for but unbuilt.

## 12. Where to look next

- `docs/theme-system.md` — colors, surfaces, themes, and the importer.
- `docs/research/gpui.md` — gpui notes specific to this pinned revision.
- `docs/research/mugen-pretext.md` — the virtualization and markdown blueprint the
  transcript was ported from.
- `docs/research/feature-inventory.md` §1.12 — the animation catalog.
- `docs/mobile-rewrite.md` — the client/mobile architecture this document deliberately
  stays out of.
