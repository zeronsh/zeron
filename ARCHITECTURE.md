# zeron — Architecture

A ground-up native rewrite of [zeron](../zeron) — a multi-device controller for coding agents
(Claude Code / Codex) — in Rust, with a gpui UI. Fresh app; no backwards compatibility required.

**Pillars (from the goal):**
- Optional sync uses Loro CRDT docs (loro-mirror model) through Cloudflare Durable Objects; the same docs persist locally when sync is disabled.
- Durable Objects stay **TypeScript** (decision + evidence: `docs/research/durable-objects-language.md`).
  Everything device-side is Rust.
- Feature parity with zeron **except token-usage display** (poor fit for CRDTs; excluded).
- Frontend is **gpui** (pinned Zed rev). Virtualization + markdown techniques ported from
  **mugen + pretext** (`docs/research/mugen-pretext.md`).
- One binary, **headed or headless**. Smooth transitions/animations matching the original
  (catalog in `docs/research/feature-inventory.md` §1.12).

## 1. Topology (unchanged shape, new materials)

```
gpui UI ─ in-proc/localhost RPC ─ engine A ══ DeviceRoom DO relay ══ engine B ─ RPC ─ gpui UI
                    │       optional edge Worker: auth, rooms, R2        │
                    └── optional chat2 sync ──  ChatRoom DO (per chat) ──┘
                                          └─ Workspace registry room ────┘
```

- **Engine = backend** (was `@zeron/backend`): runs agents, owns auth, terminals, repos/worktrees,
  diff sync, doc hosting. Pure Rust daemon, fully functional headless.
- **UI = viewport** (was Electron): gpui app rendering engine state. Talks the same typed RPC whether the engine is in-process or a separate daemon. Organized around **spaces** — (device, folder) pairs, local or synced according to the active profile. The sidebar is the data: an attention-sorted Sessions list, filtered by a searchable spaces dropdown ("All spaces" included) that also hosts space management. The horizontal tabs are a **device-local viewport** onto that list (`ui-settings.json` `openTabs`, cross-space): closing a tab is local-only — archiving is an explicit sidebar action — and a sidebar click (re)opens a session as a tab. The new-session canvas carries a space picker (defaulting to the sidebar filter, else the last selected space); new sessions are minted onto the picked space's device via relay-forwardable RPCs.
- **Edge (TypeScript, ported from zeron `apps/edge`)**: Worker + ChatRoom DO (per chat, the
  chat2 row protocol; the legacy SessionRoom DO remains deployed only for pre-cutover clients —
  no current client dials it) + DeviceRoom DO (per device) + R2 attachments + WorkOS JWKS auth.
  Absorbs the old `apps/server` responsibilities (WorkOS code exchange/refresh, orgs) so
  **Postgres, the Hono server, and the WebRTC/signaling stack are all gone**.
- **Cloud** (`docs/design/cloud-device.md`): a checkout option — a session in any project
  whose GitHub repository the Zeron GitHub App reaches can run on its own hosted sandbox,
  with Cloud's own providers — an ordinary headless engine
  provisioned per session by the edge's CloudAccount DO + Workflows behind a provider-neutral
  sandbox interface, sleeping when idle, authenticated with its own device key (runner tokens
  scoped to that one session's device), and fed provider credentials by the separate
  service-binding-only `zeron-vault` Worker (one Durable Object per account serializes
  refreshes, so parallel sessions never race a refresh token).

### Headed / headless
Binaries `zeron` (app + CLI) and `zeron-engine` (the app's background engine, no UI linked):
- `zeron` — headed. If an engine is already listening on the IPC port, connect to it. Otherwise
  start the **background engine** (`crates/engine/src/managed.rs`) — `zeron-engine` beside the app
  ("Zeron Engine" inside the macOS bundle), or `zeron engine` where no separate executable ships
  (Windows, which updates as a single `zeron.exe`) — and attach to it. It is spawned detached from
  the app (own session/process group, no inherited pipes), not through a service manager, so on
  macOS privacy permissions stay attributed to the app and no background-item approval is needed.
  Closing or quitting the app leaves it running with every session; it shows up as its own process.
  The app reconnects when the connection drops (starting it again if it is gone) without resetting
  what's on screen, and stops it itself only to replace the runtime (sign-out, the local→synced
  switch). After an app update it restarts into the new version once no run or terminal is live.
  If it can't start — or the port is 0, or something else holds the port — the app embeds the
  engine **in-process** as before (RPC over an in-memory duplex — same protocol — and serving that
  engine on the IPC port); `ZERON_EMBED_ENGINE=1` forces it. An engine that went away unasked is
  started again after 1, 2, 5 and 15 s; a fifth loss within five minutes stops and shows the failure
  (with Retry) rather than embedding — an engine bug would crash an in-process engine too.
  The engine stops by itself once idle if its app is moved to the Trash or uninstalled, and its log
  (like every long-running mode's) moves to `.old` past 64 MB rather than growing for weeks.
- Desktop banners are the engine's (`desktop_notify.rs`, advertised as `desktop-notifications-v1`):
  finished runs, questions, failures, connection loss and agent updates keep arriving with the app
  closed. Preferences come from the app's `ui-settings.json`; "only in the background" uses window
  focus the app reports (`SetViewportFocus`). The app keeps the chimes, decided by the same baselines.
- `zeron headless` — engine only. A clean installation immediately serves its local profile over localhost IPC; when a saved account selects the synced profile at startup and a bearer is available, it also hosts its DeviceRoom for remote control. A VPS can run this while a laptop's UI drives it.

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

`zeron login` and `zeron logout` operate on `session.json` while the engine is stopped. Login selects `Synced` for the next start; logout selects `Local` for the next start. The UI may update live authentication status, but the active `WorkspaceScope` still changes only after restart.

Managed Scaffold profiles are isolated per authority under `{data_dir}/profiles/scaffold/{key}/`, so moving to another deployment never reopens the old store. `comet import-native-profile --source-copy DIR --writers-stopped` (RPC `ImportNativeProfile`, owner controller only) carries a stopped-writer copy of another authority's native profile into the running one: documents keep their bytes plus unacknowledged outbox rows with cursor 0 / epoch 2, are seeded into this Edge as seq-0 checkpoints before their rebuilt registry rows land, take this identity's `crew` metadata, and move to this device when the source device hosted them (other hosts stay inert until they import). Pending source commands get outcome-less ledger claims so hosts reject rather than replay them; the source uploads root is granted read-only via `native-profile-imports.json`. `scripts/migrate-canary-to-staging.sh` is the operator wrapper.

The resolved profile selects the session snapshots, registry snapshot, run journals, and attachment cache that may contain workspace data:

| Scope | Store and journals | Uploads |
| --- | --- | --- |
| `Local` | `{data_dir}/profiles/local/` | `{data_dir}/profiles/local/uploads/` |
| `Synced` | `{data_dir}/orgs/{org_id}/{user_id}/` | `{data_dir}/orgs/{org_id}/{user_id}/uploads/` |
| `Development` | `{data_dir}/orgs/{org_id}/{user_id}/` | `{data_dir}/orgs/{org_id}/{user_id}/uploads/` |

The synced and development store roots preserve the historical cloud layout while their attachment caches are account-scoped. Local identity lives in `{data_dir}/local-profile.json`; its UUID is stable across restarts and is not an account or development identity.

Older releases wrote every synced and development attachment to `{data_dir}/uploads/`, and persisted those absolute paths in transcripts. On upgrade, the first synced or development account that opens this legacy cache claims it in `{data_dir}/legacy-uploads-owner.json`. That account may read the cache as a compatibility fallback, but all new staging and commits use its account-scoped uploads root; other accounts cannot read or write the legacy cache.

Device identity and machine resources remain device-scoped under the common data directory: `device-id`, repository registration, managed worktrees, agent credentials/accounts, and UI settings. They are available across profiles, but they do not contain or expose another profile's transcripts or attachments.

Remote host identity uses the same optional `devboxProvider` (`namespace` or `boat`) and immutable `devboxId` in device rows and `EngineInfo`. Ordinary and older devices have neither field; names never supply a provider identity. Linux hosts validate `COMET_DEVBOX_PROVIDER`/`COMET_DEVBOX_ID` against their native environment (Namespace task service or matching `BOAT_ID`) before profile registration. Restart rejects changes to an already registered binding. Workspace snapshots, registry migration, sync, and daemon installation preserve the binding; heartbeats update only liveness.
Session heartbeats also feed the in-memory device freshness overlay when the session's device matches its chat owner. Their original timestamps advance monotonically; replaying a stale row cannot renew presence, and no extra durable device writes are emitted.
The desktop wakes a registered offline host only on Send or an explicit Wake action in Devices/Add Project; browsing and catalog discovery never wake a provider. Wake replies must match the selected device and immutable provider binding before fresh capabilities and catalogs are used. Cancellation, failure, or target changes leave the draft and routing untouched. Existing threads route to their host even while browse metadata is missing. Background activity continues through newcore's workspace-wide session rows (no legacy session-reference pin layer); elapsed working time uses the current session turn's `started_at`, and idle Devbox cards retain an Idle label alongside recency.

`WakeDevice` requires an authenticated local synced controller and remains pinned to project/deployment `ashler-staging`. It never provisions, replaces, or recovers a machine. Namespace uses `devbox exec`; Boat inspects the exact immutable ID and resumes only stopped/archived machines. Both invoke the existing `~/.local/bin/crew-devbox-autostart staging`, then verify the exact synced peer's `EngineInfo` binding and current capabilities. One 180-second deadline covers mutex contention, provider commands and readiness. Scoped cancellation drops owned provider/readiness work; an already accepted resume can still finish. Each provider output stream retains at most 64 KiB and errors are redacted. Peer capability metadata is bounded to 128 strings of 128 bytes. Failure leaves the original route/draft intact.

The devbox workflow and its host launchers live in `Ashler-AI/ashler-platform` `skills/devbox/`. Devboxes build Crew on the host from one pinned commit of this repository's source tarball. `configure-host.sh` takes `CREW_OWNER_NAME`, `CREW_DEVBOX_NAME`, `COMET_DEVBOX_PROVIDER`, immutable `COMET_DEVBOX_ID`, the absolute physical `CREW_BINARY` and its extracted-executable `CREW_BINARY_SHA256`. `CREW_CHANNEL` retains the production default; this rollout explicitly selects `staging`. Linux release binaries keep their production build identity; channel-specific wrappers supply fixed native runtime coordinates (`~/.crew-staging`, IPC `27957` for staging; `~/.crew`, IPC `27958` for production). Owner identity and interactive sign-in are never copied from another profile. Adopting an existing native profile requires its explicit `CREW_EXISTING_DEVICE_ID` and matching runtime coordinates.

Namespace retains its native task directory, private access, `tmux` autostart and verified-release workflow. Boat uses matching live `BOAT_ID`, the authenticated official CLI and channel-qualified guarded `crew-devbox-{staging,production}.service` units. Configuration neither signs in nor starts an engine. On wake, the guard verifies the pinned executable and existing native profile; running-service ownership proves the kernel image's SHA-256, size, owner and executable mode plus a PID-owned LISTEN socket inode. It hashes with at most a 1 MiB buffer and never trusts a pathname prefix: cold restore can relocate the executable's backing image. Rollback preserves profiles and binaries. A previously enabled `ashler-localdev.service` may be started after Crew readiness; absent or disabled units remain untouched.

Boat may reclaim the retained machine during its warm-resume window; only a cold resume restores saved files and starts new processes. Follow the cold-proof procedure in ashler-platform's `skills/devbox/references/boat.md`, and recheck app/Tilt readiness separately from Crew. Crew never resets, reseeds, replaces or creates a machine to repair it. Ordinary stopped snapshots expire after 14 days; saved snapshots create a new sandbox when started and require a fresh immutable binding and sign-in, not cloned Crew identity. Keep authoritative work backed up elsewhere.

Namespace task markers and finite Boat TTL renewal belong to executing turns and in-flight inference, not parked harnesses. Admission fails before work if protection cannot be established; Boat renewal failure interrupts the exact active run recoverably. Namespace cleanup removes only owned markers and sweeps only provably abandoned process identities. Independent foreground jobs use `crew-devbox-keepalive`; focused regressions live alongside the scripts in ashler-platform and run there with `python3 -m unittest discover -s skills/devbox/scripts -p 'test_*.py'`.

#### Privacy boundary and follow-ups

This first local-first change does not upload, import, link, or delete local sessions when a user signs in. Local attachments remain jailed under the local upload root and are not readable through the synced attachment cache. Returning to local-only mode reopens the same local identity and data.

##### Remote workspace file trust boundary

Devices authenticated to the same synced account are trusted peers for remote workspace control. A peer may send relay-forwarded workspace file requests to the device that owns a checkout; the owning engine resolves the target and enforces workspace-relative path, containment, symlink, and write-conflict checks before touching its filesystem.

Ignored-file visibility is not an authorization boundary. A remote peer may request ignored entries and then read or write them, including potentially sensitive files such as `.env`, when `includeIgnored` is enabled. `.git` remains unavailable regardless of that option. Zeron intentionally does not maintain a filename denylist because it would be incomplete and could imply a security guarantee it cannot provide.

If authenticated devices must no longer trust one another with the full workspace, that policy must be enforced by the owning engine for remote requests. Hiding entries only in the UI is not a security control.

The following product work is intentionally deferred:

1. Explicit session selection and copy between local and synced profiles, including attachment copying, provenance, and conflict behavior.
2. Browsing both scopes simultaneously or switching the visible scope without restarting the engine.
3. A supported self-hosted backend contract covering authentication modes, room APIs, authorization, persistence, and blob storage. Current endpoint and bearer overrides remain development/deployment seams, not a promised compatibility surface.

## 2. Data model — all Loro

Two persistent doc kinds. When sync is enabled, session docs ride the chat2 row protocol (loro updates as append-only rows + Range-resumable checkpoints, ChatRoom DO) and the registry rides its own row-frame protocol; local-only profiles persist the same docs without joining rooms:

1. **Session doc** (per chat) — the transcript + durable command queue. Schema is a Rust port of
   `packages/session-doc` (same container names/shapes so the edge's tail materializer keeps
   working): `meta` map, `messages` list (parts as list-of-maps with **LoroText bodies** — the
   measured 1.03× oplog shape; never LWW value rewrites), `commands` list with ledger rules 1–3
   (append-only per-device entries; host-only outcomes; dedupe/TTL/supersede evaluation).
   Continuation splitting at 256KB, render-only tool parts (full inputs stay in the host's local
   run journal), tail/diff sidecars. Constants carried over (`STREAM_COMMIT_MS=120`,
   `DO_FLUSH_MS=5s`, compaction at 8MB, retain 30d, tail 64).

2. **Workspace registry doc** (per profile) — the `registry1` snapshot stores spaces (id, deviceId, path, name?, gitDetected, checkoutId, repositoryId), the chats index (id, deviceId, title, archived, cwd, branch, checkoutId, spaceId, lastSeenAt, lastMessagePreview/At, config), devices, session-status rows, and checkout-diff summary pointers. A space is a device+folder pair in the active profile; the owning device's `SpacesSync` stamps git presence so branch pickers and the diff sidebar can gate without another RPC, plus a repository identity (the trunk's root commit, so renames, transfers and remote spellings don't split a repository; a shallow or empty history falls back to the normalized origin remote, then a device-scoped common-git-dir hash) that groups clones and worktrees of one repository in the sidebar's project mode. Local scope keeps the registry entirely in its profile store. Synced and development scopes join `/registry/{orgId}/ws`, backed by the private per-user room `reg1/{orgId}/{userId}`; rows are never visible to every member of an organization.

   Writer discipline: each device writes its own device and session-status rows, rows for chats it hosts, and git stamps for spaces it owns. Creates, renames, archives, and seen marks are LWW sets accepted from any device. `deleteSpace` tombstones the space and every chat/session row in it in one commit. Presence uses ephemeral room frames rather than durable heartbeat writes.

   *Why one registry and not N tiny docs:* the sidebar needs one subscription for the whole list (grouping, resort animations, unseen markers). Its rows contain indexes rather than transcripts, so one local snapshot and, when enabled, one room connection remain bounded and cheap.

3. **Mirror layer** (`zeron-doc` crate) — Rust equivalent of loro-mirror: typed structs for the
   schema, **incremental** application of `doc.subscribe` diffs into cached state (no full
   re-hydration per change — this is also what fixes zeron's known O(transcript) re-projection
   inefficiency, remaining-work item 1a), and a diff-reconcile write path (evaluate `lorosurgeon`
   0.2.x as a dep; our schema is small enough to hand-roll if it doesn't fit). The UI renders
   mirror state directly with per-entry change notifications — the "endgame" the TS
   implementation documented but never reached.

### Command plane
Send/steer/interrupt/respondInput = durable command entries in the session doc (`QueueCommand`),
executed by the chat's **host** device (executor gated on chat ownership; mark-processed BEFORE
execute; steer with no live run dispatches as the next turn). Offline sends queue in the doc.
This is zeron's proven design, kept verbatim.

## 3. Cargo workspace

```
zeron/
  Cargo.toml                 # workspace
  crates/
    proto/        zeron-proto    # wire types: AgentEvent, ToolCall, RunRequest, Model,
                                 # entities, RPC envelopes (serde; ndjson framing);
                                 # `view` = the pure derivations both frontends share
                                 # (sort orders, staleness gating, grouping, boot gate)
    doc/          zeron-doc      # session-doc + workspace-registry schemas, mirror layer,
                                 # parts fold, continuations, command ledger, sidecars
    sync/         zeron-sync     # loro room client (join/VV backfill/fragments/backoff),
                                 # ephemeral presence, DocsStore (SQLite snapshots +
                                 # processed-command ledger)
    harness/      zeron-harness  # Harness trait + claude-code (stream-json subprocess),
                                 # codex (app-server JSON-RPC), mock; steering mailbox,
                                 # requestInput, models/reasoning/options catalogs
    engine/       zeron-engine   # sessions engine (pub/sub, run journal, recovery, stall
                                 # watchdog), doc host + command executor, repos/worktrees,
                                 # checkout-diff sync, terminals (portable-pty), uploads,
                                 # agent accounts (cred swap), auth (WorkOS via edge),
                                 # device-room host/peers, identity
    rpc/          zeron-rpc      # UiRpc/ControlRpc: typed req/resp/stream over WS (tokio-
                                 # tungstenite) + in-memory transport; device-room virtual
                                 # sockets ({s,k,to,from} frames)
    theme/        zeron-theme    # source-neutral theme schema + built-in/custom registry,
                                 # validation, provenance, and local VS Code compiler
    voice/        zeron-voice    # desktop-local optional Parakeet model, capture and inference; no RPC/sync
    ui/           zeron-ui       # gpui app: shell, sidebar, conversation, composer,
                                 # terminal view, diff pane, settings, animation kit
  apps/
    zeron/                       # the binary (headed default, `headless` subcommand)
  edge/                          # TypeScript Worker + DOs (ported from zeron/apps/edge,
                                 # + auth-exchange routes absorbed from apps/server)
  docs/                          # this file + research reports
```

Engine async runtime: **tokio** throughout; the UI bridges via `gpui_tokio` (`Tokio::spawn`
futures surfaced as gpui `Task`s). In-process mode runs the engine on its own tokio runtime
thread; the UI never blocks on it.

## 4. UI plan (gpui) — parity + smoothness

Reference: `docs/research/gpui.md`, `docs/research/mugen-pretext.md`,
feature spec `docs/research/feature-inventory.md` §1.

- **Deps**: `gpui` + `gpui_platform` pinned to one Zed rev (Apache-2.0). **We do not use Zed's
  GPL crates** (`markdown`, `ui`, `theme`, `editor`) — markdown, components, and theme are ours.
- **Transcript**: gpui `list()` + `ListState::new(n, ListAlignment::Bottom, overdraw)` (sum-tree
  offsets, follow-tail). On top of it, port the mugen behaviors that gpui doesn't give us:
  - stick-to-bottom **spring** with feed-forward tracking of streaming growth; interrupt from
    *user input* (wheel-up / drag), re-engage within a 70px band; own-send re-engages + smooth
    scrolls;
  - **block-granularity rows** (one row = one markdown block / tool group, not one message) with
    stable ids `msgId#blockId`; live turn stays unsplit, re-splits on persist; optimistic echo
    rows share the client-minted id so persistence never flickers;
  - row height memoization keyed by (row id, content length, width) so a streamed token
    re-measures one row;
  - scroll-anchor absorption for above-viewport height changes.
- **Markdown** (`zeron-ui::markdown`): `pulldown-cmark` parsing on `background_spawn` with
  coalescing (Zed's proven pattern), block-level incremental re-parse of the streaming tail
  (incremark's O(delta) idea: only re-parse from the last stable block boundary), monochrome
  theme where **numbers drive layout, colors are paint**. Code blocks: monospace, no wrap ⇒
  height = lines × line-height (layout independent of highlight); syntax highlighting via
  `synoptic`/`syntect`-class tokenizer run time-sliced in the background, colors applied as text
  runs (paint-only). Streaming **fade-in veil** on newly appended text via `with_animation`
  opacity (paint-layer, never affects layout). `prefers-reduced-motion` honored.
- **Composer**: hand-rolled gpui text input (start from Zed's `examples/input.rs`: IME, selection,
  clipboard, key actions), compact↔expanded auto-flip by measured text width, auto-grow 76–260px,
  Enter/Shift+Enter, Send→Steer→Stop morph, drafts + attachments per chat, drag-drop/paste
  images, QuestionPanel (paged, 1-9 keys, 220ms auto-advance, a corner close / Escape that
  answers nothing) replacing the composer while input is requested. Pickers (harness/model, traits, repo w/ folder browser, branch w/ worktree
  toggle) as gpui popovers with `menu-in` scale/fade.
- **Terminal**: `alacritty_terminal` (vte state machine, MIT/Apache) + `portable-pty` on the
  engine side; custom gpui grid element; tabs w/ drag-reorder (150ms sliding transforms), height
  drag 160px–55vh, 80ms resize debounce, 1MB replay, detach ≠ close. No artificial latency: keys
  are written the moment they land (one write in flight per tab keeps them ordered) and output
  after a quiet spell is sent the moment it is read (only floods are paced, 4ms). Typed text goes
  through the platform text input (IME, dead keys, layout symbols); mouse/focus reporting,
  DECSCUSR cursors with a typing-aware blink, synchronized output, a contrast floor for program
  colors. A terminal on another device streams over a direct WebRTC link
  (`engine/src/terminals/link.rs`, `remote_terminals.rs`), falling back to the device-room relay
  only when the devices can't reach each other and moving back mid-session without a gap (input
  is numbered and acknowledged, so a path change neither drops nor repeats a keystroke); on
  slow links the viewer predicts the echo of typed text (`ui/src/terminal/predict.rs`).
- **Diff pane**: unified-patch parser → virtualized file/hunk/line rows, per-file collapse
  (180ms height tween), time-sliced highlight, 200ms width transition on the pane itself.
- **Animation kit** (`zeron-ui::motion`): small helpers over gpui `Animation` reproducing the
  zeron catalog — `fade-in` (0.5s, cubic-bezier(0.16,1,0.3,1), translateY 4→0), `splash-out`,
  `zeron-pulse` staggered cell wave (boot splash + loaders), `gradient-spin-pulse` matrix
  spinner (WorkingIndicator + rotating flavour word), `menu-in`/`dialog-in` scale-fades, 200ms
  ease-out width/height transitions for sidebar/panes, sidebar-resort **slide animation**
  (we own the list, so animate row positions directly — the View Transitions equivalent, 260ms
  cubic-bezier(0.22,1,0.36,1)), reduced-motion switch.
- **Theme**: independent light/dark resolved variants, theme-owned semantic/syntax/terminal
  palettes, optional interaction-accent overlays, and a device-local surface preference that
  resolves each variant's recommended frost/opaque treatment without changing theme selection.
  Forced frost derives contrast-checked tints from mapped theme surfaces. Local VS Code
  file/package compilation and imported/linked custom families retain last-known-good
  persistence. Colors remain paint-only; hairline borders and bundled Geist/Geist Mono remain
  shared presentation foundations.

## 5. Engine plan

Direct ports of zeron behaviors (spec: feature-inventory §3):
- **Sessions engine**: per-session broadcast hub; on-disk run journal (resumable `seq` replay,
  crash auto-resume); persistent steerable sessions (steering mailbox at step/turn boundary; idle
  reaper; 10min stall watchdog); recovery stamps `aborted`.
- **Doc host**: per-chat handle (join room, VV backfill, write user entries + stream assistant
  segments at 120ms commits, drain commands host-only with processed-ledger idempotence, publish
  diff sidecar, presence); warm-open recent chats (14d/cap 30); nudge-driven cold open; SQLite
  snapshot store.
- **Harness** (research pending — `docs/research/harness.md`): trait mirroring zeron's
  `HarnessShape`; Claude Code via `claude` CLI stream-json in/out (control protocol for
  permissions/AskUserQuestion→requestInput, resume, steering); Codex via app-server JSON-RPC or
  `codex exec --json`; model/reasoning/option catalogs ported from `packages/harness`.
  OMP's persistent RPC loop tracks todo/goal `get_state` refreshes by request ID
  without blocking frame consumption. Refresh deadlines leave steering, stop,
  and connection teardown responsive even when the native peer never replies.
  On Linux, exact-journal ownership uses a short-lived native helper and a kernel
  read lease: existing writers block resume, while readers and unreadable unrelated
  processes do not. The helper closes inherited descriptors, confines lease-break
  signals to itself, and releases the lease before returning. After two seconds
  the parent kills and reaps a timed-out helper. Unsupported or incomplete probes
  remain unknown and fail closed;
  no ptrace, mount, or filesystem permissions are relaxed. macOS keeps its native
  `lsof` probe. Linux needs read-lease support and `close_range` (kernel 5.9+).
- **Repos/diffs**: git2 or `git` subprocess (subprocess — matches zeron, avoids libgit2 edge
  cases); worktrees under `~/.zeron/worktrees`; fs watchers (`notify`) + 2min repair; diff
  capture (patch + numstat + untracked, 3MiB cap, sha256) → workspace registry summary + DO diff
  sidecar.
- **Agent accounts**: credential-slot swap (macOS Keychain via `security-framework`, files
  elsewhere), plan labels, usage probes, paste-code/browser-poll OAuth flows.
- **Auth**: WorkOS through edge routes (`/auth/exchange`, `/auth/refresh`, orgs); loopback
  callback server headed, paste-code headless; dev mode (no key ⇒ bearer = configured user id).

## 6. Edge plan (TypeScript, `edge/`)

Port `zeron/apps/edge` nearly verbatim (it is already Loro-native and smoke-tested: session room
w/ hibernation + two-level compaction + daily alarm backups, device room byte relay + nudges +
sidecar slots, R2 attachments, JWKS auth). Additions:
1. Private per-user registry rooms (`/registry/{orgId}/ws` → `reg1/{orgId}/{userId}`) with authenticated row sync and ephemeral device presence.
2. `/auth/*` routes absorbed from `apps/server` (WorkOS API key in Worker secret).
3. Drop `/seed` migration path and legacy sync anything (fresh app).
Hibernation hygiene: no idle timers (flush timer only while dirty), auto-response ping/pong —
per `docs/research/durable-objects-language.md`.

## 7. Parity exclusions & deliberate changes

- **Excluded**: token-usage display (profile heatmap, lifetime stats, per-message token columns,
  `WatchUsage`). Rate-limit meters on agent accounts are *kept* (separate concern; probed from
  CLIs, not CRDT-synced).
- **Changed**: Postgres entity sync/server → workspace registry + edge; Electron/React/mugen → gpui with
  ported techniques; Node harness SDKs → subprocess protocols; WebRTC → device-room relay (zeron
  had already made this move); mobile app → out of scope for this repo.
- **Kept verbatim**: session-doc schema shape + constants, command ledger rules, edge DO design,
  render-parts privacy policy, UX behaviors and animation timings.

## 8. Milestones

Status legend: ✅ shipped · 🟡 shipped with named gaps (see `docs/PARITY.md`).

- ✅ **M0 Scaffold** — workspace builds; `proto`/`doc` crates with ledger + parts + continuation
  unit tests; gpui hello-window runs.
- ✅ **M1 Doc + sync core** — `zeron-doc` mirror over loro 1.13; room client syncs with the edge
  running under `wrangler dev`; Rust⇄edge⇄Rust convergence test (M1 exit: two Rust peers converge
  through a real SessionRoom DO, tail endpoint serves).
- ✅ **M2 Engine core** — Claude harness end-to-end headless: `zeron headless` + dev auth runs a
  turn, journal + doc writes, recovery test.
- ✅ **M3 UI core** — shell (sidebar/panes/header), transcript (virtualized, markdown, streaming,
  stick-to-bottom), composer (send/steer/stop, question panel); local chat fully usable headed.
- ✅ **M4 Multi-device** — device-room host/client virtual sockets, remote device control, workspace
  registry sync, WorkOS auth + org gate, presence. Proven live by `scripts/e2e-smoke.sh`:
  two headless engines against a real edge — B queues a run into the chat doc, the durable
  nudge wakes host A, A executes (mock harness), transcript + session status sync back to B.
- 🟡 **M5 Full surface** — terminals, diff pane, repo/branch/folder pickers + worktrees,
  agent accounts UI, settings (devices/shortcuts/archived), Codex harness. Gaps: composer
  attachment UI (engine upload RPCs exist), Cursor harness.
- 🟡 **M6 Polish** — wire reconciliation (proto AuthState on the wire, `LocalDevice`),
  two-device e2e smoke, keyboard map, clippy/fmt sweep, Linux packaging
  (`scripts/package-linux.sh` + release profile), macOS bundling config (`dist/macos/`,
  not executed — needs a Mac). Gaps: prefers-reduced-motion, engine hardening
  (instance lock, watchdogs), edge production deploy.

## 9. Open questions (tracked, non-blocking)

1. loro-protocol Rust client ⇄ TS edge interop — verify at M1; fallback is a ~300-line hand-rolled
   client (the frame protocol is small and we control both ends).
2. `lorosurgeon` fit for the mirror write path vs hand-rolled reconcile.
3. Cursor harness (zeron has it; CLI surface for Rust TBD) — parity item, scheduled after Codex.
4. Text shaping performance for analytic row heights: gpui measures shaped text natively (Rust ⇒
   cheap), so we start with gpui `list()` measurement + memoization rather than porting pretext's
   full analytic kernel; revisit only if cold-open of huge transcripts measures slow.
