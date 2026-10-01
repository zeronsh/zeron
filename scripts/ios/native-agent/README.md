# Embedded Codex for iOS

Zeron embeds the real Rust Codex app-server in its process. Swift talks to a
small C ABI; Codex calls three dynamic tools backed by just-bash in a WebKit
worker. Model inference still uses OpenAI over the internet.

Without a Zeron account, **Use Native Codex** enters the normal app shell in
local mode. Choose **Native Codex** in the normal new-session provider/model menu. Selection
stays in the new-chat composer. Sending creates a normal Sessions-list entry and
uses the standard draft-to-chat transition. Native chats participate in search,
pinning, sections, and archiving. Remote projects are not used as local workspaces.

A progress bar shows actual startup stages: engine initialization, account lookup,
model catalog, and conversation restoration. An initial prompt sent during loading
is persisted and delivered once the engine and account are ready. Sign in with
ChatGPT's device-code flow or an OpenAI API key from the session's account action.

Model, reasoning effort, and service tier are available in the normal composer.
Options and descriptions come from the embedded Codex model catalog; choices are
saved per conversation and become defaults for new chats. Model default reasoning
and Automatic tier explicitly reset a previous override.

Each conversation owns a local virtual workspace. The composer attachment menu
imports project folders or individual files and exports the workspace to Files.
Folder import copies the folder contents, preserving nested folders and binary
files; it skips `.git`, `node_modules`, `.build`, and `DerivedData`. Imports reject
symlinks, conflicting filenames, and projects exceeding 128 MB or 20,000 entries (16 MB per file).
Failed imports leave the existing workspace intact. The original folder is never
modified; export a copy to save your changes back to Files. The session's Workspace
files action opens a folder browser with text editing and new-file creation.
Workspaces currently belong to one conversation, not a shared project registry.
Account actions are also in the session menu. Native Codex adapts `SessionSource` and uses the shared
`SessionViewController`, transcript renderer, composer, draft storage, and model
chips. Structured local transcript entries render native file/shell calls through
the shared tool groups, including running/error states and expandable output.
Old native Markdown tool envelopes are projected into tool rows when reopened.
Stop interrupts the turn; entering the background also interrupts it.
Native conversations remain on this device and are not synced to other devices.

## Build

The existing Xcode Rust build phase calls `scripts/ios/build-codex.sh`, which
fetches the pinned upstream revision
`804d6306e84f570393cdd1eec94c41464f503b1a` into
`target/native-agent-spike/codex`. The standalone `crates/codex-mobile` workspace
keeps its dependencies separate from Zeron's desktop graph. Install stable Rust
and the `aarch64-apple-ios` / `aarch64-apple-ios-sim` targets. The first build is
large and requires network access. Upstream source is unmodified.

`NativeShellWorker.js` is committed so a clean Xcode build has its resource.
After modifying the runtime, regenerate it:

```sh
cd scripts/ios/native-agent
npm ci --ignore-scripts
npm test
npm run build
```

`ZERON_SKIP_CODEX=1` reuses an existing platform archive during Swift iteration.
Do not use it to validate Rust changes. The old `-native-agent-lab` Debug route
remains available for standalone shell experiments; `-native-codex` opens the
actual conversation screen directly in Debug builds.

## Deterministic integration tests

Run the local Responses fixture in one terminal:

```sh
python3 scripts/ios/native-agent/model-fixture.py
```

Then run, substituting an available simulator:

```sh
TEST_RUNNER_NATIVE_CODEX_FIXTURE_URL=http://127.0.0.1:28761/v1 \
xcodebuild -project apps/ios/Zeron.xcodeproj -scheme Zeron \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' \
  -only-testing:ZeronTests/MobileShellRuntimeTests \
  -only-testing:ZeronTests/EmbeddedCodexTests \
  -only-testing:ZeronUITests/NativeCodexFlowTests test
```

The fixture asserts desktop shell/file tools are absent, requests a mobile
shell edit, and verifies the tool's real output comes back to Codex before
sending a final message. Integration tests cover that complete loop, reopening
Codex and resuming its persisted thread, interrupting a pending tool, and a
late response after cancellation, plus reasoning and service-tier request forwarding.
WebKit tests cover native write → shell edit → native read → worker restart,
binary copying/redirection, globs, retained writes after cancellation, and atomic
project import. Native-store tests cover containment, quotas, expired callbacks, and immediate
visibility through aliased app-container paths. Directory scans canonicalize the
root before deriving relative paths, including during folder import. Workspace tests cover nested/binary export round-trips and invalid
paths. UI tests check normal new-chat loading and settings/history after relaunch. No credentials or paid model requests are used by these tests.

The build applies `patches/ios-direct-tools.patch` to the pinned checkout. On iOS,
a disabled code-mode host forces direct tools even when model metadata requests
`code_mode_only`. Desktop behavior is unchanged. The fixture catalog explicitly
requests code-mode-only routing, and request assertions require all mobile tools
to be exposed directly with no `exec`/`wait` wrapper, including after thread resume.

## Runtime boundaries

- New and restored threads explicitly use a workspace-write policy with `/workspace`
  as a writable root. This keeps model-facing permissions aligned with the native
  tools; desktop execution remains disabled. Fixture tests inspect both permissions
  instructions and tool exposure on the actual Responses requests.
- Before resuming a saved thread, the Rust host repairs a missing rollout path
  after an iOS container move. It locates the same thread’s log under the current
  private home and uses Codex’s conditional path update, preserving paginated
  history and metadata. Existing paths are left alone; missing logs are not reset.
  The integration test moves a saved home and verifies the original thread and
  tool context survive.
- Rust `InProcessAppServerClient` owns the actual agent loop, model transport,
  auth and thread persistence. This is a pinned internal API, not a stable iOS SDK.
- `EnvironmentManager::without_environments` removes desktop execution
  backends. Desktop shell, patching, plugins, hooks, host skill discovery,
  multi-agent, and web search are disabled. The mobile tool specifications and
  workspace instructions are supplied at thread creation.
- `NativeWorkspaceStore`, a Swift actor, owns real files under the app's
  Application Support directory. Native read/write, the editor, import/export,
  and just-bash all use this store. The worker's filesystem adapter retains only
  a path index for globs; file bytes are read or written on demand through a
  narrow WebKit message bridge. Codex credentials and session metadata stay
  outside the workspace. Unmounted paths, including `/tmp`, are ephemeral.
- Existing JSON checkpoints migrate once into `workspace.files` directories;
  the original checkpoint remains as a backup and cannot overwrite native edits.
  File replacements use atomic writes. Folder imports stage a complete candidate
  directory and atomically swap it into place on the same volume.
- Cancel terminates the worker and revokes its native command ID. Already saved
  changes remain, including changes made before a nonzero shell exit. Shell
  results include `changedPaths`; interruption errors name retained changes.
  Interrupted scripts are never replayed. Each invocation gets a fresh interpreter
  at `/workspace`, so `cd` and environment changes do not leak between calls.
- The workspace is capped at 128 MB and 20,000 entries, with 16 MB per file. Shell execution has
  interpreter limits plus an independent worker watchdog; output is capped at
  256 KB. Patch tools and undo remain follow-up work. Native browsing uses metadata only;
  imports/exports copy files without base64-encoding the whole project.
- No Node, Python, package managers, native executables, PTYs, or general shell network
  access. Generated JavaScript runs inside the separate document renderer or website preview,
  never in the privileged shell context. This version can inspect/edit text projects;
  it cannot build arbitrary desktop projects. Public HTTPS Git clone is supported.
- just-bash's browser import of `node:zlib` is replaced with a throwing shim;
  compression commands are excluded.
- Auth uses Codex's file credential backend in app storage protected until the
  first device unlock and excluded from backups. Keys are not in preferences or
  the shell filesystem. There is no credential access from model tools.
- The app must remain in the foreground to work. There is no background runtime,
  offline model, remote workspace sync, or Claude implementation in this change.

## Validation

- Debug app built for the arm64 simulator and physical iPhone with Xcode 27;
  installed and launched on the development iPhone.
- Controlled Responses fixture exercises the embedded Codex model/tool/model loop,
  direct mobile tools, writable policy, reasoning/service tier, interruption, and
  resuming the original thread after moving its saved home.
- Workspace/store/runtime tests cover persistence, immediate file visibility,
  container aliases, containment, cancellation, binary round-trips and selection
  exports. The store/files/runtime suite also passed on the physical iPhone.
- UI tests cover normal native chats and saved tool rows. The workspace UI test
  opens a file preview and completes Save to Files on the simulator.
- Fixture tests use no credentials or paid model requests. Live OpenAI inference,
  release distribution and background execution are not validated by them.

Workspace browsing supports folder search, file types/sizes, read-only text previews with explicit editing, and Quick Look for supported binary documents. The download button or an item’s context menu opens the system Save to Files picker for a file, folder, or the whole workspace. Exports use fresh snapshots and disposable copies; selected folders exclude their siblings. Tests cover binary selection export and the browser → preview → Files save flow.

### Generated artifacts

- `render /workspace/input.html /workspace/output.pdf 800 600` (or `.png`)
  renders a single viewport from self-contained HTML, inline canvas/SVG/JavaScript,
  and embedded data images. Scripts can set `window.zeronReady` to a Promise.
  It uses a disposable WebView without filesystem handlers or network access,
  a 4-second deadline, and a maximum of 4 million pixels (each side 1–2048).
  PNGs are rasterized from the same PDF output. Native persistence finishes before
  the shell reports success; cancellation prevents late writes.
- `import_image GENERATED_FILENAME.png /workspace/output.png` imports only the
  active conversation’s generated images. Old absolute container paths are
  rebased by filename, and symlinks/non-images are rejected. Image-result events
  also save copies under `/workspace/generated/`; reopening an older chat recovers
  images whose events were ignored by previous app versions, subject to quota.
- The pinned iOS image-tool hint no longer claims output is displayed inline.
  Copy Transcript includes tool names, errors, and artifact paths instead of blank
  assistant entries. These capabilities work in existing chats through the
  existing `mobile_shell` tool; no thread/schema reset is required.

## Native project commands

These are `mobile_shell` commands, so existing conversations retain their tool
schema and immediately gain the capabilities on resume:

```sh
git init
git config user.name "Your name"
git config user.email "you@example.com"
git add .
git commit -m "Initial project"
git status
git diff --cached
git log
git branch experiment
git clone https://github.com/octocat/Hello-World.git hello
git -C /workspace/hello status
pdf html /workspace/report.html /workspace/report.pdf a4 36
pdf images /workspace/images.pdf /workspace/plot.png /workspace/photo.jpg
serve /workspace/index.html
serve stop
```

Git uses embedded libgit2, never a subprocess, shell hook or credential helper.
Only the documented subset is available: no private authentication, fetch/push,
checkout/switch, submodules, or arbitrary config. Clone rejects credentials in
URLs, redirects, symlinks/submodules and excessive downloads/trees; failed clone
removes its partial destination. `.git` is hidden from file tools, browser,
website snapshots and exports, but preserved during native folder imports.
Cancellation reaches libgit2 transfer callbacks. Network connect/read timeouts
are 5/10 seconds, transfer callback deadline is 30 seconds, native Git's worker
watchdog is 45 seconds; ordinary shell work retains its 7-second watchdog. Already
committed changes remain after cancellation. Git output uses the normal tool row.

`pdf html` prints self-contained HTML with CSS page breaks, A4/Letter paper,
0–144-point margins (default 36), up to 100 pages. `pdf images` fits one image per
A4 page, up to 24 images/16 million pixels combined. The earlier `render` command
still produces a single viewport PDF or PNG. Outputs always land in `/workspace`.

`serve` publishes a consistent native copy to a tokenized `127.0.0.1` HTTP URL.
Open **Preview website** in the chat menu, or **Open website** on an HTML file.
Relative CSS/JavaScript/images and JSON fetch work; generated pages have no
workspace bridge, external network, or server-side runtime. Refresh republishes
edits; Stop closes the server. iOS foreground operation only. The server accepts
GET/HEAD, validates Host and paths, limits requests/connections, and excludes Git
metadata. Root-absolute asset URLs should be changed to relative URLs.
