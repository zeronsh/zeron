# Cloud device and credential vault

Status: v1 implementation (2026-10-01). Supersedes the "restricted runners are deferred"
note in the encrypted-sync RFC for the Cloud device only.

## Product shape

**Cloud is a place a session runs, and every Cloud session gets its own machine.** It is a
checkout option, next to "Current checkout" and "New worktree": the checkout picker offers
**Cloud** for any project whose GitHub repository (the folder's `origin`) the Zeron GitHub App
reaches. A session started there runs in its own sandbox — which clones the repository, sleeps
when the session is idle and wakes when the chat is sent to — instead of on the computer the
project lives on, which may then be offline. Cloud has its own providers: a provider works there
only once it has been connected *for Cloud* (Settings → Cloud). Cloud is not a device: no device
list shows it.

- v1 providers on Cloud: **Codex** and **Claude Code** only.
- v1 repositories on Cloud: **GitHub repositories** the user installed the Zeron GitHub App on.
  Each session sandbox clones its project's repository from the branch picked in the composer;
  agents get a one-hour GitHub App installation token for `git push` and `gh pr create`, so
  pushes and pull requests act as the App (see "GitHub").
- Side chats an agent spawns (`parentChatId`) run in their parent session's sandbox: they work
  on the same files.
- Opt-in. Nothing is provisioned, and no credential leaves a device, until the user enables
  Cloud in Settings → Cloud. Local login stays the default everywhere.

## Topology

```
laptop engine ─┐                      ┌─ zeron-vault Worker (service binding only)
               │  WorkOS JWT          │    VaultAccount DO  per (user, provider)
               ├──────────────► edge ─┤    VaultDevices DO  per user (keys, parents, revokes)
session engine ┘  runner JWT   Worker │    secret KEK (rotatable), daily canary cron
 (one sandbox                         │
  per session)                        ├─ CloudAccount DO per (org, user): account,
                                      │    sessions (repo + sandbox + lifecycle each), metering
                                      ├─ CloudIndex DO: every Cloud user (cron, orphan scan)
                                      └─ Workflows per session: provision / wake / sleep / delete
```

Each **session device** is an ordinary Zeron engine (`zeron headless`) running as a systemd
service inside one sandbox ([Boat](https://docs.boat.dev) today). It hosts its own DeviceRoom
and its chat, so transcripts, diffs, terminals, file browsing and side chats work unchanged. We
do not use Boat's integrated agents (`POST /prompt`): they would bypass the session doc, the
command plane and every existing UI surface. Every sandbox is created `noEnv: true` (nothing of
Zeron's Boat account reaches it).

### Devices and ids

| Device | Id | Registry row | Engine |
| --- | --- | --- | --- |
| Logical Cloud device | `cloud-{uuid}`, minted at enable | none — it is the vault parent provider connections are authorized for, and the runner JWT's `cld` | none — never dialed or forwarded to |
| Session device | `cloud-{uuid}`, minted per session | written by its engine: `platform: "cloud"`, capability `cloud-session` | `zeron headless` in the session's sandbox |

Projects stay where they are: a space on the computer whose folder it is, whose owner stamps
`githubRepo` (`owner/name` of the folder's `origin`) next to `gitDetected`, so every device can
offer Cloud without asking that computer. A Cloud session's chat row is `{spaceId: <the
project>, deviceId: <session device>, cwd: /home/user/{repo}}`. Device lists hide session
devices. Catalog requests aimed at a Cloud device (`ListHarnesses`, `ListModels`; the picker
aims at the logical device while Cloud is picked) are answered by the asking engine from its own
catalog restricted to Codex + Claude Code, so opening a composer never wakes a sandbox.

### Session lifecycle

1. **Create.** When a chat is created with Cloud picked (`createChat {cloud: true}`), the
   creating engine looks the project's `githubRepo` up in the repositories Cloud reaches and
   calls `POST /cloud/{org}/sessions {chatId, spaceId, repo, branch?}` (idempotent per chat),
   which mints the session device id, claims its device room for the user and starts the
   ProvisionWorkflow; the chat row is written with that device as host and the machine's
   checkout as cwd. The first send queues in the chat doc as usual and is executed once the
   engine is up.
2. **Provision.** Sandbox created with the enrollment and the project
   (`ZERON_CLOUD_ACCOUNT`, `ZERON_CLOUD_CHAT`, `ZERON_CLOUD_REPO`, `ZERON_CLOUD_PATH`,
   `ZERON_CLOUD_BRANCH`) in its per-sandbox env; the engine installs, enrolls, clones the
   repository with its GitHub grant (public repositories need none) and only then drains the
   chat's commands. With `CLOUD_TEMPLATE` set, sandboxes start from that named snapshot (engine
   pre-installed, no identity) instead of installing.
3. **Sleep.** After `CLOUD_IDLE_MINUTES` without activity (no turn running, no relay requests)
   the session's sandbox is stopped. Archiving a chat lets it sleep.
4. **Wake.** A send to the chat nudges its host device; a nudge to a sleeping session device
   wakes it, and the queued command runs once the engine reconnects. Merely viewing a sleeping
   chat (or its sidebar row) does not wake it: client dials to a sleeping session device get
   the ordinary offline answer, and the UI offers an explicit Wake for files, terminals and
   diffs.
5. **Delete.** Deleting the chat (or Cloud) runs the DeleteWorkflow: revoke the
   session device, stop, capture final usage, delete the sandbox. The transcript stays.

At most `CLOUD_MAX_AWAKE` (default 5) sessions per user are awake at once; waking another puts
the least recently active idle session to sleep first, and fails with a clear error when every
awake session is busy.

## Edge: Cloud control plane (main Worker)

### CloudAccount DO — `cloud1/{orgId}/{userId}`

Single source of truth for one user's Cloud: account state and logical device id; sessions
(`chatId → {deviceId, spaceId, repo {fullName, cloneUrl, defaultBranch, path}, baseBranch?,
sandbox {provider, id}, state, enrollHash, runnerKey, lastTokenTs, activeRuns, lastActiveAt,
error, failedAction, generation}`); the lifecycle ledger and metering rows. Nothing is written
to the registry: Cloud is not a device.

### Runner identity — `POST /runner/enroll`, `POST /runner/token`

Each session machine's engine is its own device and never sees a WorkOS session (WorkOS
refresh tokens are single-use; sharing a laptop's `session.json` would race). Instead:

1. Provisioning passes `ZERON_RUNNER_ENROLL={orgId}.{userId}.{deviceId}.{code}` as per-sandbox
   env. On first boot the engine generates an Ed25519 key (`{data_dir}/runner-key`, 0600) and
   calls `POST /runner/enroll {orgId, userId, deviceId, code, publicKey}`. The DO checks the
   code hash, stores the key, and clears the code (single use). It also enrolls the same key
   with the vault as device kind `cloud`, with the account's logical device as `parentId`, so
   providers authorized for Cloud cover every session machine.
2. Every token refresh is `POST /runner/token {orgId, userId, deviceId, ts, sig}` with
   `sig = Ed25519("zeron-runner-token\n{orgId}\n{userId}\n{deviceId}\n{ts}")`, `ts` in Unix
   milliseconds within ±120 s and strictly greater than the last accepted `ts`. The Worker
   answers with a **runner JWT** (ES256, `RUNNER_JWT_PRIVATE_KEY` secret, 1 h lifetime):
   `{iss: "zeron-edge", sub: userId, org_id, dev: deviceId, cld: accountDeviceId, kind: "runner"}`.
3. `authenticate()` accepts runner JWTs alongside WorkOS JWTs. `Verified` gains
   `deviceId?` and `kind: "user" | "runner"`. Runner tokens are refused on `/auth/*`,
   `/cloud/*` and the vault upload/admin routes, and may host only their own DeviceRoom
   (`/device/{dev}/ws?role=host`).

Deleting a session's machine (or turning Cloud off, or a vault revoke) clears its key, so the
next token refresh fails and that engine goes dark within one token lifetime.

#### Trust boundary

The sandbox runs full-access agents on arbitrary repository code, so anything that can read
`runner-key` can mint runner tokens. A runner token therefore acts as **its session's device**
(the JWT's verified `dev` claim), never as the whole user — and never as a neighbouring
session. Enforced at the edge:

- **Paths** (`edge/src/runner-policy.ts`, 403): no `/auth/*` org routes, no `/cloud/*`, no
  `/vault/*` except `POST /vault/{org}/grant` for its own device. `/device/{id}/*` only for its
  own id; for any other device only `GET /device/{id}/status` (read-only liveness) — no
  client/host sockets, nudges or sidecars. No legacy loro rooms (`/session`, `/tail`,
  `/stats`, `/diff`, `/snapshot`, `/append`, `/workspace`). On `/registry/{org}` only row sync
  (`ws`, `GET rows`, `POST push`) — no `stats`, `push-target` (APNs titles) or `reset`.
- **Chat-scoped data** (`edge/src/runner-access.ts`): `/chat2/{chatId}/*` and
  `/blob/{chatId}/*` only while registry row `chats/{chatId}` is live and its
  `fields.deviceId` equals the runner's device; otherwise 403 `not_host`. Positive answers are
  cached per isolate for ≤5 min; refusals are never cached.
- **Registry** (`edge/src/registry-runner.ts`; the Worker stamps `x-zeron-runner-device`
  on runner forwards and strips any inbound value). Writes: `devices` only its own row;
  `chats`, `spaces`, `sessions` only when the existing live row is hosted by the runner AND
  the row after the op still is (no claiming, handing away or ownerless rows); deletes need a
  live owned row; tombstones are never revived; every other kind (`preferences`,
  `sidebarPins`, anything new) is refused. Refused ops are skipped individually (an
  `{t:"error", code:"not_owner"}` frame each, plus `rejected` in the ack); the rest of the
  batch applies. Reads (hello/backfill, live `rows` broadcasts, `GET /rows`): every `devices`
  row, the hosted rows the runner owns, the projects (spaces) of the chats it hosts, and
  tombstones of hosted kinds (they carry no fields); laptop chats, other projects and other
  sessions' chats and session rows — titles, previews, status — never reach it.
  Presence is unfiltered, but a runner socket's presence/attribution device is pinned to its
  verified id.

User bearers are unaffected by all of the above.

### Workflows (Cloudflare Workflows)

Every provider call is a `step.do` with retries; provider configuration errors (e.g. an API
key lacking `sandbox.resume`) fail the step immediately with an operator-facing
`CloudStatus.error`. Secrets never appear in step outputs. Instance ids are deterministic per
(DO, generation), so a double click cannot run two provisions. Each state change is reported
back to the DO, which also re-checks state before and after every wait, so a lost workflow
event costs at most that wait's timeout.

All four run per session (one instance per session lifecycle, ids from the session device +
its generation).

**ProvisionWorkflow** — `create` (`Idempotency-Key: {sessionDeviceId}-g{generation}`,
`noEnv`, `ttlSeconds: CLOUD_TTL_SECONDS`, `machine: CLOUD_MACHINE`, `from: CLOUD_TEMPLATE` when
set, per-sandbox env `ZERON_RUNNER_ENROLL`, `ZERON_EDGE_URL`, `ZERON_DEVICE_NAME="Cloud
session"`, `ZERON_DEVICE_PLATFORM=cloud`, `ZERON_CLOUD_{ACCOUNT,CHAT,REPO,PATH,BRANCH}`) → poll
until ready → `exec` the install script → `waitForEvent("enrolled", 10 min)` (sent by `/runner/enroll`) → `ready`. The
install script (`edge/src/cloud/install-script.ts`) is idempotent POSIX `sh`: it downloads the
Linux release from `{edge}/releases/`, writes `~/.zeron/env` (0600) from the per-sandbox env
(systemd services don't see it), and enables a **system** unit `zeron-cloud.service` running
`zeron headless` as the exec user with `Restart=always`. Enabled units restart on resume
(verified on Boat: a resumed sandbox lands on a new machine in ~4 s with the service up).

**WakeWorkflow** — `resume` → poll until ready → `waitForEvent("online", 5 min)` (the
runner's first token after resume); on timeout it runs one idempotent
`sudo systemctl restart zeron-cloud` before failing.

**SleepWorkflow** — `stop` → poll until stopped. A refused stop is retried (the provider bills
nothing while a stop is refused); it is never forced.

**DeleteWorkflow** — revoke the runner and its vault enrollment → `stop` → wait until stopped →
read and persist final usage (it must report `running: false`; usage is unreadable after
deletion) → `delete` → `off`. A refused stop leaves `error` + `failedAction: "delete"`; a
second DELETE resumes.

Idle policy: the runner posts `POST /runner/heartbeat {activeRuns, clients}` every 60 s, where
`clients` counts relay requests it served since the previous heartbeat (open links and watch
streams alone don't count, so a laptop merely displaying a Cloud chat doesn't keep it awake).
The DO alarm puts the device to sleep after `CLOUD_IDLE_MINUTES` (default 20) without activity.
When the provider requires auto-stop (Boat trial: ≤2 h), the DO re-extends the TTL while runs
are active, and a provider-initiated stop is detected and recorded as sleep.

### Portability

Boat is one implementation of a provider-neutral `SandboxProvider` interface
(`edge/src/cloud/sandbox-provider.ts`): create (idempotent, with per-sandbox env), get
(normalized state), stop, resume, delete, extend TTL, exec, usage, list. Only
`edge/src/cloud/providers/boat.ts` knows Boat's API; `SANDBOX_PROVIDER` selects the
implementation. Everything above it (workflows, the CloudAccount DO, metering) and everything
inside the sandbox (a POSIX install script, a systemd unit, a `zeron headless` runner configured
purely by `ZERON_*` env) is provider-neutral. Sandbox references are stored as
`{provider, sandboxId}`, so a user can hold sandboxes on two providers during a migration and
billing stays exact.

A provider must guarantee: idempotent create, a disk that persists across stop/resume,
enabled systemd units restarting on resume, per-sandbox env visible to exec, and usage readable
until deletion. Moving users needs no migration step: new sessions start on the new provider,
existing ones keep theirs until deleted; transcripts live in sync rooms and repositories on
GitHub, so only unpushed work in an old session's machine is tied to the old provider.

### Metering

Cloud machine time is billed to Zeron by Boat and will be charged on to users, so every
second is attributed to a user and reconciled against Boat's own meter rather than estimated
from our lifecycle events.

- **Authoritative numbers** come from Boat's `GET /sandboxes/{id}/usage?since&until`
  (billable seconds with the machine-type multiplier applied, stopped time excluded, period
  boundaries split pro rata).
- The CloudAccount DO keeps the user's **sandbox history** (one sandbox per session, each
  tagged with its chat), an append-only **lifecycle ledger** (create / ready / wake / sleep / stop /
  delete / TTL extensions / errors — the audit trail that explains a bill), and **monthly
  usage rows** per sandbox.
- Rows are reconciled hourly while anything is unclosed and on every lifecycle transition.
  After a UTC month ends, one last reconciliation bounded at the month end marks the row
  `closed`; closed rows never change. Explicit deletion captures final usage *before* the
  sandbox is deleted. Reconciliation failures never block lifecycle actions.
- A `CloudIndex` DO lists every Cloud user so an hourly cron can reconcile all of them, and a
  daily scan of Boat's sandbox list reports **orphans** (sandboxes no user owns, usually a
  crashed provision) — those are Zeron's cost, not a user's.
- Users see their month in Settings → Cloud (`GET /cloud/{orgId}/usage`). Operators bill from
  `GET /admin/cloud/usage?month=YYYY-MM[&format=csv]` (`ADMIN_TOKEN`), and each closed month
  is written to R2 at `usage/{YYYY-MM}.json`.
- Pricing (markup, included hours) is a separate business decision; the meter records raw
  seconds and Boat list-price dollars only.

## Credential vault — `zeron-vault` Worker

A separate Worker (`edge/vault/`), with no routes, no `workers_dev`, and a minimal
dependency list (none at runtime; WebCrypto only). The only way in is the `VAULT` service
binding from the main Worker, which passes the **verified** identity. It has its own deploy job and CODEOWNERS entry.

### Durable Objects

- **VaultDevices** `dev1/{userId}` — enrolled device keys `{deviceId, kind: laptop|cloud,
  publicKey, enrolledAt, revokedAt?}`, an append-only audit log, and the per-user disable flag.
- **VaultAccount** `acct1/{userId}/{provider}` — one encrypted credential record, its
  `authorizedDevices`, `generation`, `status` (`connected` · `needs_reconnect`) and refresh
  bookkeeping. **Single-threaded per id, so at most one refresh is ever in flight per
  account.** That is the property that fixes refresh-token races.

### Encryption

Records are AES-256-GCM encrypted under a KEK held as a secret on the vault Worker
(`VAULT_KEK`, 32 random bytes). Worker secrets are stored apart from Durable Object storage, so
a leaked storage dump alone decrypts nothing; a compromise of the Cloudflare account itself
could reach both (the trade-off for not running a separate key service). AAD =
`{userId}|{provider}|{generation}` binds a ciphertext to its slot and generation.

Envelopes carry the id of the key that sealed them (`v2.{kid}.{iv}.{ct}`, `kid` = a SHA-256
prefix of the key). Rotation: set a new `VAULT_KEK` and move the old one to
`VAULT_KEK_PREVIOUS`; records sealed under the previous key are re-sealed under the new one the
next time they are opened. Removing `VAULT_KEK_PREVIOUS` makes any record not yet migrated
unreadable (that user reconnects), which doubles as the emergency response to a leaked key.
The key source is pluggable (`KekProvider`), so an external key manager can replace the
secret later without touching records' format.

### Providers (v1)

| Provider id | Material | Refresh |
| --- | --- | --- |
| `codex` | ChatGPT OAuth token set from `codex login` (`id_token`, `access_token`, `refresh_token`, `account_id`) | `POST https://auth.openai.com/oauth/token` (`grant_type=refresh_token`, Codex public client id) |
| `claude` | Claude Code credential blob from a sign-in through Claude's own OAuth flow (`claudeAiOauth`), or a long-lived `claude setup-token` token | `POST https://platform.claude.com/v1/oauth/token` (`grant_type=refresh_token`, Claude Code client id); setup tokens never refresh |
| `github` | GitHub App user token + refresh token (never granted; see "GitHub") | `POST https://github.com/login/oauth/access_token` with `GITHUB_APP_CLIENT_ID/SECRET` (vault secrets) |
| `anthropic-key` | User's own Anthropic API key | none (static) |
| `openai-key` | User's own OpenAI API key | none (static) |

**Claude subscriptions.** Anthropic's published Claude Code terms restrict third parties from
storing or intermediating Claude.ai credentials "unless we've mutually agreed otherwise".
Zeron holds Claude logins in the vault under its arrangement with Anthropic for running
Claude Code on users' Cloud devices (confirmed by Wing, 2026-10-01; keep the written
confirmation on file next to this doc). Scope it to that purpose: Claude grants are delivered
only to the unmodified Claude Code CLI on the user's own devices, never to another harness or
SDK, and sign-in always completes through Claude's own OAuth flow. Alternatives that need no
vault: the CLI's own `claude auth login` inside the Cloud device, or the user's own Anthropic
API key (`anthropic-key`).

### Sign-in stays on the device

- **Codex**: the laptop runs `codex login` into a throwaway `CODEX_HOME` (the existing account
  flow), reads `auth.json`, uploads it, and deletes the temp home. The laptop's own login is
  untouched, so there is exactly one refresher (the vault) for the uploaded grant.
- **Claude**: the laptop runs the existing Claude sign-in (Claude's own OAuth flow) in a
  capture-only mode that never activates the result locally, and uploads the credential blob.
  Again one refresher.
- **GitHub**: device flow. The laptop asks the edge to start it; the user approves on
  github.com; the *vault* polls GitHub and stores the result, so the token never transits a
  device. Separately, the user installs the App on the accounts / repositories Cloud may reach
  (`githubInstallUrl` in the vault status → `github.com/apps/{slug}/installations/new`).
- **API keys**: pasted on the laptop, uploaded once.

The vault only refreshes. It owns each refresh token from upload on.

### Grants

`POST /vault/{orgId}/grant {provider, deviceId, ts, sig}` (sig by the device key, same scheme
as runner tokens with prefix `zeron-vault-grant`) → `{provider, accessToken, expiresAt,
accountId?, generation}`. A grant requires: the device is enrolled and not revoked, the device
is in the credential's `authorizedDevices`, the user is not disabled, and the global kill switch
(`VAULT_DISABLED=1`) is off. Grants are the provider's own access token, refreshed by the vault
when within 10 minutes of expiry (Codex: when `last_refresh` is older than 7 days or the JWT has
<1 h left). The consumer caches a grant until expiry (offline fallback) and never sees a
refresh token.

Failure semantics: a refresh that returns `invalid_grant` marks the account `needs_reconnect`
and stops issuing grants. An ambiguous failure (timeout/5xx) keeps the current generation and
retries on the next grant; the DO persists a new generation before returning it.

### GitHub

The standard GitHub App model: sessions act as the App, never as the user.

- The **user token** (device flow) stays in the vault. It lists the user's repositories for
  the checkout picker's Cloud option and proves which App installations the user can reach.
- A `github` grant names the session repository (`repo: "owner/name"`, required). The vault
  lists `/user/installations` with the user token, picks the installation on `owner` (not
  suspended), and mints `POST /app/installations/{id}/access_tokens` with an RS256 App JWT
  (`GITHUB_APP_PRIVATE_KEY`, `iss` = client id). The one-hour token reaches every repository
  of that installation — the usual reach of a GitHub App. Minted tokens are cached sealed per
  installation and reused while more than 15 minutes remain; the installation lookup runs on
  every grant, so a user who loses access to an account stops getting its tokens at once.
- Because installations are found through the user's own token, a caller can never name an
  installation it doesn't belong to (the GitHub-documented check; an `installation_id` from a
  setup redirect is never trusted).
- No installation on `owner` → `not_found` with a `github_app_not_installed:` message carrying
  the install link. A 401 on the lookup → `needs_reconnect`. A 401/403 on minting is our App's
  credentials, never the user's: `unavailable`, the connection stays connected.
- Commits keep the sandbox's git author; pushes and pull requests show as the App. The daily
  canary also checks the App credentials (`GET /app`).

### Admin

| Route (WorkOS bearer only) | Effect |
| --- | --- |
| `GET /vault/{orgId}` | Providers `{provider, status, authorizedDevices, updatedAt, account?}` + devices |
| `PUT /vault/{orgId}/credentials/{provider}` | Upload `{material, authorizedDevices}` |
| `PATCH /vault/{orgId}/credentials/{provider}` | Change `authorizedDevices` |
| `DELETE /vault/{orgId}/credentials/{provider}` | Disconnect (wipe record) |
| `POST /vault/{orgId}/devices/enroll` | Enroll the caller's device key (laptops) |
| `POST /vault/{orgId}/devices/{deviceId}/revoke` | Per-device revoke |
| `POST /vault/{orgId}/github/device` | Start GitHub device flow → `{userCode, verificationUri, interval, flowId}` |
| `POST /vault/{orgId}/github/device/{flowId}` | Poll; on success the vault stores the token |

Kill switches: per device (revoke), per user (`POST /vault/{orgId}/disable`), and global
(`VAULT_DISABLED` var; or drop a leaked key via rotation).

### Daily canary

A cron trigger (`0 6 * * *`) refreshes one canary credential per refreshable provider (codex,
claude, github — the connections of the dedicated Zeron account named by `CANARY_USER_ID`) and
checks the GitHub App's own credentials, records `{provider, ok, status, at}` in a `canary` table and logs a
structured `vault.canary` line (Workers observability alert on `ok=false`). A provider whose
refresh contract changes is caught within a day, before users' grants start failing.

## Engine changes

- **Runner mode** (`ZERON_RUNNER_ENROLL` or an existing `{data_dir}/runner.json`): `Auth`
  gains a runner token source (enroll once, then signed `/runner/token`); scope is `Synced`
  for the runner's `{orgId, userId}`. Platform comes from `ZERON_DEVICE_PLATFORM` (`cloud`).
  The runner posts heartbeats and restricts its harness catalog to Codex + Claude Code.
- **Session machines** (`ZERON_CLOUD_*`, persisted to `cloud-session.json`): the device row
  carries `cloud-session`; at boot the engine holds command execution, clones the project into
  `ZERON_CLOUD_PATH` (with the GitHub grant's credential store; retried with backoff) and checks
  out `zeron/cloud-{12 alphanumerics of the chat id}` — one branch per session, so neighbours on
  one repository never push the same branch — then releases execution and drains whatever was
  queued meanwhile. A wake or restart keeps the existing checkout. A Cloud machine writes no
  viewport rows (sidebar pins) and no legacy diff sidecar. What the boot did ("Started a
  machine" / "Woke the machine", "Cloned owner/repo", "Checked out zeron/cloud-…") opens the
  first answer after it, as resolved tool chips (`ToolCall::Unknown` tagged `cloudSetup`; the
  start/wake chip says how long the message waited), grouped as "Set up the Cloud machine" /
  "Woke the Cloud machine". Before the machine can write anything, the sender's trailer reads
  "Starting a Cloud machine" / "Waking the Cloud machine", and the send is not "Not
  delivered" while the session is provisioning, starting or asleep.
- **No keychain on Cloud**: a Cloud device never calls macOS `security` (Providers-page
  account code), and git's helper list for github.com starts with an empty entry, so no system
  helper (Apple git's `osxkeychain`, libsecret) also stores the App token.
- **No login, no run**: a Cloud run of Codex or Claude Code with no credential at all (no
  vault grant, no API key, no login of the sandbox's own) fails at once with where to sign in,
  instead of the CLI retrying unauthenticated.
- **Credential broker** (runner only): before a harness spawn, the host resolves credentials:
  Codex from a vault grant (managed `CODEX_HOME` with the access token, empty refresh token;
  re-granted before expiry), Claude from a vault grant (Claude Code's credential file with the
  access token and an empty refresh token, rewritten before expiry), else the native sandbox
  login, else `anthropic-key`. GitHub grants (App installation tokens for the session
  repository's owner, requested with `repo`) become `GH_TOKEN`/`GITHUB_TOKEN` at spawn and
  are kept fresh in a git credential store file (`{data_dir}/cloud/git-credentials`, 0600,
  wired as git's `credential.https://github.com.helper`) and `gh`'s `hosts.yml`, so `git`
  keeps working in sessions that outlive the spawn-time `GH_TOKEN`. Secrets ride a
  `#[serde(skip)]` field on `RunRequest`, so they are never persisted in the session doc, the
  journal or logs. On platform `cloud` only Codex and Claude Code are registered: nothing else
  is probed, update-checked or installable (some providers' images install a harness the first
  time its binary is invoked).
- **Projects' GitHub repositories**: SpacesSync stamps `githubRepo` (the `origin` remote's
  `owner/name`) on the spaces this device owns, with `gitDetected`. `ListGithubRepos` lists the
  repositories Cloud reaches through the edge (the vault calls GitHub with the user token:
  installed repositories only; a session machine lists its installation token's
  repositories); the UI keeps that list and offers Cloud for a project whose `githubRepo` is in
  it. `CloneRepo` on a session machine is credentialed and idempotent for an existing clone of
  the same remote.
- **Chats run on Cloud**: `createChat {cloud: true}` asks the edge for a session (`POST
  /cloud/{org}/sessions {chatId, spaceId, repo {fullName, cloneUrl, defaultBranch}, branch?}`,
  idempotent per chat; `branch` = the base branch picked in the composer, else the repository's
  default) and writes the chat with the returned session device as host and its checkout as
  cwd (the reply carries both). It refuses a project without a GitHub origin or whose
  repository Cloud doesn't reach. The edge claims the session device's room for the user when it
  mints the device, so the first send's nudge queues while the machine boots instead of
  404ing. `ListCloudBranches {repo, defaultBranch?}` lists the branches a session can start from
  (GitHub's, default first; `GET /vault/{org}/github/branches?repo=`). A side chat
  (`parentChatId`) whose parent runs on a session machine is hosted on that same machine. `ListHarnesses` / `ListModels` aimed at any Cloud
  device are answered by the asking engine (Codex + Claude Code), and anything else aimed at the
  logical device fails fast — it has no engine.
- **Laptop side**: `CloudStatus` / `CloudEnable` / `CloudDelete` / `CloudUsage`,
  `CloudSessions` / `CloudSessionWake` / `CloudSessionSleep` / `CloudSessionDelete`,
  `ListCloudBranches`, `VaultStatus`, `VaultConnectCodex` / `VaultConnectClaude`, `VaultPutApiKey`,
  `VaultAuthorize`, `VaultDisconnect`, `VaultRevokeDevice`, `GithubConnectStart` /
  `GithubConnectPoll` — RPCs that call the edge with the user's bearer and are never routed by
  `targetDeviceId`. The two sign-ins reply `AgentLoginStart` immediately and are driven with
  the existing `PollAgentLogin` / `CompleteAgentLogin` / `CancelAgentLogin`.

## HTTP contract (main Worker)

JSON bodies; errors are `{error: <code>, message}` with the HTTP status. `{orgId}` must equal
the bearer's `org_id`.

| Route | Bearer | Body → reply |
| --- | --- | --- |
| `GET /cloud/{orgId}` | user | → `CloudStatus` (account) |
| `POST /cloud/{orgId}/enable` | user | `{}` → `CloudStatus` (instant; mints the logical device id) |
| `DELETE /cloud/{orgId}` | user | → `CloudStatus` (`deleting` → `off`; deletes every session sandbox after capturing usage) |
| `GET /cloud/{orgId}/sessions` | user | → `{sessions: CloudSession[]}` |
| `POST /cloud/{orgId}/sessions` | user | `{chatId, spaceId, repo {fullName, cloneUrl, defaultBranch}, branch?}` → `CloudSession` (idempotent per chat; starts provisioning) |
| `POST /cloud/{orgId}/sessions/{chatId}/wake` · `/sleep` | user | → `CloudSession` |
| `DELETE /cloud/{orgId}/sessions/{chatId}` | user | → `CloudSession` (`deleting` → removed) |
| `GET /vault/{orgId}/github/repos?q=` | user | → `{repos: GithubRepo[]}` (the vault calls GitHub) |
| `POST /runner/enroll` | none | `{orgId, userId, deviceId, code, publicKey}` → `{ok: true}` |
| `POST /runner/token` | none | `{orgId, userId, deviceId, ts, sig}` → `{accessToken, expiresAt}` |
| `POST /runner/heartbeat` | runner | `{activeRuns, clients}` → `{ok: true}` (per session device) |
| `GET /vault/{orgId}` | user | → `VaultStatusView` |
| `PUT /vault/{orgId}/credentials/{provider}` | user | `PutCredentialRequest` → `VaultStatusView` |
| `PATCH /vault/{orgId}/credentials/{provider}` | user | `{authorizedDevices}` → `VaultStatusView` |
| `DELETE /vault/{orgId}/credentials/{provider}` | user | → `VaultStatusView` |
| `POST /vault/{orgId}/devices/enroll` | user | `EnrollDeviceRequest` → `VaultStatusView` |
| `POST /vault/{orgId}/devices/{deviceId}/revoke` | user | → `VaultStatusView` |
| `POST /vault/{orgId}/disable` | user | `{disabled}` → `VaultStatusView` |
| `POST /vault/{orgId}/grant` | user or runner | `GrantRequest` → `Grant` |
| `POST /vault/{orgId}/github/device` | user | `{authorizedDevices}` → `GithubDeviceStart` |
| `POST /vault/{orgId}/github/device/{flowId}` | user | → `GithubDevicePoll` |

Types are in `edge/vault/src/api.ts` (TypeScript) and `crates/proto/src/cloud.rs` (Rust).
Signatures are base64url (no padding) Ed25519 over UTF-8:

- runner token: `zeron-runner-token\n{orgId}\n{userId}\n{deviceId}\n{ts}`
- vault grant: `zeron-vault-grant\n{userId}\n{deviceId}\n{provider}\n{ts}`

`ZERON_RUNNER_ENROLL` = `{orgId}.{userId}.{deviceId}.{code}` (`code` is 32 random bytes,
base64url). After enrollment the engine persists `{data_dir}/runner.json`
(`{orgId, userId, deviceId, edgeUrl}`) and `{data_dir}/runner-key` (PKCS#8 Ed25519, 0600);
`runner.json` wins over the env var from then on.

### Metering

| Route | Auth | Reply |
| --- | --- | --- |
| `GET /cloud/{orgId}/usage?month=YYYY-MM` | user (own usage only; runner refused); `month` defaults to the current UTC month | `CloudUsage`: `{month, seconds, dollars, sandboxes: [{provider, sandboxId, sandboxType, seconds, dollars, running, reconciledAt}], closed, available: true}` |
| `GET /admin/cloud/usage?month=YYYY-MM[&format=csv]` | `Authorization: Bearer $ADMIN_TOKEN` (header only; constant-time compare; the route is `404` when the secret is unset) | `{month, generatedAt, closed, totals: {seconds, dollars, users, orphans}, users: [{orgId, userId, deviceId, month, seconds, dollars, closed, errors, sandboxes}], orphans: [{provider, sandboxId, state, firstSeenAt, lastSeenAt}]}`; CSV: `kind,month,orgId,userId,deviceId,provider,sandboxId,sandboxType,seconds,dollars,running,reconciledAt,closed,note`, one line per (user, sandbox) and per orphan |

`seconds` are the provider's billable seconds (machine-size multiplier applied), `dollars` its
list price, `reconciledAt` Unix ms. A month is `closed` once it has ended (plus a one-hour
grace for the provider's meter) and every sandbox that existed in it has a closed row; closed
figures never change and are the billable ones. Each closed month is also written once to R2
`BLOBS` at `usage/{YYYY-MM}.json` (same JSON as the admin export) by the hourly cron
(`17 * * * *`), which also pokes every user's reconcile and, at 00:17 UTC, scans each
provider for orphan sandboxes (logged as `{"event":"cloud.orphan", provider, sandboxId, state}`).

## Prerequisites before real users store tokens

1. **Localhost IPC (P1, local-process vector).** The engine's IPC port accepted any local
   process. It now requires a per-install bearer (`{data_dir}/ipc-token`, 0600, rotated on each
   engine start) on the WebSocket handshake. Viewports, the CLI and `zeron mcp` read it at
   dial time; the engine passes the file's path (`ZERON_IPC_TOKEN_FILE`, never the token —
   Claude gets MCP env on its argv) to the MCP servers it injects. See `docs/mcp.md`.
2. **Claude injected-token expiry probe.** The Cloud engine writes vault-granted Claude access
   tokens (never refresh tokens) into Claude Code's credential file and rewrites it before
   expiry. Confirm with `scripts/probe-claude-injected-token.sh` that a long-running Claude
   Code session picks up the rewritten file after its injected token expires (and the
   `CLAUDE_CODE_OAUTH_TOKEN` variant), before any real user's login is stored.
3. Vault infrastructure secrets set (`VAULT_KEK`, GitHub App), canary
   credentials stored, and the vault deploy job approved.

## Operations

| Secret / var | Where | Purpose |
| --- | --- | --- |
| `BOAT_API_KEY` | main Worker | Boat API. Scoped key with `sandbox.create`, `.read`, `.update`, `.stop`, `.resume`, `.delete` and `exec` |
| `SANDBOX_PROVIDER` | main Worker var | provider for new sandboxes (`boat`; `fake` for local dev) |
| `CLOUD_TTL_SECONDS` | main Worker var | unset on a paid plan (no auto-stop); `7200` on a Boat trial |
| `CLOUD_IDLE_MINUTES` | main Worker var | idle sleep threshold (default 20) |
| `RUNNER_JWT_PRIVATE_KEY` / `RUNNER_JWT_PUBLIC_KEY` | main Worker | ES256 runner tokens (JWK JSON; add a `kid` to rotate) |
| `ADMIN_TOKEN` | main Worker | operator usage export (`/admin/cloud/usage`); route 404s when unset |
| `VAULT_KEK` / `VAULT_KEK_PREVIOUS` | vault | record-encryption key (base64, 32 bytes) and the one being rotated out; keep an offline backup — secrets can't be read back |
| `GITHUB_APP_CLIENT_ID` / `GITHUB_APP_CLIENT_SECRET` | vault | GitHub App user tokens (device flow enabled on the app); the client id is also the App JWT's `iss` |
| `GITHUB_APP_PRIVATE_KEY` | vault | the App's private key (PEM as downloaded): signs App JWTs that mint installation tokens |
| `GITHUB_APP_SLUG` | vault | `github.com/apps/{slug}` — the install link in Settings and the project picker |
| `CANARY_USER_ID` | vault | the dedicated account whose connections the daily canary refreshes |
| `VAULT_DISABLED` | vault | `1` = global kill switch |

Deploy order: the vault Worker before the main Worker (service binding); `deploy.yml` gates the
vault job on the `vault-production` GitHub environment (required reviewers), and the Cloudflare
API token also needs Workflows edit permission.
