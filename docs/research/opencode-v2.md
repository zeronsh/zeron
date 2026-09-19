# opencode v2 — what zeron must know to support v1 and v2 together

Researched 2026-09-18. Primary sources: the opencode repo cloned at `tmp/opencode`
(checked-out `dev` branch = the 1.x maintenance line) and a worktree of the `v2`
branch at `tmp/opencode-v2` (tip `c076066c3`, version-synced to v2.0.8); release
tags v2.0.0 (2026-09-11) … v2.0.8 (2026-09-18) and v1.18.31 (2026-09-14) share that
history. Companion: `docs/research/opencode-v2-web-research.md` (web-side facts,
every claim URL-cited; written by a separate research pass — where it conflicts
with source read directly, this doc wins and says so).

**TL;DR.** "opencode v2" is a new Effect-TS server stack (`packages/core`,
`server`, `protocol`, `schema`, `client`, `sdk`, npm scope `@opencode/*`) served
under `/api/*` by a separate release line (2.0.x). It removed every v1 route.
zeron's native driver already speaks v1 (verified live against 1.18.31) and the
**early** v2 wire (verified live against 2.0.3) — but the v2 wire broke **within
the 2.0.x patch series**: against any 2.0.4+ server zeron today fails at startup
detection, and two request bodies no longer validate. Supporting "both v1 and v2"
really means supporting **three dialects** (§3), of which zeron covers the first
two.

## 1. What v2 is; branch and release topology

- v2 is **not** a language rewrite of the agent. Server/core/protocol are
  TypeScript on Effect (`effect/unstable/httpapi`), storage is Drizzle+SQLite,
  event-sourced: durable events per-aggregate with `seq`/`version`
  (`tmp/opencode-v2/specs/v2/`, `packages/schema/src/event.ts`). Rust/Tauri
  exists only for desktop packaging. (The "v2 = Rust rewrite" summaries floating
  around search engines are wrong; the web companion could find no primary
  source either.)
- The repo moved `sst/opencode` → `anomalyco/opencode` (old URL redirects; the
  install script in the clone points at `anomalyco/opencode` releases).
- Two parallel, both-active lines (as of 2026-09-18):
  - **`dev` branch = 1.x line.** `packages/opencode` (v1 CLI/TUI/desktop,
    `@opencode-ai/*`), last tag **v1.18.31** (2026-09-14) — still GitHub
    "latest" and `opencode-ai@latest` on npm. Its server is a **hybrid**: the
    legacy unprefixed routes plus an **older fork** of the v2 `/api/*` surface
    (`packages/opencode/src/server/routes/instance/httpapi/api.ts` composes
    `@opencode-ai/protocol`'s ServerApi).
  - **`v2` branch = 2.x line**, releases cut through `beta` (v2.0.0 2026-09-11
    … v2.0.8 2026-09-18, daily cadence). This tree **deletes**
    `packages/opencode` entirely — the 2.x binary serves **no v1 routes**
    (router = v2 `Api` + `/openapi.json` + static web-app fallthrough that 404s
    `/api*`; `tmp/opencode-v2/packages/server/src/routes.ts`,
    `packages/cli/src/services/web-ui.ts`).
- The 2.x binary is the packages `@opencode/cli` 2.0.8 with bins `opencode` and
  `opencode2` (nix also aliases `opencode2`). Distribution is Docker +
  source; npm `latest` still installs 1.18.31. Users will flip to 2.x when it
  becomes the default channel — zeron will meet both for a long time.

## 2. What zeron does today (`crates/harness/src/opencode/mod.rs`)

- Spawns `opencode serve --port <free> --hostname 127.0.0.1` with
  `OPENCODE_SERVER_PASSWORD=<uuid>` (Basic user `opencode`), `OPENCODE_CLIENT=zeron`.
- Detects the wire once at boot (`Protocol::detect`): `/api/health` answering
  JSON **with a `version` field** ⇒ V2; else `/global/health` with `version` ⇒ V1.
  (Rationale, observed live: 1.x answers `/global/health` `{healthy, version}`
  and *also* serves `/api/health` `{"healthy":true}` with no version; 2.0.3
  answered `/api/health` `{healthy, version}`.)
- V1 wire: `/global/event` SSE, `POST /session/{id}/prompt_async`, `/abort`,
  `/command` (`{command, arguments}`), `/session/status`, `/provider`,
  `/command`, `/permission/{id}/reply` (+ session fallback).
- V2 wire (2.0.3-era): `POST /api/session {location:{directory}}`,
  `GET /api/session/{id}`, `POST /api/session/{id}/model {model:{providerID,id,variant?}}`,
  `POST /api/session/{id}/prompt {text, files:[{uri,name}]}`,
  `POST /api/session/{id}/interrupt` (null body),
  `POST /api/session/{id}/command {command, text}`,
  `GET /api/model`, `GET /api/command`, `GET /api/session/active` (map),
  `POST /api/session/{id}/permission/{id}/reply {reply}`,
  `GET /api/event` SSE normalized 1.x-shaped internally
  (`normalize_v2_frame`); directory via `x-opencode-directory`
  (percent-encoded; V1 also gets the query param).
- Registered as `SteeringMode::TurnBoundary` (steers queue → next prompt at
  idle), `deterministic_turn_end = true`.
- Verified live against **1.18.31** and **2.0.3** (fixtures in
  `crates/harness/src/opencode/tests.rs`; fake server speaks both wires).

## 3. The three dialects in the wild

| | **v1 unprefixed** | **early v2** | **current v2** |
| --- | --- | --- | --- |
| Served by | 1.15.x–1.18.x binaries (dedicated routes) | 1.18.x hybrid `/api/*` and 2.0.0–2.0.3 | 2.0.4+ (verified at v2.0.8 source) |
| Health/version | `GET /global/health` `{healthy, version}` | `GET /api/health` `{healthy, version}` | **`GET /api/info`** `{version, pid, urls, paths}` — `/api/health` gone; `healthy` gone; readiness = HTTP status (503 + `retry-after` while starting) |
| Permission reply | `{response}` / `{reply}` | `{reply: "once"\|"always"\|"reject"}` | **`{decision: ...}`** (`reply` renamed; values unchanged) |
| Slash command body | `{command, arguments}` | `{command, text}` | **`{name, text, ...}`** |
| Prompt | `prompt_async`, model per prompt | `{text, files}` | `{text, files}` **unchanged** + new `delivery:"steer"\|"queue"`, `resume`, `id` (idempotency → 409 dup), `agents`, `skills`; response now `{data: inbox-user}` |
| Session create | `POST /session {}` | `{location:{directory}}` | **unchanged** + `id?/title?/agent?/model?/metadata?/permissions?` |
| Set model | — (per prompt) | `POST /api/session/{id}/model` | **unchanged** |
| Interrupt | `POST /session/{id}/abort` | `POST .../interrupt` | same path, **query `?resume=`**, response `{interrupted}` |
| Active map | `GET /session/status` | `GET /api/session/active` (map) | unchanged (map `{id:{type:"running"}}`) |
| Model list | `GET /provider` | `GET /api/model {location,data}` | unchanged wrapper; `Model.Info` fields incl. `limit{context,…}`, `variants[]`, `compatibility.reasoningField`, `status`, `cost[]` |
| Event bus | `GET /global/event` `{type, properties}` | `GET /api/event` `{id,type,data}` | same + `created`, `metadata?`, `location?`, `durable?{aggregateID,seq,version}` envelope, first frame `server.connected`, 15s `: heartbeat` comments, 4096-frame slow-consumer budget |
| Auth | Basic if `OPENCODE_SERVER_PASSWORD` | same | **always-on Basic** — password = `OPENCODE_PASSWORD ?? OPENCODE_SERVER_PASSWORD`, else generated (32B random, printed); username hardcoded `opencode`; `?auth_token=` fallback for headerless clients; `OPENCODE_SERVER_USERNAME` unread by v2 code |

Sources: per-cell diffs verified at
`tmp/opencode-v2/packages/protocol/src/groups/{server,permission,session,model,command,event}.ts`
and `packages/server/src/{auth.ts,location.ts,middleware/authorization.ts}`,
`packages/cli/src/server-process.ts`; early-v2 columns from the 2.0.3 capture in
zeron's tests and `tmp/opencode/packages/protocol/src/groups/health.ts` +
`permission.ts` (`reply` still) on `dev`. The renames landed in the
2026-09-13 audit window (repo root `V2_HTTP_API_AUDIT.md`, regenerated 09-13,
between 2.0.3 (09-12) and 2.0.4 (09-16)); `POST /api/experimental/fs/write`
appeared after it.

## 4. Confirmed breakages against current v2 (2.0.4+) — FIXED 2026-09-18

All five items below are implemented in `crates/harness/src/opencode/mod.rs`
(`Protocol` split into `V1 | V2Early | V2`), covered by fake-server tests for
every dialect, and **live-verified end-to-end on 2026-09-18 against real
binaries**: `opencode_turn_probe` completed a streamed "PONG" turn against
**2.0.6** (detection via `/api/info`) and **1.18.31** (detection via
`/global/health`), with raw-wire probes confirming every detection endpoint's
status/body on both binaries. Severity-ordered as found:

1. **Boot detection fails ⇒ every run times out. — FIXED.** The detect
   ladder is now `/api/info` → `/global/health` → `/api/health`
   (version-bearing JSON decides; 2.0.4+ also 503s `/api/info` with
   `retry-after` while booting, which `is_success` already skips). Live
   evidence: 2.0.6 answers `/api/info` `{version:"2.0.6",…}`, 404s
   `/api/health`, and serves HTML on `/global/health`; 1.18.31 answers
   `/global/health` `{healthy,version}`, `/api/health` `{healthy:true}`
   (no version), and HTML on `/api/info` — each dialect resolves exactly
   one probe.
2. **Permission replies rejected. — FIXED.** The reply body is
   dialect-aware: `{reply}` on V1/V2Early, `{decision}` on V2 (`reply_field`
   in the `permission.asked` arm). Verified live on 2.0.6 by the
   schema-vs-route status split (`{reply}` 400s validation, `{decision}`
   reaches the handler).
3. **Slash-command turns rejected. — FIXED.** `command_body()` emits
   `{command, arguments}` / `{command, text}` / `{name, text}` per dialect,
   and the detached command task now logs non-2xx responses (previously the
   failure was silent).
4. **Interrupt with a JSON `null` body — live-OK, unchanged.** The current
   endpoint declares no payload (query-only `?resume=`, response
   `{interrupted}`); the null body was already tolerated on 2.0.3 and the
   endpoint's body handling did not tighten — zeron sends it unchanged and
   ignores the response. (The `?resume=true` interrupt-with-resume option
   remains an unadopted v2 capability.)
5. **Attached-server mode and auth — NOT a production gap.** v2 always
   requires Basic auth (password = `OPENCODE_PASSWORD ?? OPENCODE_SERVER_PASSWORD`,
   else generated + printed; username hardcoded `opencode`). Zeron's
   `with_base_url` attach path is a test seam (no spawn, no auth) and is
   unreachable in production — every real run spawns its own server with
   `OPENCODE_SERVER_PASSWORD` set, which v2 honors. No action needed;
   noted here so the always-on-auth change doesn't read as a break.

Non-breakages verified (do not "fix" these): prompt body `{text, files:[{uri,name}]}`
still validates (FileAttachment gained only optional fields); session-create
`{location:{directory}}` unchanged; `/api/model` wrapper + `limit.context` +
variant fold unchanged; `/api/session/active` still a map (zeron already reads
it as one); `x-opencode-directory` percent-encoded header still the carrier
(session routes resolve location from the session row, so the header is ignored
there — zeron already sends the body `location` on create); SSE `: heartbeat`
comment lines are skipped by zeron's `data:`-prefix parser; `server.connected`
and unknown frame types drop harmlessly in `normalize_v2_frame`; the bus
reconnect + `/api/session/active` re-sync covers the documented
slow-consumer-disconnect contract (4096-frame budget).

## 5. Event-bus deltas that matter (v2.0.3 → v2.0.8 source)

zeron's captured vocabulary is essentially intact; the event family is
durable/event-sourced with an explicit volatile live-stream contract
(`packages/protocol/src/groups/event.ts`: "a slow consumer overflows and fails
the stream, and events during disconnection are missed").

- **Unchanged, still authoritative:** `session.execution.started/.succeeded/
  .failed{error}/.interrupted{reason: user|shutdown|superseded|inactivity}`
  are the turn terminals; `session.step.*`, `session.text.*`,
  `session.reasoning.*`, `session.tool.input.started/.called/.success`,
  `session.usage.updated {cost, tokens{input,output,reasoning,cache{read,write}}}`
  all match what zeron normalizes.
- **Tool failure spelling:** `session.tool.failed` (never `.error` on the v2
  wire — zeron's earlier notes said `.error`; the driver now matches both
  arms, which is correct). It gained `metadata?`, `executed`, `resultState?`.
- **zeron-gap: provider retries — CLOSED.** v1 surfaced retries as
  `session.status{type:"retry",attempt,message}` — zeron's report/abort ladder
  (RETRY_REPORT_ATTEMPT/RETRY_ABORT_ATTEMPT) keys on that. v2's only retry
  surface is **`session.retry.scheduled {sessionID, assistantMessageID,
  attempt, at, error}`** (`packages/core/src/session/runner/retry.ts`); the
  `session.status`/`session.idle` declarations exist in the schema but have
  **no publisher** on the v2 branch (the official client derives busy/idle
  from execution events). The normalizer maps `retry.scheduled` onto the v1
  retry shape (2026-09-18; unit-tested at attempts 3 and 8).
- **zeron-gap: subagent binding — CLOSED.** v2 subagents are child
  sessions: `session.created {…, parentID, title}` (flat payload — confirmed
  at `packages/schema/src/session-event.ts` and captured live), interleaved
  child traffic under the child's sessionID, and the spawning tool links via
  `session.tool.progress {metadata:{sessionID, status}}` +
  `session.tool.success/failed` metadata; background completion lands on the
  parent as `session.synthetic {metadata:{source:"subagent", childID, …}}`.
  The blocker was naming: the 2.x spawn tool is **`subagent`**, not 1.x's
  `task` — both the genus decode and the `register_spawn` gate now accept
  either (2026-09-18). Live-verified on 2.0.6: child traffic arrives as
  `Subagent { parent_tool_use_id: "<parent>:<msg>:<call>", event:
  TextDelta { "SUB-PONG" } }` and the parent's tool result embeds
  `<subagent sessionID="…" state="completed">`. Unadopted remainder:
  `tool.progress`-metadata binding (today's path binds via
  `session.created{parentID}` + description/title matching, proven live);
  `session.synthetic` background-completion wakes.
- **New-but-ignorable:** `session.step.started.started` (dispatch ts, 09-17),
  `session.permissions` (renamed from `.updated`, 09-15), `provider.updated` /
  `model.updated` (catalog refresh signals, 09-14), `location.shutdown`
  (09-16), `session.compaction.*`, `session.revert.*`, `session.inbox.*`,
  `session.shell.*`, `form.*` (v2's structured questions — successor of
  `question.asked`), `session.message.content.updated` (replay-only).
  No todo/plan event exists at all (todo state is REST-only).
- **Enriched envelope** (`created`, `metadata?`, `location?`,
  `durable?{aggregateID,seq,version}`) is additive; zeron reads `type`/`data`
  only, which stays valid.

## 6. v2 capabilities worth adopting — adoption status

- **ADOPTED — Native mid-turn steering.** `POST /api/session/{id}/prompt`
  (and `/command`) accept `delivery: "steer" | "queue"` — steer wakes the
  running turn. Zeron's mailbox now sends live steers this way on both v2
  dialects (boundary fallback for v1 and POST failures); the harness and
  engine descriptor report `StepBoundary` (2026-09-18, live-verified).
  Unadopted remainder: `PATCH /api/session/{id}/inbox/{inboxID}` to flip a
  queued row to steer before delivery.
- **Idempotent prompts.** `id` (client-minted message id) on the prompt body →
  duplicate sends 409 instead of double-charging a turn; maps directly onto
  zeron's at-least-once command ledger ids. Still unadopted.
- **Model at session create** (`model: Model.Ref` in `POST /api/session`) —
  one fewer round-trip than create-then-set (keep set_model for mid-session
  changes; it's unchanged). Still unadopted.
- **Transcript recovery** without replaying the bus:
  `GET /api/session/{id}/message` (+`/{messageID}`, cursor-paginated,
  `limit ≤ 200`) and the durable per-session event log
  `GET /api/experimental/session/{id}/event?after=` / `…/log?follow=` — the
  documented composition for resuming after a dropped SSE stream
  (`specs/v2/event-stream-architecture.md`).
- **Idle barrier:** `POST /api/experimental/session/{id}/wait` — race-free
  "wait for idle" to replace post-interrupt polling.
- **Rename:** `PATCH /api/session/{id}` `{title}` (2.0.3 had `POST .../rename`)
  — useful for zeron's auto-titler if it ever renames server-side.
- **ADOPTED — Context window on v2:** `session.usage.updated` carries no
  provider/model, so the run's picked `(provider, model)` rides the
  synthetic message info and `context_usage_event` joins the catalog's
  `limit.context` (2026-09-18; live 2.0.6 shows
  `ContextUsage { tokens: 9959, window: Some(1048576) }`).
- **`opencode api <operationId>`** exists on the 2.x CLI for ad-hoc probing
  during development.

## 7. Recommended zeron design — status after the 2026-09-18 implementation

1. **DONE — `Protocol` split into `V1 | V2Early | V2`** with the
   `/api/info` → `/global/health` → `/api/health` detection ladder;
   payload spellings (permission `decision`, command `name`) key off the
   dialect. Fake-server tests cover all three dialects
   (`crates/harness/src/opencode/tests.rs`).
2. **DONE — normalizer extended:** `session.retry.scheduled` maps onto the
   v1 retry shape so the report/abort ladder fires on v2 (previously
   v2-only blind spot; unit-tested at attempts 3 and 8).
3. **DONE — per-dialect request builders** (`command_body`, permission
   `reply_field`); the detached command task now logs non-2xx answers.
4. **DONE (2026-09-18) — steering upgrade.** The mailbox now forwards live
   steers on both v2 dialects as prompt POSTs with `delivery:"steer"`
   (fallback: queue at boundary on POST failure, always on v1), and the
   harness + engine descriptor flipped to `SteeringMode::StepBoundary`.
   Live-verified on 2.0.6 (`scratch steer-2.0.6.log`): steer posted 3.5s
   into a running counting turn (no idle before it), `Steered` split
   mid-stream, final text honors the steer (`STEERED-WINS`), turn
   Completed. The 1.18.x hybrid fork's `SessionDelivery.Delivery` is
   `Schema.Literals(["steer","queue"])`
   (`tmp/opencode/packages/schema/src/session-delivery.ts`) — identical to
   the current branch, so early-dialect sends validate.
5. **DONE — live verification matrix** (2026-09-18, this machine):
   `opencode_turn_probe` against real 2.0.6 and 1.18.31 binaries (full
   streamed turns, "PONG"), raw-wire probes of every detection endpoint on
   both, session create + set_model (204) + prompt admission + command
   listing + the permission `{reply}`-400 / `{decision}`-404 split on
   2.0.6, and a captured SSE turn showing the exact vocabulary
   (`execution.*`, `inbox.*`, `step.*`, `reasoning.*`, `text.*`,
   `usage.updated`, `server.connected` first frame). The second pass
   (2026-09-18, same day) closed the remaining live gaps on the installed
   2.0.6 with the free `opencode/muse-spark-1.3-contributor-free` model:
   turn probe ×2 with non-null ContextUsage window, a real
   `permission.asked` round-trip (workspace `permissions` rule with
   `effect:"ask"` → question surfaced through `request_input` →
   `{decision:"once"}` accepted → tool executed → file written → turn
   Completed), a subagent spawn rendering as Subagent-tagged traffic bound
   to the spawn chip, and a mid-turn steer injected via `delivery:"steer"`
   3.5s into a live turn. Captures live with the session scratch
   (`turn-2.0.6-{1,2}.log`, `permission-2.0.6.log`, `subagent-2.0.6.log`
   + the two pre-fix attempts, `steer-2.0.6.log`).
6. **DONE (partially) — fail-soft detection:** unknown dialects fall back
   to `V1` after the readiness loop, unchanged. Watching drift via
   `V2_HTTP_API_AUDIT.md` + `packages/protocol/openapi.json` remains a
   manual step per release.

## 8. Source index (pinned)

- v2 line, tip `c076066c3` ("sync release versions for v2.0.8", 2026-09-18):
  `tmp/opencode-v2/packages/protocol/src/groups/{server,session,permission,
  command,model,event,location}.ts`; `packages/schema/src/{session-event,
  event-manifest,session-inbox,prompt-input,permission,token-usage,session,
  model}.ts`; `packages/server/src/{routes,auth,location,event-feed,process}.ts`
  + `middleware/{authorization,session-location}.ts`;
  `packages/cli/src/{server-process,commands/commands}.ts`; `specs/v2/`;
  root `V2_HTTP_API_AUDIT.md`, `packages/protocol/openapi.json`.
- v1 line, `dev` (= 1.18.31 era): `tmp/opencode/packages/opencode/src/server/
  routes/instance/httpapi/**` (v1 routes + hybrid composition in `api.ts`);
  `tmp/opencode/packages/protocol/src/groups/{health,permission,session}.ts`
  (the hybrid's older v2 fork: `/api/health`, `reply`).
- Tags in the shared history: `v2.0.0` 2026-09-11 · `v2.0.3` 2026-09-12 ·
  `v2.0.4` 2026-09-16 · `v2.0.8` 2026-09-18 · `v1.18.31` 2026-09-14.
  The audit-window renames (`/api/info`, `decision`, `name`, `PATCH session`)
  landed between v2.0.3 and v2.0.4 (audit doc regenerated 2026-09-13).
- zeron side: `crates/harness/src/opencode/{mod.rs,tests.rs}`,
  `crates/engine/src/registry.rs` (opencode descriptor), live probes in
  `crates/harness/examples/opencode_{turn,models}_probe.rs`.
