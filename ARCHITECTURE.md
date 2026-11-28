# Zeron — Architecture

Zeron is a local-first, multi-device controller for coding agents (Claude Code, Codex, Cursor, OpenCode, Devin, Grok, Hermes, fx, Pi, Antigravity, Muse Code). Everything device-side is written in **Rust**, with a **[zgx](https://zgx.zeron.sh)** desktop UI (native Rust host + hot-reloading Luau interface), native **iOS** and **Android** mobile clients sharing a Rust core, and a **TypeScript** Cloudflare Workers + Durable Objects edge for optional multi-device sync and Cloud sandboxes.

**Core pillars:**
- **Local-first with optional CRDT sync:** sessions and workspace metadata are Loro CRDT documents persisted locally in SQLite. When signed in, the same documents sync through Cloudflare Durable Objects; offline local profiles never open online transports.
- **Rust on devices, TypeScript at the edge:** Durable Objects stay TypeScript (decision + evidence: `docs/research/durable-objects-language.md`). Engines, harnesses, sync, layout/math/visual runtimes, and desktop/mobile cores are Rust.
- **ZGX desktop app (`apps/zgx`):** the desktop app for macOS (Apple silicon, macOS 13+), Linux, and Windows is built on `zgx` — a GPU-rendered Rust host running a hot-reloading Luau UI (`apps/zgx/ui/`) with first-class templates, signed plugins, and separated themes. (The earlier GPUI frontend in `crates/ui` has been retired.)
- **Headed or headless:** the desktop app talks to a detached background `zeron-engine` process over localhost IPC; Linux servers and Cloud sandboxes run the same engine headless (`zeron headless`).

## 1. Topology

```
zgx UI (Rust + Luau) ─ localhost IPC ─ engine A ══ DeviceRoom DO relay ══ engine B ─ IPC ─ zgx UI
                                           │       optional edge Worker: auth, rooms, R2        │
                                           ├── optional chat2 sync ── ChatRoom DO (per chat) ───┤
                                           └── workspace registry ─── RegistryRoom DO ──────────┤
                                                                                                │
                                           iOS (UIKit) / Android (Compose) ── crates/mobile ────┘
```

- **Engine (`crates/engine`, binary `zeron-engine`):** pure Rust daemon that runs coding agents, owns auth, terminals, repos/worktrees, checkout-diff sync, local previews, plugin verification/services, MCP tools, and Loro doc hosting. Fully functional headless.
- **Desktop UI (`apps/zgx`, packaged as `zeron`):** zgx viewport rendering engine state over local WebSocket IPC (`127.0.0.1`). Does not link the engine. Organized around **spaces** (device + folder pairs, grouped by git repository identity in the sidebar) with multi-pane split chats (up to 6 side-by-side in the default **Zeron v3** template, or single-page in **Zeron v2**), a session companion panel (files, git changes, terminals, browser, previews), customizable templates, signed plugins, and data-only themes.
- **Mobile clients (`apps/ios`, `apps/android`):** peer devices over the edge DOs built on one shared Rust mobile core (`crates/mobile`, UniFFI). No agent runs on the phone; Rust handles sync, markdown/text layout, virtualization, and terminal emulation while Swift (UIKit) and Kotlin (Jetpack Compose) paint and handle gestures.
- **Edge (`edge/`):** TypeScript Cloudflare Worker + `ChatRoom` DO (per chat, `chat2` row protocol; legacy `SessionRoom` DO remains deployed only for pre-cutover clients) + `DeviceRoom` DO (per device) + per-user `RegistryRoom` DO + `PreviewRoom` DO + R2 attachments + WorkOS JWKS/OAuth auth (`/auth/*`). No Postgres, Hono server, or WebRTC signaling stack.
- **Cloud (`docs/design/cloud-device.md`):** a checkout option — a session in any project whose GitHub repository the Zeron GitHub App reaches can run on its own hosted sandbox, with Cloud's own providers — an ordinary headless engine provisioned per session by the edge's `CloudAccount` DO + Workflows behind a provider-neutral sandbox interface, sleeping when idle, authenticated with its own device key (runner tokens scoped to that one session's device), and fed provider credentials by the separate service-binding-only `zeron-vault` Worker (`edge/vault/`, one Durable Object per account serializes refreshes, so parallel sessions never race a refresh token).

### Headed / headless
Packaged releases ship the ZGX app binary (`zeron`) alongside the engine helper (`zeron-engine`, built from `apps/zeron`):

- **CLI dispatch (`crates/cli`):** `zeron` handles `--version`, `update`, `daemon`, and help directly, and delegates engine subcommands (`headless`, `login`, `logout`, `status`, `plugin`, `template`, etc.) to the bundled engine helper beside it (`zeron_cli::Engines::Helper`): `Zeron Engine` inside `Zeron.app/Contents/MacOS/` on macOS, `zeron-engine` beside `zeron` on Linux, and `beta/<version>/engine/zeron.exe` on Windows. The standalone `apps/zeron` `zeron` binary links the engine in-process (`Engines::InProcess`) for headless CLI usage and opens no window.
- **Windowed launch (`apps/zgx/src/engine/manager.rs`, `crates/launch`):** if an engine is already listening on the IPC port (`27654` / `~/.zeron` in release builds; `27871` / `~/.zeron-staging` in dev builds by default), `zeron` connects to it. Otherwise it spawns the background engine detached (own session/process group, no inherited pipes) so on macOS privacy permissions stay attributed to the app and no background-item approval is needed. Closing or quitting the app leaves the engine and all running sessions alive. The app reconnects if the connection drops, stops the engine only to switch runtime mode (sign-out, local↔synced switch), and after an app update the engine restarts into the new bundle once no run or terminal is live (`SupersededRestart`). The engine exits once idle if its app is moved to the Trash or uninstalled, and rotates its log to `.old` past 64 MB.
- **Updates from another device are transactions** (`crates/update/src/transaction.rs`, `crates/engine/src/update_trial.rs`). The installed version changes in one step, only once a new version has proved it's ready: `app/current` on Linux, an atomic bundle swap on macOS, a rename on Windows. Until then every start runs the old version, which is how a new binary that exits, hangs, or loses power gets undone. The old version copies the SQLite stores, leaves a watchdog, and hands over; the new one holds back what it can't take back (resuming chats, queued messages) until it has committed. A failed trial restores the stores, and `UpdateStatus.update` says which happened under the id `ApplyUpdate` answered with. Only the SQLite stores roll back. Provider CLIs update under running chats where their installer keeps old files (`UpdatePlan::while_chats_run`), and wait otherwise.
- **Desktop notifications** are the engine's (`desktop_notify.rs`, advertised as `desktop-notifications-v1`): finished runs, questions, failures, connection loss, and agent updates keep arriving with the app closed. Preferences come from the app's settings; "only in the background" uses window focus the app reports (`SetViewportFocus`), and clicking a banner opens the chat via `zeron://notification/…` (`apps/zgx/src/links.rs`). The app plays chimes using the same baselines.
- **`zeron headless`** — engine only. A clean installation immediately serves its local profile over localhost IPC; when a saved account selects the synced profile at startup and a bearer is available, it also hosts its `DeviceRoom` for remote control. A VPS can run this while a laptop's UI or phone drives it.

### Local-first workspace profiles

Authentication and workspace selection are deliberately separate state machines:

- `AuthState` is live credential state: `SignedOut`, `NeedsOrganization`, or `SignedIn`. It may change after login, refresh, revocation, or logout.
- `WorkspaceScope` is the immutable storage and transport boundary captured once at engine startup: `Local`, `Synced`, or explicit `Development`.

The engine never re-resolves an open store because `AuthState` changed. This prevents a sign-in, token refresh, or revocation from silently swapping databases or attaching online transports to a runtime that started local-only.

| Startup condition | `WorkspaceScope` | Online transports |
| --- | --- | --- |
| WorkOS enabled, no parseable saved `session.json` | `Local` | Disabled |
| Parseable saved WorkOS session | `Synced` | Enabled when a bearer is available; organization onboarding completes before opening the store when needed |
| WorkOS disabled without a dev bearer | `Development` | Disabled |
| Explicit non-empty dev bearer | `Development` | Enabled |

`zeron login` and `zeron logout` operate on `session.json` while the engine is stopped. Login selects `Synced` for the next start; logout selects `Local` for the next start. The UI may update live authentication status, and restarts the managed engine once to transition between local and synced scopes.

The resolved profile selects the session snapshots, registry snapshot, run journals, and attachment cache that may contain workspace data:

| Scope | Store and journals | Uploads |
| --- | --- | --- |
| `Local` | `{data_dir}/profiles/local/` | `{data_dir}/profiles/local/uploads/` |
| `Synced` | `{data_dir}/orgs/{org_id}/{user_id}/` | `{data_dir}/orgs/{org_id}/{user_id}/uploads/` |
| `Development` | `{data_dir}/orgs/{org_id}/{user_id}/` | `{data_dir}/orgs/{org_id}/{user_id}/uploads/` |

The synced and development store roots preserve the historical cloud layout while their attachment caches are account-scoped. Local identity lives in `{data_dir}/local-profile.json`; its UUID is stable across restarts and is not an account or development identity.

Older releases wrote every synced and development attachment to `{data_dir}/uploads/`, and persisted those absolute paths in transcripts. On upgrade, the first synced or development account that opens this legacy cache claims it in `{data_dir}/legacy-uploads-owner.json`. That account may read the cache as a compatibility fallback, but all new staging and commits use its account-scoped uploads root; other accounts cannot read or write the legacy cache.

Device identity and machine resources remain device-scoped under the common data directory: `device-id`, repository registration, managed worktrees, agent credentials/accounts, plugins/templates/themes, and UI settings. They are available across profiles, but they do not contain or expose another profile's transcripts or attachments.

#### Privacy boundary and follow-ups

Local sessions are never silently uploaded, imported, linked, or deleted when a user signs in. Local attachments remain jailed under the local upload root and are not readable through the synced attachment cache. Returning to local-only mode reopens the same local identity and data.

##### Remote workspace file trust boundary

Devices authenticated to the same synced account are trusted peers for remote workspace control. A peer may send relay-forwarded workspace file requests to the device that owns a checkout; the owning engine resolves the target and enforces workspace-relative path, containment, symlink, and write-conflict checks before touching its filesystem.

Ignored-file visibility is not an authorization boundary. A remote peer may request ignored entries and then read or write them, including potentially sensitive files such as `.env`, when `includeIgnored` is enabled. `.git` remains unavailable regardless of that option. Zeron intentionally does not maintain a filename denylist because it would be incomplete and could imply a security guarantee it cannot provide.

If authenticated devices must no longer trust one another with the full workspace, that policy must be enforced by the owning engine for remote requests. Hiding entries only in the UI is not a security control.

Deferred product work in this area:

1. Explicit session selection and copy between local and synced profiles, including attachment copying, provenance, and conflict behavior.
2. Browsing both scopes simultaneously or switching the visible scope without restarting the engine.
3. A supported self-hosted backend contract covering authentication modes, room APIs, authorization, persistence, and blob storage. Current endpoint and bearer overrides remain development/deployment seams, not a promised compatibility surface.

## 2. Data model — all Loro

Two persistent doc kinds. When sync is enabled, session docs ride the `chat2` row protocol (Loro updates as append-only rows + Range-resumable checkpoints, `ChatRoom` DO) and the registry rides its own row-frame protocol; local-only profiles persist the same docs without joining rooms:

1. **Session doc** (per chat) — the transcript + durable command queue (`crates/doc`): `meta` map, `messages` list (parts as list-of-maps with **LoroText bodies** — the measured 1.03× oplog shape; never LWW value rewrites), `commands` list with ledger rules 1–3 (append-only per-device entries; host-only outcomes; dedupe/TTL/supersede evaluation). Continuation splitting at 256 KB, render-only tool parts (full inputs stay in the host's local run journal), tail/diff sidecars. Constants: `STREAM_COMMIT_MS=120`, `DO_FLUSH_MS=5s`, compaction at 8 MB, retain 30 d, tail 64.

2. **Workspace registry doc** (per profile) — the `registry1` snapshot stores spaces (`id`, `deviceId`, `path`, `name?`, `gitDetected`, `checkoutId`, `repositoryId`), the chats index (`id`, `deviceId`, `title`, `archived`, `cwd`, `branch`, `checkoutId`, `spaceId`, `lastSeenAt`, `lastMessagePreview/At`, `config`), devices, session-status rows, and checkout-diff summary pointers. A space is a device+folder pair in the active profile; the owning device's `SpacesSync` stamps git presence so branch pickers and the diff sidebar can gate without another RPC, plus a repository identity (the trunk's root commit, so renames, transfers, and remote spellings don't split a repository; a shallow or empty history falls back to the normalized origin remote, then a device-scoped common-git-dir hash) that groups clones and worktrees of one repository in the sidebar's project mode. Local scope keeps the registry entirely in its profile store. Synced and development scopes join `/registry/{orgId}/ws`, backed by the private per-user room `reg1/{orgId}/{userId}`; rows are never visible to every member of an organization.

   Writer discipline: each device writes its own device and session-status rows, rows for chats it hosts, and git stamps for spaces it owns. Creates, renames, archives, and seen marks are LWW sets accepted from any device. `deleteSpace` tombstones the space and every chat/session row in it in one commit. Presence uses ephemeral room frames rather than durable heartbeat writes.

   *Why one registry and not N tiny docs:* the sidebar needs one subscription for the whole list (grouping, resort animations, unseen markers). Its rows contain indexes rather than transcripts, so one local snapshot and, when enabled, one room connection remain bounded and cheap.

3. **Mirror layer (`crates/doc`)** — typed Rust structs over Loro with incremental application of `doc.subscribe` diffs into cached state (avoiding O(transcript) re-hydration per change) and per-entry change deltas streamed over `WatchDocMessages`.

### Command plane
Send/steer/interrupt/respondInput are durable command entries in the session doc (`QueueCommand`), executed by the chat's **host** device (executor gated on chat ownership; mark-processed BEFORE execute; steer with no live run dispatches as the next turn). Offline sends queue in the doc.

## 3. Repository & Cargo workspaces

Zeron uses **two Cargo workspaces** so `apps/zgx` can carry zgx's `[patch.crates-io]` (`winit`, `wgpu-hal`, `accesskit_*`) without affecting the root workspace (the engine, shared crates, and mobile core):

```
zeron/
  Cargo.toml                       # root workspace (engine, CLI, shared crates, mobile core)
  assets/                          # shared fonts, icons, file icons, sounds
  apps/
    zgx/                           # DESKTOP APP (standalone Cargo workspace)
      Cargo.toml                   # zeron-zgx binary; links UI-free shared crates, no engine
      src/                         # Rust host: windows, IPC, engine manager, bridges, plugins/templates
      ui/                          # hot-reloading Luau UI: app.luau, gallery.luau, ds/, screens/, locales/
      plugins-builtin/             # built-in plugins: git, github, parallax-scenes
      plugins-dev/                 # example plugins and staging rig
      docs/                        # composer, plugins, templates, themes, i18n specs
    zeron/                         # `zeron-engine` binary + headless CLI `zeron` binary (no window)
    ios/                           # iOS app (Swift / UIKit, links crates/mobile via UniFFI)
    android/                       # Android app (Kotlin / Jetpack Compose, links crates/mobile via UniFFI)
    landing/                       # zeron.sh Astro marketing site
    card-web/                      # member card web viewer
  crates/
    proto/          zeron-proto    # wire types, RPC envelopes, capabilities, pure shared view derivations
    doc/            zeron-doc      # session-doc + workspace-registry schemas, mirror layer, continuations, ledger
    sync/           zeron-sync     # Loro room client, ephemeral presence, SQLite DocsStore
    harness/        zeron-harness  # Harness trait + native/ACP drivers for 11 agents + mock
    engine/         zeron-engine   # sessions engine, doc host, repos/diffs, terminals, plugin host, auth, relay
    rpc/            zeron-rpc      # typed req/resp/stream over WS + in-memory + device-room virtual sockets
    cli/            zeron-cli      # shared CLI parser & dispatch (`Engines::Helper` in zgx, `Engines::InProcess` in apps/zeron)
    launch/         zeron-launch   # detached engine spawning and bundle/helper resolution
    update/         zeron-update   # self-updater, release channels, transactional updates & rollback
    notify/         zeron-notify   # desktop notification formatting & `zeron://` link payloads
    mcp/            zeron-mcp      # Zeron's MCP server (chats, devices, plugins, templates, themes, visuals, commands)
    plugin-guard/   zeron-plugin-guard  # shared plugin path, program, and network guardrails
    theme/          zeron-theme    # source-neutral theme catalog, VS Code compiler, contrast hardening
    preview/        zeron-preview  # local dev-server discovery and preview proxying
    preview-label/  zeron-preview-label # preview process labeling & command redaction
    term/           zeron-term     # renderer-free alacritty_terminal state machine + echo predictor
    term-link/      zeron-term-link # direct P2P WebRTC terminal transport + relay fallback
    syntax/         zeron-syntax   # tree-sitter syntax highlighting
    markdown/       zeron-markdown # markdown parsing/layout shared with mobile
    text/           zeron-text     # analytic text measurement and line layout (rustybuzz)
    zatex/          zatex          # OpenType MATH-table LaTeX layout engine
    visual/         zeron-visual   # sandboxed Luau runtime for agent `show_visual` scripts
    visual-gpu/     zeron-visual-gpu # offscreen wgpu renderer for agent visual shaders & 3D scenes
    voice/          zeron-voice    # desktop-local optional Parakeet v3 speech-to-text dictation
    voice-session/  zeron-voice-session # voice call session state & protocol
    voice-media/    zeron-voice-media   # voice media transport
    audio/          zeron-audio    # WebRTC AEC3 audio stack
    orb/            zeron-orb      # renderer-free thinking-orb animation geometry
    veil/           zeron-veil     # renderer-free streaming fade-in veil
    client/         zeron-client   # high-level sync client shared by mobile
    mobile/         zeron-mobile   # UniFFI library for iOS and Android
    mobile-bindgen/ zeron-mobile-bindgen # UniFFI Swift/Kotlin binding generator
  edge/                            # Cloudflare Worker + Durable Objects + `edge/vault/` (`zeron-vault`)
  dist/                            # platform icons, Info.plist, entitlements, Windows installer (.iss/.rc), .desktop
  scripts/                         # packaging, CI planner/admission/reuse, smoke rigs, profiling
  docs/                            # design docs, references, performance notes, research records
```

## 4. Desktop UI (`apps/zgx`)

Detailed developer guide: [`apps/zgx/README.md`](apps/zgx/README.md).

- **Host + Luau split:** the Rust host (`apps/zgx/src/`) creates windows, registers fonts, manages the engine (`src/engine/manager.rs`), speaks local WebSocket IPC (`src/engine/rpc.rs`), handles auth/files/git/terminals/previews/visuals/dictation/appshots/updates, and runs the plugin/template runtime. The UI (`apps/zgx/ui/`) is written in strict-mode Luau and hot-reloads in place on save while preserving state. Packaged builds ship `ui/` and `plugins-builtin/` alongside the binary (`Contents/Resources/` on macOS, beside `zeron` on Linux, `beta/<version>/` on Windows).
- **Design system & reactivity (`apps/zgx/ui/ds/`):** token-driven components built with `ui.variants` (`themes/zeron-dark.json`, `themes/zeron-light.json`, `tokens.luau`) and enforced by `ui.theme.strict("warn")` (`ZGX_DESIGN_LINT=error`). Data updates in place through keyed `ui.each` and live functions/states rather than rebuilding subtrees, and the host publishes per-entry list updates so streaming touches only the active part. `cargo run --release -- gallery` runs the interactive component workbench in light and dark mode side by side.
- **Templates (`apps/zgx/docs/templates.md`):** whole-interface overrides stored in `<data>/templates/<id>/`. The main window runs from `<data>/templates/.live/` — Zeron's own `ui/` with the active template overlaid — reloading changed files in place. Ships with **Zeron v3** (`zeron`, default: rounded inset cards and split chats up to 6 side by side) and **Zeron v2** (`zeron-v2`, built-in classic single-page layout). Per-file errors fall back to Zeron's copy; templates created or modified via agents run as restricted **previews** until explicitly kept in Zeron's own consent window; safe mode (`--zeron-template`, holding Option at launch, or View → Use Zeron v3) always restores Zeron's own UI.
- **Plugins (`apps/zgx/docs/plugins.md`, `apps/zgx/plugins-dev/README.md`):** folders in `<data>/plugins/<id>/` with `zeron-plugin.json` and `init.luau` (plus optional headless `service.luau` run by the engine). The engine verifies Ed25519 signatures against account keys registered on the edge, tracks user consent, and publishes immutable snapshots (`WatchPlugins`). The app loads only those snapshots with explicit zgx capability grants (`src/plugins/grants.rs`), scoped read records (`src/plugins/records.rs`), an isolated consent window (`ui/consent.luau`), per-plugin SQLite (`zeron.db`), cross-device plugin messaging, and agent MCP tools. Ships with built-in plugins in `apps/zgx/plugins-builtin/` (`git`, `github`, `parallax-scenes`).
- **Themes (`apps/zgx/docs/themes.md`):** color palettes separated from templates, backed by `zeron-theme::catalog`. Supports built-in families, template-provided `theme.luau`, imported VS Code color themes, independent light/dark choices (`theme-choice.json`), automatic contrast legibility checks, frosted/opaque surface preferences, and live restyling via agent MCP tools (`WatchThemes`).
- **Transcript & rich content (`apps/zgx/src/chat.rs`, `ui/ds/transcript.luau`):** virtualized transcript with block-level incremental markdown streaming (`append` frames write only to `chat.draft`, while completed/structural changes reconcile `chat.messages` in place), syntax highlighting, LaTeX math (`zatex` with bundled OpenType math fonts), Mermaid diagrams (`merman`, a headless port of Mermaid's own parser and layouts with ELK for flowcharts, labels measured with rustybuzz over Geist, drawn by `usvg`/`resvg`), sandboxed Luau agent visuals and 3D scenes (`zeron-visual`, `zeron-visual-gpu`, `zgx-script`'s `scene3d`), and owner-targeted image attachments with lightbox.
- **Composer (`apps/zgx/docs/composer.md`):** send/queue/steer/stop flow, per-chat drafts and image attachments, `@` file/workspace mentions, interactive question/approval panel, harness/model/effort/access pickers, optional on-device dictation (`zeron-voice` Parakeet v3), and macOS Appshots capture (`src/appshots/`).
- **Terminals & embedded browser:**
  - **Terminals (`apps/zgx/src/terminal.rs`):** `ui.terminal` backed by `zeron-zgx terminal-bridge`, which bridges zgx's local PTY to the engine's session-scoped PTY (local or remote over direct P2P `zeron-term-link` / device-room relay). On macOS, `zgx-desktop`'s `ghostty` feature renders terminals with Ghostty (built via Zig 0.16); Linux and Windows use zgx's built-in terminal renderer.
  - **Browser (`ui.webview`):** native webviews in the companion panel — WKWebView on macOS, WebView2 on Windows (`zgx-desktop` `webview2` feature), and WebKitGTK 4.1 on Linux (`zgx-desktop` `webkitgtk` feature, loaded at runtime with `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`).
- **Localization (`apps/zgx/docs/i18n.md`):** ICU MessageFormat via `zgx-script`'s `i18n` feature (`ui/locales/en.json`, `zh-Hans.json`).

## 5. Mobile clients (`apps/ios`, `apps/android`, `crates/mobile`)

See [`apps/ios/README.md`](apps/ios/README.md), [`apps/android/README.md`](apps/android/README.md), and [`docs/mobile-rewrite.md`](docs/mobile-rewrite.md).

- Both iOS (Swift / UIKit) and Android (Kotlin / Jetpack Compose) link the same UniFFI library (`zeron-mobile` over `zeron-client`).
- Rust owns sync, incremental session-doc projection, markdown parsing (`zeron-markdown`), analytic text measurement (`zeron-text`), prefix-sum virtualization, display lists, and terminal emulation (`zeron-term` + `zeron-term-link`); native platform code paints, scrolls, and handles gestures.
- Phones act as peer devices on the mesh: they browse spaces/sessions, stream transcripts, queue runs/steers/answers into the session doc, inspect subagents and side chats, and attach to host terminals.

## 6. Engine (`crates/engine`, `crates/harness`, `crates/mcp`)

Engine async runtime is **tokio** throughout:

- **Sessions engine:** per-session broadcast hub, on-disk run journal (resumable `seq` replay, crash recovery with `aborted` stamps), persistent steerable sessions (steering mailbox at step/turn boundaries, background subagent/process tracking, stall/idle handling).
- **Doc host:** per-chat handle joining rooms when synced, `chat2` row sync + VV backfill, streamed assistant writes at 120 ms commits, host-only command draining with mark-before-execute idempotence in SQLite (`docs.sqlite`), diff sidecar publishing, nudge-driven cold opens.
- **Harnesses (`crates/harness`):** one `Harness` trait over native and ACP drivers plus `MockHarness`:
  - **Native drivers:** Claude Code (`claude` stream-json), Codex (`codex app-server` JSON-RPC), Cursor (pinned `@cursor/sdk` shim), OpenCode (`opencode serve` HTTP/SSE), Pi (JSONL RPC), Muse Code (`muse serve` MSP JSON-RPC).
  - **Shared ACP driver (`AcpHarness`):** Devin (`devin acp`), Grok (`grok agent stdio`), Hermes (`hermes acp`), fx (`fx acp`), Antigravity (`agy_acp_server`).
  - Explicit user-triggered CLI installation (`crates/harness/README.md`).
- **Repos, worktrees & diffs:** `git` subprocess operations, managed worktrees under `~/.zeron/worktrees`, filesystem watchers (`notify`) + periodic repair, bounded checkout diffs (3 MiB cap, sha256) published to workspace registry summaries and diff sidecars, workspace search via `fff-search`.
- **Terminals (`crates/engine/src/terminals/`):** `portable-pty` sessions (1 MB replay buffer, detach ≠ close, Job Objects on Windows), plus direct WebRTC peer-to-peer links (`zeron-term-link`) for remote terminals with seamless relay fallback and numbered/acknowledged input.
- **Plugin host & MCP server:** Ed25519 signature verification, snapshot staging, headless plugin services (`service.luau`), guardrails (`zeron-plugin-guard`), and Zeron's MCP server (`crates/mcp`) exposing chat, device, plugin, template, theme, visual, and command tools to agents.
- **Agent accounts & auth:** multi-account credential-slot management, usage/rate-limit probes, OAuth flows, WorkOS auth through edge `/auth/*` routes (loopback callback headed, paste-code headless).
- **Local previews (`crates/preview`):** automatic discovery of local dev servers and tunnelled preview sharing via `PreviewRoom` DO.

## 7. Edge (`edge/`)

TypeScript Cloudflare Worker + Durable Objects + R2:

- **`ChatRoom` DO** (per chat): `chat2` append-only Loro update log + Range-resumable checkpoints, hibernatable WebSockets, two-level compaction, daily alarm backups to R2, tail/diff sidecar endpoints.
- **`DeviceRoom` DO** (per device): byte-pipe virtual sockets (`{s,k,to,from}` frames) for relay-forwarded RPCs and terminal fallback, single host socket with supersede, durable wake nudges, sidecar slots.
- **`RegistryRoom` DO** (per user): private `/registry/{orgId}/ws` → `reg1/{orgId}/{userId}` row sync and ephemeral device presence.
- **`PreviewRoom` DO** and **Cloud (`CloudAccount` DO + Workflows + `edge/vault/` `zeron-vault`):** preview tunnelling and hosted per-session Cloud sandboxes with serialized credential refresh.
- **Auth & profile (`/auth/*`, R2):** WorkOS JWKS verification, OAuth code exchange/refresh, organizations, member cards, and avatars.
- **Hibernation hygiene:** no idle timers (flush timer only while dirty) and WebSocket auto-response ping/pong.

## 8. Deliberate design decisions

- **No CRDT-synced token-usage display:** profile heatmaps, lifetime stats, and per-message token columns are excluded (poor fit for CRDTs). Rate-limit meters on agent accounts and live context-window usage are kept separately.
- **Direct P2P only where latency demands it:** general multi-device RPC and sync ride the edge Durable Objects (no Postgres, Hono server, or general WebRTC mesh), while remote interactive terminals opportunistically establish direct P2P links (`zeron-term-link`) with transparent fallback to `DeviceRoom`.
- **Two Cargo workspaces:** `apps/zgx` stays outside the root Cargo workspace so zgx's upstream crate patches (`winit`, `wgpu-hal`, `accesskit_*`) stay isolated from the engine and mobile core, while still depending on shared UI-free root crates by path.
- **Compatibility across versions:** devices update independently, so wire/document fields (`zeron-proto`, `zeron-doc`) stay additive (`#[serde(default)]`) and cross-device features gate on `zeron_proto::capabilities`.

