# opencode v2 — primary-source research for the harness compatibility audit

Researched 2026-09-18 from primary sources only: opencode.ai docs, the GitHub repo (now `anomalyco/opencode`; `github.com/sst/opencode` redirects there), GitHub Releases/Tags/compare APIs, npm registry, and maintainer-authored docs in the repo (`CONTEXT.md`, `V1_API_MIGRATION.md`, `V2_HTTP_API_AUDIT.md`). Every claim carries its source URL. Raw-file URLs are pinned to a ref (`dev`, `v2.0.8`, `v2.0.3`) so claims are reproducible.

**Read this first — naming hazard:** "v2" means three different things in this ecosystem:

1. **The v1 server wire** (what our harness speaks today): the *legacy unprefixed* API (`/session/...`, `/global/health`, `/event`, `/doc`) served by the 1.18.x line. The repo's own migration doc explicitly says: "V1 refers to the legacy unprefixed server APIs used by `@opencode-ai/sdk/v2`, *despite the SDK package name*" — i.e. the npm SDK's `/v2` export speaks the **v1** wire. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/app/V1_API_MIGRATION.md
2. **The v2 HTTP API** (aka "current API"): the new `/api/*`-prefixed, Effect-TS `HttpApi` surface defined in `packages/protocol`. This is what opencode 2.0.x serves.
3. **The 2.0.x release line**: separate tags cut from a v2 lineage that no longer contains the legacy `packages/opencode` CLI.

---

## 1. What opencode v2 is; release timeline; current version lines

### Product and repo

- The project is "the open source AI coding agent", available as terminal UI, desktop app, and IDE extension; company is **Anomaly** (site footer "©2026 Anomaly", `anoma.ly`). Sources: https://opencode.ai/docs , https://opencode.ai/
- The repo **moved from `sst/opencode` to `anomalyco/opencode`** (the old URL redirects; all links, CI, and packages reference `anomalyco/opencode`). Sources: https://github.com/sst/opencode (redirect), https://github.com/anomalyco/opencode
- Stats claimed on the site: 208k GitHub stars, 950 contributors, 16M monthly devs. Source: https://opencode.ai/

### What was rewritten

- v2 is **not a language rewrite of the server**. Primary source shows the v2 core/server/protocol are **TypeScript on Effect** (`effect/unstable/httpapi`: `HttpApi`, `HttpApiGroup`, `HttpApiEndpoint`, `HttpApiBuilder`, Effect `Schema`, Drizzle over SQLite):
  - Protocol groups (Effect `HttpApiGroup`): https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/api.ts
  - Server router (`HttpApiBuilder`, `HttpRouter`): https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/server/src/routes.ts
  - SQLite/Drizzle core: `packages/core/src/database/*` in the repo tree (https://github.com/anomalyco/opencode/tree/dev/packages)
  - Note: web search surfaced third-party-style summaries claiming a "Rust rewrite"; **no primary source supports that for the server**. There *is* a Rust/Tauri build toolchain for the desktop app (`packages/containers/rust`, `packages/containers/tauri-linux` Dockerfiles: https://github.com/anomalyco/opencode/tree/dev/packages/containers), and v1 release notes mention a "native runtime" for LLM requests (`OPENCODE_EXPERIMENTAL_NATIVE_LLM`, see §6). Treat "v2 = Rust" as false for the wire-relevant components.
- Architecturally v2 is a **client/server split with a new "Session Runtime"**: the TUI/desktop/web are clients of an HTTP server exposing an OpenAPI spec from which SDKs are generated; there is also an in-process "Embedded OpenCode" that runs the same router with an in-memory `HttpClient`. Maintainer-authored architecture doc (language/relationships section, e.g. "OpenCode Client", "Embedded OpenCode", "SDK Contract IR", "Session Drain", "Context Epoch"): https://raw.githubusercontent.com/anomalyco/opencode/dev/CONTEXT.md
- The v2 API is still explicitly labeled experimental in code: protocol `api.ts` OpenAPI annotations say *"Experimental HttpApi surface for selected instance routes"*, version `0.0.1`; session group titled *"Experimental session routes"*. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/api.ts , .../groups/session.ts

### Timeline (all from GitHub Releases/Tags)

- v2 development shipped *inside* the 1.x line first:
  - v1.15.6 (2026-05-20): "The v2 HTTP API now exposes structured public error schemas", "The v2 OpenAPI spec now preserves endpoint error responses", "Anthropic API-key models now use the native runtime". Source: https://github.com/anomalyco/opencode/releases/tag/v1.15.6
  - v1.16.0 (2026-06-05): "v2 desktop UI improvements", "Added a thinking level selector for v2 prompts", SDK: "Exposed session location data in v2 responses". Source: https://github.com/anomalyco/opencode/releases/tag/v1.16.0
  - v1.18.24 (2026-08-28): "V1 now reads supported V2 config fields so newer config files keep working in more mixed setups." Source: https://github.com/anomalyco/opencode/releases/tag/v1.18.24
- **v2.0.0 was tagged 2026-09-11** by `thdxr` (commit "fix(release): use V2 Docker artifact paths (#48571)") — the v2 GA marker. Source: https://github.com/anomalyco/opencode/releases/tag/v2.0.0
- Full v2 tag stream (tags page): v2.0.0 09-11 · v2.0.1, v2.0.2, v2.0.3 09-12 · v2.0.4, v2.0.5 09-16 · v2.0.6, v2.0.7 09-17 · **v2.0.8 2026-09-18 (today, latest v2)**. Source: https://github.com/anomalyco/opencode/tags
- The **1.x line is still alive and is GitHub's "Latest" release**: v1.18.31 (2026-09-14). npm `opencode-ai@latest` = **1.18.31** (platform optional-deps binaries; published from `packages/opencode`). Sources: https://github.com/anomalyco/opencode/releases , https://registry.npmjs.org/opencode-ai/latest
- So as of 2026-09: **two parallel release lines** — 1.18.31 (legacy CLI/TUI/desktop product; GitHub "latest"; npm latest) and 2.0.8 (v2 server line; source-only GitHub release assets; Docker artifacts).
- v2.0.x releases carry only source assets and a bare "release: v2.0.x" note (e.g. v2.0.3: 2 assets). Binary distribution for v2 is Docker (v2.0.0 commit message; `ghcr.io` login + buildx in the publish workflow). Sources: https://github.com/anomalyco/opencode/releases/tag/v2.0.3 , https://github.com/anomalyco/opencode/releases/tag/v2.0.0 , https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/.github/workflows/publish.yml
- The v2 lineage lives on dedicated refs: repo has a `2.0` branch (https://api.github.com/repos/anomalyco/opencode/branches), and the v2.0.8 publish workflow gates legacy CLI artifact publication with `if: github.ref_name != 'v2' && github.ref_name != 'beta'` (a `v2` branch also exists). Compare stats: `dev` vs `v2.0.8` = diverged (dev ahead 3523 / behind 1237); `2.0` branch is strictly behind v2.0.8 (v2.0.8 ahead by 6906). Sources: https://api.github.com/repos/anomalyco/opencode/compare/dev...v2.0.8 , https://api.github.com/repos/anomalyco/opencode/compare/2.0...v2.0.8
- **The v2 tag tree does not contain `packages/opencode`** (the legacy CLI; 404 at `v2.0.8/packages/opencode/package.json`), while the `dev` branch still carries both legacy and v2 packages. v2-tag package identities: `@opencode/cli` 2.0.8 with bins **`opencode` and `opencode2`**; `@opencode/server`, `@opencode/sdk`, `@opencode/client`, `@opencode/protocol` all 2.0.8 (scope renamed from `@opencode-ai/*` on dev). Sources: https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/packages/cli/package.json , https://api.github.com/repos/anomalyco/opencode/git/trees/v2.0.8
- Nix packages v2 under an `opencode2` alias: commit "fix(nix): install opencode and keep opencode2 alias (#48662)" in the v2.0.3→v2.0.8 range. Source: https://api.github.com/repos/anomalyco/opencode/compare/v2.0.3...v2.0.8
- Related npm state: `@opencode-ai/sdk` latest = 1.18.31 (legacy wire SDK; `next`/`beta`/`dev` 0.0.0 snapshot dist-tags); `@opencode-ai/client` (v2 client) exists but latest = **0.0.0** with only snapshot dist-tags; `@opencode-ai/sdk-next` not published. Sources: https://registry.npmjs.org/@opencode-ai%2fsdk , https://registry.npmjs.org/@opencode-ai%2fclient , https://registry.npmjs.org/@opencode-ai%2fsdk-next (404)

### Announcement

- I could **not locate a maintainer announcement post** (no `/blog` on opencode.ai — 404; sitemap has no blog/changelog/announcement page: https://opencode.ai/sitemap.xml ; homepage banner is about desktop tabs, not v2: https://opencode.ai/ ). The v2 "launch" is evidenced by the v2.0.0 tag/release above. See "Confidence + gaps".

---

## 2. Server mode in v2 (`opencode serve`)

### Flags (documented on opencode.ai — these pages track the current shipping CLI)

- `opencode serve [--port] [--hostname] [--cors]` plus `--mdns` / `--mdns-domain`; defaults **port 4096, hostname 127.0.0.1**, `--cors` repeatable. Source: https://opencode.ai/docs/server
- Same flags on the CLI reference page for `serve`, `web`, `acp`, and the TUI's embedded server; `opencode web` = headless server + web UI; `opencode attach <url>` attaches a TUI to a remote serve/web backend. Source: https://opencode.ai/docs/cli
- When you run `opencode` (TUI) it starts TUI + server; TUI is a client of the server; `/tui/*` endpoints let IDE plugins drive the TUI. Source: https://opencode.ai/docs/server

### Auth

- **HTTP Basic auth, enabled by setting `OPENCODE_SERVER_PASSWORD`**; username defaults to `opencode`, override with `OPENCODE_SERVER_USERNAME`; applies to `opencode serve` and `opencode web`. Sources: https://opencode.ai/docs/server , https://opencode.ai/docs/cli (env-var table)
- v2 server implementation (source-verified): `packages/server/src/auth.ts` reads `OPENCODE_SERVER_PASSWORD` (optional) and `OPENCODE_SERVER_USERNAME` (default `opencode`) via Effect Config; **auth is disabled when no password is set** (`required()` returns false for absent/empty password); `authorized()` compares Basic credentials. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/server/src/auth.ts
- v2 authorization middleware also accepts credentials via **`?auth_token=<base64 user:pass>` query parameter** (for cases like browser/SSE that can't set headers), replies `401` with `WWW-Authenticate: Basic realm="Secure Area"`, and **skips the header check for ticketed PTY WebSocket connects** (`/api/pty/:ptyID/connect` via connect-token). Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/server/src/middleware/authorization.ts
- Client-side: `opencode attach` and `opencode run --attach` expose `--password/-p` (default `OPENCODE_SERVER_PASSWORD`) and `--username/-u` (default `OPENCODE_SERVER_USERNAME` or `opencode`). Source: https://opencode.ai/docs/cli

### Spec + SDK

- **v1 (1.18.x) server**: publishes an **OpenAPI 3.1 spec at `GET /doc`** (HTML/Swagger viewer) and the docs say this endpoint "is also used to generate an SDK". Sources: https://opencode.ai/docs/server , https://opencode.ai/docs/sdk
- **v2 server**: the router is built with `HttpApiBuilder.layer(Api, { openapiPath: "/openapi.json" })` — the machine-readable spec lives at **`/openapi.json`**. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/server/src/routes.ts
- A generated **`packages/protocol/openapi.json` is checked into the repo** (139 endpoints as of the audit, regenerated 2026-09-13). Source: https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/V2_HTTP_API_AUDIT.md (header: "Source: `packages/protocol/openapi.json`", "Current endpoint count: 139")
- Generated clients: `packages/client` ships Promise + Effect emitters from an "SDK Contract IR" (`@opencode-ai/client` root = zero-Effect Promise client; `/effect` = Effect client); `packages/sdk-next` is the embedded in-process host that "will assume the existing `@opencode-ai/sdk` name after legacy consumers migrate". Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/CONTEXT.md ("Client contract architecture" section), https://github.com/anomalyco/opencode/tree/dev/packages/client , https://github.com/anomalyco/opencode/tree/dev/packages/sdk-next
- The documented (website) SDK `@opencode-ai/sdk` still describes the **v1 wire** (`createOpencode`/`createOpencodeClient`, `client.session.prompt(...)`, `client.event.subscribe()`); its event example iterates `event.type, event.properties` (v1 envelope). Source: https://opencode.ai/docs/sdk

---

## 3. The v2 HTTP API surface (as of 2026-09-18)

### Where it is documented

- **Canonical, current**: `V2_HTTP_API_AUDIT.md` at the v2.0.8 tag — a full audit of all 139 endpoints with method/path/operation-ID/stability decision ("Keep"/"Change"/"Remove"/"Experimental-only"). Source: https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/V2_HTTP_API_AUDIT.md
- **Route truth**: `packages/protocol/src/groups/*.ts` (Effect endpoint definitions with literal paths). Dev-branch index: https://github.com/anomalyco/opencode/tree/dev/packages/protocol/src/groups
- **v1→v2 migration mapping** (maintainer doc, per-endpoint): `packages/app/V1_API_MIGRATION.md`. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/app/V1_API_MIGRATION.md
- The **website's `/docs/server` page documents the v1/unprefixed API** (`/session/...`, `/global/health`, `/event`, `/tui/...`, `/doc`), **not** the v2 `/api/*` surface — there is no v2 API page on opencode.ai yet (sitemap confirms page list). Sources: https://opencode.ai/docs/server , https://opencode.ai/sitemap.xml

### v2 endpoint catalogue (from `V2_HTTP_API_AUDIT.md` + protocol sources at `v2.0.8`)

Foundation:
- `GET /api/info` (`server.info`) — **replaces `/api/health` + `/api/server`** (audit: "Merge and rename … Returns `version`, `pid`, and connection `urls`; readiness is conveyed by HTTP status"). At **2.0.3 these were still separate**: `GET /api/health` (`health.get`) and `GET /api/server` (`server.get`). Sources: audit (above); https://raw.githubusercontent.com/anomalyco/opencode/v2.0.3/packages/protocol/src/groups/health.ts and `.../groups/server.ts`
- `GET /api/location` (`location.get`); `POST /api/location/reload`
- `GET /api/project` (`project.list`); `PATCH /api/project/{projectID}` (`project.update`). `GET /api/project/current` was **removed** during the audit (use `/api/location`).

Catalogs: `GET /api/agent`, `GET /api/agent/{agentID}`, `GET /api/plugin`, `POST /api/plugin/check`, `POST /api/plugin/update`, `GET /api/model` (`model.list`), `GET /api/model/default`, `GET /api/provider`, `GET /api/provider/{providerID}`, `GET /api/command` (`command.list`), `GET /api/skill` (`skill.list`), `GET /api/reference` (`reference.list`), `GET /api/config` (`config.get`), `GET /api/config/shell`, `PATCH /api/experimental/config` (experimental; initially only `shell`).

Credentials/integrations/MCP/websearch: `GET /api/integration`; `GET /api/integration/{id}`; `POST /api/integration/{id}/connect/key`; `POST /api/integration/{id}/connect/oauth`; `GET|DELETE /api/integration/{id}/connect/oauth/{attemptID}` (status/cancel); `POST /api/integration/{id}/connect/command` + `GET|DELETE .../connect/command/{attemptID}`; `POST /api/experimental/integration/wellknown`; `GET /api/mcp`; `PUT|DELETE /api/experimental/mcp/{server}`; `POST /api/experimental/mcp/{server}/connect|disconnect`; `GET /api/mcp/resource`; `PATCH|DELETE|POST /api/credential/{credentialID}` (update/remove/activate); `GET /api/websearch/provider`; `POST /api/websearch`.

**Sessions** (the core, for our harness):
- `GET /api/session` (`session.list`) — query: `workspace?`, `limit?` (default newest 50), `order=asc|desc`, `search?`, `directory?` (absolute path), `project?`, `subpath?`, `cursor?`; response `{ data: Session.Info[], cursor: { previous?, next? } }` (opaque base64url cursors). Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/groups/session.ts
- `POST /api/session` (`session.create`) — payload `{ id?, agent?, model?, location? }`
- `GET /api/session/active` (`session.active`) — server-scoped snapshot `{ data: Record<sessionID, { type: "running" }> }`; "Sessions absent from the result are inactive" (replaces v1 `GET /session/status`)
- `GET /api/session/{sessionID}` / `DELETE ...` (`session.get`/`session.remove`; typed 404 `SessionNotFoundError`)
- `POST /api/session/{sessionID}/fork` (`session.fork`; optional `before` message ID)
- `POST /api/session/{sessionID}/agent` (`session.switchAgent`) and **`POST /api/session/{sessionID}/model`** (`session.switchModel`, payload `{ model: Model.Ref }`, `204 No Content`) — see §6
- `PATCH /api/session/{sessionID}` (`session.update`; title + permissions; subsumes 2.0.3's `POST .../rename`)
- `POST /api/session/{sessionID}/move`, `POST .../background`
- **`POST /api/session/{sessionID}/prompt`** (`session.prompt`) — payload `{ id?, prompt: PromptInput.Prompt, delivery?, resume? }`; "Durably admit one session input and schedule agent-loop execution unless `resume` is false"; errors `ConflictError`, `SessionNotFoundError`. There is **no `/prompt_async` in v2** — prompt is always admit+schedule (async by design); v1's `POST /session/:id/prompt_async` is the legacy wire (see migration doc).
- `POST /api/session/{sessionID}/command` (slash command; request field renamed `command`→`name` during audit), `/synthetic`, `/shell`, `/compact`
- `POST /api/session/{sessionID}/interrupt` (`session.interrupt`) — idempotent ("Idle interruption is a no-op"); option renamed `continue`→`resume` during audit. Replaces v1 `POST /session/:id/abort`.
- `POST /api/experimental/session/{sessionID}/wait` (idle barrier), `GET /api/experimental/session/stats`, `POST /api/experimental/session/import`, `GET /api/experimental/session/{sessionID}/export`, `POST /api/experimental/session/{sessionID}/skill`, `GET /api/experimental/session/{sessionID}/log`, instructions entries (`GET|PUT|DELETE /api/experimental/session/{sessionID}/instructions/entries[/{key}]`)
- History/recovery: `POST /api/session/{sessionID}/revert/stage`; `DELETE /api/session/{sessionID}/revert` (clear — was `POST .../revert/clear` at 2.0.3); `POST .../revert/commit`; `GET /api/session/{sessionID}/context` (active model-context projection); `GET /api/session/{sessionID}/diff`; `GET /api/session/{sessionID}/message` (`session.message.list`) and `GET .../message/{messageID}` (`session.message.get`)
- Inbox/forms/permissions/questions:
  - `GET /api/session/{sessionID}/inbox`; `DELETE|PATCH /api/session/{sessionID}/inbox/{inboxID}` (cancel; `delivery: "steer"|"queue"`)
  - `GET /api/form`; `GET|POST /api/session/{sessionID}/form`; `GET .../form/{formID}`; `POST .../form/{formID}/reply`; `DELETE .../form/{formID}`
  - `GET /api/permission/request` (pending, global); `GET /api/permission/saved`; `DELETE /api/permission/saved/{id}`; `POST /api/session/{sessionID}/permission` (create/ask); `GET /api/session/{sessionID}/permission`; `GET .../permission/{requestID}`; `POST .../permission/{requestID}/reply` (request field renamed `reply`→`decision` during audit)
  - Questions (v2 equivalent of ask/reply): `GET /api/question/request`; `GET /api/session/{sessionID}/question`; `POST /api/session/{sessionID}/question/{requestID}/reply`; `POST .../question/{requestID}/reject`. Source (dev snapshot): https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/groups/question.ts

Filesystem/VCS/worktrees: `GET /api/fs/read/*`, `GET /api/fs/list`, `GET /api/fs/find`, `POST /api/experimental/fs/write` (added 2026-09-17, commit "feat(server): add fs.write endpoint (#49466)"); `GET|POST|DELETE /api/worktree`, `POST /api/worktree/refresh`; `GET /api/vcs`, `GET /api/vcs/base`, `GET /api/vcs/status`, `GET /api/vcs/branch`, `GET /api/vcs/diff`.

PTY/shells: `GET|POST /api/pty`; `GET|PUT|DELETE /api/pty/{ptyID}`; `POST /api/pty/{ptyID}/connect-token`; `GET /api/pty/{ptyID}/connect` (WebSocket); plus the experimental persistent-PTY family under `/api/experimental/session/{sessionID}/terminal*` and `/api/experimental/persistent-pty/*`; `GET|POST /api/shell`, `GET|DELETE /api/shell/{id}`, `GET /api/shell/{id}/output`.

Events/RPC/debug/migration: `GET /api/event` (`event.subscribe`, SSE — see §4); `POST /api/rpc/{rpcID}/{method}` (plugin RPC); `GET|DELETE /api/debug/location`; `GET /api/experimental/migration/v1` (v1-migration status); `POST /api/experimental/generate` (stateless generation) and `POST /api/session/{sessionID}/generate` (transient generation from session context, "marked stateless generation experimental" on 2026-09-16).

Sources for this catalogue: https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/V2_HTTP_API_AUDIT.md (all audit rows); v2.0.8 route extraction from `packages/protocol/src/groups/*` (verified against the tag); dev-branch files cited inline.

### The v1 (unprefixed) surface our harness speaks — confirmed still present in the 1.18.x/dev hybrid server

From `packages/opencode/src/server/routes/instance/httpapi/*` (dev): root API `GET /global/health`, `GET /global/event` (SSE), `GET|PATCH /global/config`, `POST /global/dispose`, `POST /global/upgrade`, `PUT|DELETE /auth/:providerID`, `POST /log`; instance API `/session` family incl. `prompt`, **`promptAsync`**, `abort`, `command`, `shell`, `revert`/`unrevert`, `summarize`, `permissions`, plus `/path`, `/vcs*`, `/command`, `/agent`, `/skill`, `/lsp`, `/formatter`, `/instance/dispose`, `/event`, `/experimental/control-plane/move-session`. Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/opencode/src/server/routes/instance/httpapi/groups/global.ts , `.../groups/instance.ts` , `.../groups/session.ts` , `.../groups/control.ts` , `.../groups/event.ts` , `.../groups/control-plane.ts ; aggregator: `.../httpapi/api.ts` (https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/opencode/src/server/routes/instance/httpapi/api.ts — note it composes the v2 `ServerApi` from `@opencode-ai/protocol` **plus** these legacy groups, i.e. the 1.x server is a hybrid serving both wires).
The website's table of this wire (incl. `POST /session/:id/message` sync, `POST /session/:id/prompt_async` → `204`, `GET /session/status`, permission respond `POST /session/:id/permissions/:permissionID`): https://opencode.ai/docs/server

### v1→v2 mapping (migration doc, selected)

- `GET /global/event` → `GET /api/event`; `GET /session/status` → `GET /api/session/active` + execution events; `POST /session/:id/abort` → interrupt; `/session/:id/summarize` → compact API; `GET /permission` → `GET /api/permission/request`; `GET /question` → `GET /api/question/request`; question reply/reject → `/api/session/:sessionID/question/:requestID/*`; `/pty*` → `/api/pty*`; `GET /path` → `GET /api/path` (pre-audit; now `/api/location`); `GET /project/current` → `GET /api/project/current` (pre-audit; now removed in favor of `/api/location`); `GET /vcs*` → `GET /api/vcs*`; `GET /provider/auth` → `GET /api/integration/:integrationID`; OAuth → `/api/integration/:id/connect/oauth/*`; `GET /experimental/session` (search) → `GET /api/session`. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/app/V1_API_MIGRATION.md
- App is explicitly **hybrid with v1 fallbacks** ("These calls are retained as fallback adapters… Remove fallback `GET /session/:sessionID` after compatibility support is unnecessary") and sharing is "Blocked: the current API has no sharing contract or implementation" (v1 share endpoints unmigrated). Same source.

---

## 4. Event / SSE protocol (v2)

### Endpoints and semantics

- **Global live stream: `GET /api/event`** (`event.subscribe`, SSE). Union schema = server event manifest definitions **plus `server.connected`** ("First event is `server.connected`, then bus events" is the v1 doc phrasing; v2 code adds `server.connected` if absent). Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/groups/event.ts , https://opencode.ai/docs/server (v1 `/event` row)
- **Session-scoped durable stream: `GET /api/session/{sessionID}/event`** — query `after` (aggregate sequence): "Replay durable events after an aggregate sequence, then continue with new durable events"; SSE of `SessionEvent.Durable`. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/groups/session.ts
- **Durable paging: `GET /api/session/{sessionID}/history`** — `{ data: SessionEvent.Durable[], hasMore }`, query `limit ≤ 100`, `after`. Same source.
- **No auto-reconnect, no replay on the live stream**: `events.subscribe()` "has no replay guarantee"; transport loss fails with `ClientError`; consumers refresh authoritative state then resubscribe; durable resume is explicit composition over `sessions.events({ sessionID, after })`. A Session ID is *not* an optional filter on the live stream (different schemas/cursors/guarantees). Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/CONTEXT.md (event-stream bullets)
- The live stream is instance/workspace-bounded: "The initial common OpenCode Client does not expose server-global event aggregation. `events.subscribe()` is bounded to the connected OpenCode instance or workspace." Same source.

### Envelope shape

- **v2 envelope: `{ id, type, data, metadata?, durable?: { aggregateID, seq, version }, location? }`** — note the payload key is **`data`**, not v1's `properties`. Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/groups/event.ts (fields) and `.../packages/schema/src/event.ts` (Definition shape: `type`, optional `durable { aggregateID, seq, version }`)
- **v1 envelope: `{ type, properties }`** (plus id) — shown in the official SDK docs example (`event.type, event.properties`) and in the hybrid server's additional schema `{ id, type, properties: definition.data }`. Sources: https://opencode.ai/docs/sdk (Events), https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/opencode/src/server/routes/instance/httpapi/api.ts

### Event types

- **v2 session events are the `session.next.*` family** (durable, aggregated by `sessionID` with `seq`): `session.next.agent.switched`, `model.switched`, `moved`, `prompted`, `prompt.admitted`, `context.updated`, `synthetic`, `shell.started`, `shell.ended`, `step.started`, `step.ended`, `step.failed`, `text.started`, `text.delta`, `text.ended`, **`reasoning.started` / `reasoning.delta` / `reasoning.ended`** (thinking), `tool.input.started/delta/ended`, `tool.called`, `tool.progress`, `tool.success`, `tool.failed`, `retried` (retry; identifier `session.next.retry_error`), `compaction.started/delta/ended`, `revert.staged/cleared/committed`. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/session-event.ts
- Non-session v2 server events come from the manifest inventory: ModelsDev, Integration, Catalog, FileSystem, Reference, **Permission**, Plugin, ProjectDirectories, FileSystemWatcher, Pty, **Question**, SessionTodo (this is `ServerDefinitions` = what the standalone v2 server streams); the fuller `Definitions` (used by the hybrid/embedded setups) adds Installation, Lsp, PermissionV1, Tui, Mcp, Legacy, Project, **SessionStatusEvent** (`session.status`, `session.idle`), QuestionV1, SessionCompaction, Vcs, Workspace, Worktree, ServerEvent. Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/event-manifest.ts , `.../src/server-event.ts` (`server.connected`), `.../src/session-status-event.ts` (`SessionStatus` union includes `idle`/`retry`/`busy`; legacy `session.status`/`session.idle`)
- **v1 event names being retired** (listed as legacy in the migration doc): `session.created`, `session.updated`, `session.diff`, `session.status`, `session.idle`, `session.error`; `message.updated`, `message.removed`, `message.part.updated`, `message.part.removed`, `message.part.delta`; also `pty.exited`, file-watcher and VCS events. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/app/V1_API_MIGRATION.md (Events section)
- **Subagents/child sessions**: no dedicated v2 subagent event type appears in the schema; subagents surface as child sessions (v1 hybrid had `GET /session/:id/children`; v2 has session `parentID`/`background` semantics — "Backgroundable foreground tools transition to background observation") and CONTEXT.md states "background subagents and tasks do not make their parent Session active". Recent commit "fix(tui): open completed subagent sessions (#49675)". Sources: https://opencode.ai/docs/server (v1 `/session/:id/children` row), audit row 058 (https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/V2_HTTP_API_AUDIT.md), https://raw.githubusercontent.com/anomalyco/opencode/dev/CONTEXT.md , https://api.github.com/repos/anomalyco/opencode/compare/v2.0.3...v2.0.8
- **Permission/question events**: v2 has Permission and Question event modules in the manifest (above); a post-2.0.3 commit "refactor(session): simplify permissions event" (2026-09-15) touched this area; the v2 permission request lifecycle is also pollable via `/api/permission/request` + `/api/session/{id}/permission*`. Sources: manifest URL above, compare URL above, audit Group 7.
- **Context usage**: v2 exposes it via `GET /api/session/{sessionID}/context` (active model-context projection) and compaction events; the web app has a context-usage component (commit "fix(app): shorten context usage label (#48927)"). Sources: protocol session.ts (above), https://api.github.com/repos/anomalyco/opencode/compare/v2.0.3...v2.0.8

---

## 5. Versioning / compatibility policy

- **The v2 wire is explicitly unstable and changed inside the 2.0.x patch series.** 207 commits between v2.0.3 (2026-09-12) and v2.0.8 (2026-09-18), including breaking protocol refactors: "remove current project endpoint", "remove v2 operation prefixes", "remove workspace API", "consolidate server status", "simplify response locations", "remove preferences API", "refine audited endpoints", "simplify session actions", "rename interrupt resume option", "mark transient session controls", "normalize session resources", "refine vcs and shell APIs", "mark stateless generation experimental". Source: https://api.github.com/repos/anomalyco/opencode/compare/v2.0.3...v2.0.8
- Concrete wire deltas 2.0.3→2.0.8 (verified by diffing protocol sources at both tags):
  - OpenAPI operation IDs lost the `v2.` prefix (`v2.session.list` → `session.list`). Sources: https://raw.githubusercontent.com/anomalyco/opencode/v2.0.3/packages/protocol/src/groups/session.ts vs https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/packages/protocol/src/groups/session.ts
  - `GET /api/health` + `GET /api/server` → `GET /api/info`. Sources: 2.0.3 groups (above) + audit "Merge and rename".
  - `/api/session/stats`, `/api/session/import`, `/api/session/{id}/export`, `/api/session/{id}/skill`, `/api/session/{id}/wait` moved under `/api/experimental/*`.
  - `POST /api/session/{id}/rename` → `PATCH /api/session/{id}` (`session.update`).
  - `POST .../revert/clear` → `DELETE /api/session/{id}/revert`.
  - Field renames: permission `reply`→`decision`; session.command `command`→`name`; interrupt `continue`→`resume`.
  - Audit disposition removes: `POST /api/plugin/await-activation`, `GET|PATCH /api/config/preferences`, `POST|DELETE /api/workspace{...}`, `GET /api/project/current`.
  All from the two session.ts tags + https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/V2_HTTP_API_AUDIT.md
- **Stability tiers are now codified** by the audit: each endpoint is Keep/Change/Remove/Experimental-only; "Experimental-only: retain outside the stable API commitment" (i.e. `/api/experimental/*` endpoints carry no stability promise). Source: audit (disposition legend).
- **1.x↔2.x coexistence**: the 1.18.x server is a *hybrid* — the legacy unprefixed API and the v2 `/api/*` API are served side-by-side from the same router (`OpenCodeHttpApi = Root + EventApi + InstanceHttpApi + ServerApi + PtyConnectApi`, where `ServerApi` is the protocol v2 API), and the web app keeps v1 fallback adapters. Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/opencode/src/server/routes/instance/httpapi/api.ts , https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/app/V1_API_MIGRATION.md
- The v2 lineage **deleted the legacy package** (no `packages/opencode` at v2.0.8) — so on the 2.0.x server there is no v1 compatibility layer; v1 compat lives only in the 1.18.x product. Sources: https://api.github.com/repos/anomalyco/opencode/git/trees/v2.0.8 (package list), 404 at https://raw.githubusercontent.com/anomalyco/opencode/v2.0.8/packages/opencode/package.json
- No formal deprecation notice page for v1 endpoints exists on the website (sitemap has no migration/deprecation page); the de-facto deprecation list is the unchecked "Remove …" items in the app migration checklist. Sources: https://opencode.ai/sitemap.xml , migration doc.
- **How clients track the server**: version comes from the server endpoints — v1: `GET /global/health` → `{ healthy: true, version: string }`; v2 (2.0.3): `/api/health` / `/api/server`; v2 (2.0.8+): `GET /api/info` → `{ version, pid, urls }`. The v2 client/SDK is generated from the server's own OpenAPI (`SDK Contract IR`; "Server's concrete HttpApi is authoritative for shared OpenCode Client capabilities"), and the desktop app got a "Servers tab in Settings" plus fixes like "fix(client): replace servers missing status endpoint" (2026-09-13) — clients probe the server's status/info endpoint and regenerate per release. Sources: https://opencode.ai/docs/server , audit , https://raw.githubusercontent.com/anomalyco/opencode/dev/CONTEXT.md , compare URL, https://github.com/anomalyco/opencode/releases/tag/v1.16.0 (Servers tab)
- Protocol versioning of durable events: each durable event definition carries `version` (envelope `durable.version`), and the manifest resolves duplicate types by highest durable version — a built-in event-schema versioning mechanism. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/event.ts

---

## 6. Session model setting, directory scoping, OPENCODE_CLIENT, headless config

- **Session-scoped model setting exists**: `POST /api/session/{sessionID}/model` (`session.switchModel`, payload `{ model: Model.Ref }`, 204; "Switch the model used by subsequent provider turns"), alongside `POST /api/session/{sessionID}/agent` (`switchAgent`). Model refs can include a variant (audit row 048/055: "model reference includes optional variant"). Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/protocol/src/groups/session.ts , audit. (Contrast: v1 wire sets model per-prompt via `POST /session/:id/message` body `model: { providerID, modelID }` — https://opencode.ai/docs/sdk.)
- **Directory-scoped requests (v2)** — two carriers, query first, header fallback, `process.cwd()` default (source-verified):
  - query params `location[directory]` (absolute path) and `location[workspace]`
  - headers **`x-opencode-directory`** (URI-decoded) and `x-opencode-workspace`
  - Session-scoped routes (`/api/session/{sessionID}/...`) **resolve location from the session row** (its stored `directory`/`workspace_id`), not from the request; invalid session ID → `InvalidRequestError`, unknown → `SessionNotFoundError`.
  - `GET /api/session` also accepts `directory`/`project`/`subpath` filters in its own query schema.
  Sources: https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/server/src/location.ts , `.../server/src/middleware/session-location.ts` , session.ts query schemas (above)
  - Response side: generic endpoints wrap responses with a location object, reduced by the audit to `{ directory }` ("Reduce generic endpoint response locations to `{ directory }`; full project metadata remains available from `GET /api/location`"). Source: audit.
- **`OPENCODE_CLIENT`** — documented env var: "Client identifier (defaults to `cli`)". Source: https://opencode.ai/docs/cli (Environment variables table). Its server-side effect is not documented further on the site (gap).
- **Headless-relevant config**:
  - Env vars (documented): `OPENCODE_CONFIG` (path), `OPENCODE_CONFIG_CONTENT` (inline JSON), `OPENCODE_CONFIG_DIR`, `OPENCODE_PERMISSION` (inline JSON permissions), `OPENCODE_DISABLE_DEFAULT_PLUGINS`, `OPENCODE_DISABLE_AUTOUPDATE`, `OPENCODE_DISABLE_LSP_DOWNLOAD`, `OPENCODE_DISABLE_MODELS_FETCH`, `OPENCODE_DISABLE_AUTOCOMPACT`, `OPENCODE_SERVER_PASSWORD`/`OPENCODE_SERVER_USERNAME`, `OPENCODE_CLIENT`, `OPENCODE_MODELS_URL`; experimental: `OPENCODE_EXPERIMENTAL_NATIVE_LLM` ("Enable native LLM request path"), `OPENCODE_EXPERIMENTAL_WORKSPACES`, `OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS`, `OPENCODE_EXPERIMENTAL_EVENT_SYSTEM`. Source: https://opencode.ai/docs/cli
  - v1 reads supported v2 config fields since v1.18.24 ("so newer config files keep working in more mixed setups"). Source: https://github.com/anomalyco/opencode/releases/tag/v1.18.24
  - v2 config endpoints: `GET /api/config` ("response now contains only documents and OpenCode directories"), `GET /api/config/shell`, `PATCH /api/experimental/config` (initially only `shell`); global config preferences API removed by audit. Source: audit Group 2.
  - Headless serving: `opencode serve` (API only) and `opencode web` (API + web UI), both basic-auth capable; `opencode run --attach <url>` drives a remote server with `--dir` ("path on the remote server when attaching"), `--format json` (raw JSON events), `--thinking`, `--auto`. Source: https://opencode.ai/docs/cli
  - Ambient project config discovery honors `OPENCODE_DISABLE_PROJECT_CONFIG`; global instructions remain eligible. Source: https://raw.githubusercontent.com/anomalyco/opencode/dev/CONTEXT.md

---

## Confidence + gaps (what to verify against source/binaries)

**High confidence** (multiple primary sources agree): v2 timeline and dual release lines; repo move to `anomalyco/opencode`; v2 = Effect-TS HttpApi under `/api/*` with `/openapi.json`; v1 = unprefixed wire still documented at opencode.ai/docs/server and served by 1.18.x hybrid; basic-auth model incl. `?auth_token=`; directory carriers (`location[directory]` query / `x-opencode-directory` header / session-row resolution); v2 event envelope `{id, type, data, durable?}` vs v1 `{type, properties}`; `session.next.*` event family; the 139-endpoint audit table; breaking changes 2.0.3→2.0.8.

**Could NOT verify from primary sources — check the code/binaries**:

1. **No maintainer announcement post for v2** (no blog on opencode.ai; no changelog page; Discord/X not fetchable). Web-search summaries claimed a "rewrite from scratch"/"Rust rewrite" but returned no citable primary URL and conflict with the source code (Effect-TS server). If an announcement exists it is on Discord/X — worth asking in https://opencode.ai/discord.
2. **Exact v2 Docker image name/tag and entrypoint** (v2.0.0 commit says "use V2 Docker artifact paths"; publish.yml logs into `ghcr.io` and builds with buildx, but I did not extract the final image name, e.g. `ghcr.io/anomalyco/opencode-server`). Check `packages/containers/publish/Dockerfile` + publish.yml at the `v2` branch, or `docker pull` candidates.
3. **Whether the 2.0.8 binary's `serve` CLI flags are identical to the documented ones** (`--port/--hostname/--mdns/--mdns-domain/--cors`). The docs track the shipping CLI (1.18.31-era); I verified flags in docs and the v1 `serve.ts` (`withNetworkOptions`), not the 2.0.8 `packages/cli` implementation.
4. **Runtime frame shapes actually emitted on the wire** (SSE `event:`/`data:` framing, heartbeat events, `id:` fields). Source defines the payload schema; framing details (and any heartbeat/keepalive cadence) need a live capture against 2.0.8. CONTEXT.md mentions "connection, heartbeat, and instance-disposal lifecycle events" on the live stream but the schema files I read don't enumerate them.
5. **Which `session.next.*` events the 2.0.8 binary actually emits for subagents/child sessions** (no subagent-specific event type found; `session.next.moved`/`background` endpoint semantics need a live test).
6. **`OPENCODE_CLIENT`'s effect on the server** (docs say "client identifier (defaults to cli)"; usage not traced in source during this pass — grep the repo for it).
7. **v2 SDK publication state**: `@opencode-ai/client` exists only as `0.0.0` snapshot dist-tags; `@opencode-ai/sdk-next` unpublished; the 2.0.8 tag uses scope `@opencode/*` (`@opencode/sdk` 2.0.8) — whether/where the v2 SDK is installable by end users (vs generated in-repo) is unresolved.
8. **Desktop app ↔ server version negotiation details** (how the app picks v1 fallback vs v2 path per server; "fix(client): replace servers missing status endpoint" implies probing, but the exact capability check is unverified).
9. **`/api/event` stream on the 2.0.x server vs the hybrid 1.x server**: both mount a protocol event group, but the hybrid's additional schema uses `properties` while the standalone uses `data` — I did not live-test whether 1.18.31's `/api/event` and 2.0.8's `/api/event` emit identical envelopes. Verify with `curl -N` against both.
10. **OpenAPI spec URL on the 2.0.8 server** (`/openapi.json` read from dev `routes.ts`; not re-verified at the v2.0.8 tag, and `/doc` HTML viewer may or may not exist there).
