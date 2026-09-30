# Android — the app's features

The Android app (`apps/android`, Material 3 Expressive on the shared
`zeron-mobile` core) is a viewer and remote control for the Zeron engines on
the account's computers, and for the offline Demo. This page covers what it
does beyond the transcript and session list; the layout of the app is in
[`apps/android/README.md`](../apps/android/README.md), the shared Rust core
in [`docs/mobile-rewrite.md`](mobile-rewrite.md).

```
Compose UI ── zeron-mobile (UniFFI) ── zeron-client ── relay / device room ── the computer's engine
```

Everything below talks to a computer's engine through the same path the
transcript uses: a **host call** (`CoreClient.host_call`, an untyped engine
RPC to a device) or a **host stream** (`CoreClient.host_watch`, acknowledged
by the host, items as JSON, cancelled by dropping it). In Demo a simulated
engine answers, so every screen below can be tried offline.

## UI

- **Sign in** (WorkOS) or **Explore the demo**. Debuggable builds also have
  *Developer sign-in*: seven taps on the mark asks for an edge URL and a
  `user@org`, and joins an `AUTH_MODE=dev` edge with no WorkOS (see
  [Development](#development)).
- **New session**: projects grouped by computer, each with "No project"; a
  model picker (providers along the bottom, models of the chosen harness,
  a star to favorite — favorites are their own tab); the draft's working
  branch and context-usage chips; **New project** clones a repository or
  creates an empty one on the chosen computer through its engine
  (`CloneRepo` / `CreateRepo`).
- **Coding agents** (Settings → Coding agents, per computer) speaks
  `host_call`: `ListHarnesses`, `InstallHarness` / `CancelInstall`,
  `CheckHarnessUpdates`, `ApplyHarnessUpdate`, the header's Update all
  (`ApplyAllHarnessUpdates`, progress by polling `ListHarnessUpdates`),
  `UninstallHarness` (the confirmation lists the engine's `dryRun`; accounts
  stay), `ListAgentAccounts`, `StartAgentLogin` → Custom Tab →
  `PollAgentLogin` (or paste-code `CompleteAgentLogin`), `ForgetAgentAccount`.
  Reply parsing is `core/Agents.kt`. The desktop's installed-agent details
  gained the matching Uninstall row.
- **Context usage**: a chip in the composer shows the share of the agent's
  context window in use; tapping it opens the desktop's context card
  (tokens used, remaining). The Demo reports it as turns run.
- The keyboard is dismissed the moment a screen is left.

## Developer tools

The desktop's files panel, editor, terminal and browser, on the phone
(`app/…/tools/`). Every tool addresses a workspace — a chat's folder
(`chatId`) or a project's (`spaceId`) on its computer, `WorkspaceRef` — and
speaks that computer's engine over host RPC. They open from the session
screen: the file-tree button in its header, and Files / Terminal / Browser &
previews in its menu. Launch routes `files:<chat>`, `terminal:<chat>`,
`file:<chat>|<path>` and `browser:<chat>|<url>` open them directly.

- **Streams** (`host_watch`): `WatchWorkspaceFiles`,
  `WatchWorkspaceGitStatus`, `WatchPreviews` and `SubscribeTerminal`.
- **Files** (`FilesScreen`): the tree from `ListWorkspaceDirectory` (folders
  load when opened), the desktop's file and folder icons, git markers from
  `WatchWorkspaceGitStatus`, live reloads, fuzzy find (`SearchWorkspaceFiles`),
  show/hide ignored files. Long-press: open, open in the browser (HTML), save
  to Downloads, copy path.
- **Viewer / editor** (`FileScreen`): `ReadWorkspaceFile`; tree-sitter
  highlighting over FFI (the desktop editor's grammars), line numbers, wrap
  or sideways scroll. Edit → Save uses `WriteWorkspaceFile`, whose
  content-hash check turns a concurrent change into a dialog (Overwrite /
  Reload / Keep editing). Markdown previews as GFM in a script-less WebView
  whose relative images load from the workspace; images decode natively with
  pinch zoom; PDFs render with `PdfRenderer`; anything else offers Save to
  Downloads and Open with… (a cached copy through the app's FileProvider).
- **Raw bytes**: `ReadWorkspaceBytes` (a new engine method, relay-forwardable)
  reads any regular workspace file in <= 512 KiB chunks by offset; a
  continuation names the revision (size, mtime, inode) of the first chunk and
  fails if the file changed. PDFs, images, Open with…, workspace pages and
  Save to Downloads all stream through it.
- **Terminal** (`TerminalScreen`, `TerminalView`, `Terminals`): engine
  terminals (`OpenTerminal` in the chat's folder) rendered through the FFI
  `TerminalScreen` — the desktop's emulator, moved out of `crates/ui` into
  the shared `zeron-vt` crate — fed by `SubscribeTerminal` (a replay of the
  engine's scrollback, then live output; resumed with `afterSeq` when the
  stream drops), input as ordered `WriteTerminal` calls, `ResizeTerminal`
  debounced with the view. Kotlin paints styled runs on a Canvas in Geist
  Mono; an extra-keys row (Esc, Tab, sticky Ctrl / Alt, arrows with repeat,
  `| ~ / -`, Home/End, PgUp/PgDn, paste); drag to scroll back, long-press to
  select. Tabs per workspace survive navigation; shells keep running on the
  engine, Close kills one. (Termux's terminal-view was not an option: its
  emulator owns a local PTY, ours are remote.)
- **Browser** (`BrowserScreen`): a WebView with an address bar (a port ->
  `localhost:<port>`, loopback over http, hosts over https, anything else a
  search), back / forward / reload-stop, open in another app, print or save
  as PDF. *Workspace pages*: `https://<token>.workspace.zeron.invalid/<path>`
  is served by `shouldInterceptRequest` from `ReadWorkspaceBytes`, so an
  agent's `index.html` loads its CSS, scripts and images from whichever
  computer owns it. *Previews*: the session's dev servers from
  `WatchPreviews`. The app has no preview proxy, so a preview opens straight
  from the computer over the network: the sheet asks once for the computer's
  address (`10.0.2.2` from the emulator) and remembers it; the server must
  listen beyond loopback. A PDF a page opens (WebView can't show PDFs)
  renders in the native viewer. Cleartext HTTP is allowed app-wide
  (`network_security_config.xml`) for dev servers on the LAN.
- **Save to Downloads** (`Downloads`): a file as itself; a folder or the whole
  project as `<name>.zip` (git-ignored files left out unless shown), written
  by `ZipOutputStream` straight into a `MediaStore.Downloads` stream in
  `Download/Zeron` as `ReadWorkspaceBytes` chunks arrive — never whole in
  memory — with progress on screen and in a notification. An engine without
  `ReadWorkspaceBytes` (an older computer) is told to update.
- **Transcript**: links and file paths open in the app — http(s) and
  localhost in the browser, workspace files in the viewer (absolute paths
  under the chat's folder, relative ones, `file://`, editor `:line`
  suffixes), inline-code paths too (underlined chips). A tool line's file
  badge (Read / Write / Edit) opens its file on tap and offers Open / Open in
  browser / Save to Downloads / Copy path on long-press.

## Subagents

The desktop's Subagents view (#638, #647), on the phone:

- **Where the data comes from.** The session row carries `running_subagents`
  (the engine publishes it as subagent sinks open and settle and clears it
  when the run ends; stale rows count zero), so the sessions list can badge a
  chat that is not open. The list itself is read from the open chat's
  transcript: every spawn chip with a stamped doc ref is one subagent
  (`zeron_client::subagents`, exposed as `CoreClient.subagents(chat)`). Order
  and grouping are the desktop's and live in Rust: running first,
  longest-running on top, then unstamped; finished ones split into Completed
  and Failed, newest first.
- **Sessions list**: a "N" pill (capped "99+") leads the row's own status; it
  never replaces it, so a finished parent reads "2 Done".
- **Session header**: a subagents button appears once the chat has any; while
  some run its face breathes and a badge shows the count. It opens the
  **Subagents** sheet: running rows on top, then **Finished (N)** — closed by
  default — holding **Completed** and **Failed**, each collapsible and
  paging at ten. State and slot plan are `ui/SubagentsModel.kt` (JVM-tested).
- **Opening one**: a row, or a spawn card in the transcript, opens
  `SubagentScreen`: the subagent's own transcript, read-only
  (`CoreClient.open_subagent` joins its doc `{chat}--sub--{id}`), above a card
  with what the spawn chip records. Subagents are steered through the parent
  chat; the screen has no composer.
- **Demo**: "Audit every sync path" (3 running, 13 completed, 2 failed) and
  "Background test sweep". Launch straight into them with
  `--es route subagents:chat-fanout` or
  `--es route 'subagent:chat-fanout|chat-fanout--sub--fo-soak'`.

## Development

Try the app against your own computers with no WorkOS:

```
cd edge && npx wrangler dev --port 27740 --var AUTH_MODE:dev
ZERON_EDGE_URL=http://127.0.0.1:27740 ZERON_EDGE_TOKEN=dev-user@dev-org \
  ZERON_ORG_ID=dev-org zeron headless
```

A debuggable build's *Developer sign-in* (seven taps on the sign-in mark; the
edge defaults to `http://10.0.2.2:27740`, the emulator's host) signs in as
`dev-user` / `dev-org` — the bearer `user@org` such an edge accepts — and
remembers the edge with the credentials. For scripted runs:
`adb shell am start -n sh.zeron.android/.MainActivity --es dev-edge
http://10.0.2.2:27740 --es dev-user dev-user --es dev-org dev-org`. Release
builds never offer it.

## Known gaps

- In full-screen terminal programs (vim, less, htop) a drag scrolls the
  client's scrollback rather than sending wheel/arrow input.
- `WriteWorkspaceFile` can't create files, so the editor edits existing ones.
- Previews need the computer's LAN address (no proxy in the app).
- The Demo has no workspace files, terminals or previews (its simulated engine
  answers the agent-management and new-project calls only); the developer
  tools open on a real computer.
- Subagent end times aren't recorded on the spawn chip, so finished ones show
  their last update instead.
- Verified on an Android 16 x86_64 emulator; not yet on arm64 hardware.

Emulator note: boot with `-feature -ReadColorBufferDma -feature -GLDMA2`
(swiftshader) or `system_server` aborts; use three-button navigation and
`adb emu screenrecord screenshot <file>` (`screencap` hits the same
assertion).

## Build and test

```
cd apps/android && ./gradlew :app:assembleDebug :app:testDebugUnitTest
```

`:app`'s `preBuild` runs `scripts/android/build-core.sh` (`crates/mobile` ->
`jniLibs` and the Kotlin bindings in `target/android-core`); `-PzeronSkipCore`
reuses the last build. Toolchain: JDK 21, Android SDK platform 37 + NDK
29.0.14206865, rustup targets `aarch64-linux-android x86_64-linux-android`,
`cargo-ndk`.
