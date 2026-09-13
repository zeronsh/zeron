# Web deployment: session summary

> Historical deployment record, not the current asset/pin inventory. Later staging
> updates used ignored snapshots with local zui overrides; their success alone
> did not validate the committed runtime pin. The web manifest now pins published
> `Gratenes/zui@aca6b04288396d26a0b080ba9a21a50446443834` for reproducible review
> builds. See [the web README](../apps/web/README.md) for runtime-first landing and
> [the production checklist](web-production-rollout.md) for prerequisites,
> including the new `v5` browser-session migration. Production remains undeployed.


## What is deployed?

The existing Rust/GPUI web application and edge backend are deployed together as
one **Cloudflare Worker with Static Assets**, not Cloudflare Pages or a new
JavaScript frontend.

- URL: **https://zeron-test.embedez.com**
- Purpose: isolated staging to verify deployment; `web.zeron.sh` was not deployed
  because the user does not have access to that domain.
- Worker: `zeron-web-staging`
- Account: `f3130440b1b0daacd1b0426a3a7f6bb5`
- Latest version deployed in this chat: `b3736dcf-8046-4b18-8a89-6efcce3ea975`
- Configuration: `edge/wrangler.staging.jsonc`
- Release assets: `apps/web/dist/`

```text
Browser -> https://zeron-test.embedez.com
           |-- /                       Rust/GPUI WASM web UI
           |-- /api/browser/*          existing browser session/device backend
           |-- /auth/*                 existing CLI/auth endpoints
           `-- Durable Objects + R2    independent staging state

Browser/CLI login -> WorkOS -> registered staging callback
Native zeron headless -> same staging Worker -> device relay -> browser
```

There is no SPA fallback. API routes must return API responses, not the UI HTML.
Authentication stays enabled; this is not a public dev-auth deployment.

## Product/configuration changes made in this chat

Paths below are relative to `gratenes/zeron/`.

| File | Change |
| --- | --- |
| `edge/wrangler.staging.jsonc` | New independent Worker, custom domain, static assets, six Durable Object bindings/migration, two R2 buckets, WorkOS settings and observability. |
| `apps/web/Cargo.toml` | Added `[profile.release] strip = "symbols"`. This reduced the generated WASM from about 29.6 MiB to 23.63 MiB (24,778,300 bytes), below Cloudflare's 25 MiB per-asset limit. |
| `apps/web/_headers` | Added COOP `same-origin`, COEP `require-corp`, CORP `same-origin` and `nosniff` for static responses. Shared-memory WASM requires cross-origin isolation. |
| `apps/web/index.html` | Added a Trunk `copy-file` link so `_headers` is included in the deployment bundle. |
| `edge/src/browser-routes.ts` | Removed the requirement for `WORKOS_BROWSER_OWNER_SUBJECT` and the callback comparison against that one user. Kept verified WorkOS identity, PKCE, cookie security and per-user ownership. |
| `edge/src/env.ts` | Removed the obsolete single-owner environment field. |
| `edge/test/workerd/browser-multi-user.workerd.test.ts` | Added two-user authentication/isolation regression coverage, including sessions, revocation, devices, WebSockets, callback replay and mismatched identities. |
| `edge/scripts/staging-smoke.mjs` | Added repeatable read-only deployment checks for assets, MIME types, isolation headers, backend health and unauthenticated API routing. |
| `apps/web/README.md` | Updated authentication documentation for multiple users. Most other README changes already existed before this chat. |
| `docs/web-staging-deployment.md` | Added the operational deployment guide. |
| `docs/web-deployment-session-summary.md` | This summary. |

The runtime Git pins, web lockfile changes, `apps/web/trunk.toml` and
`.github/workflows/web-validation.yml` were already present when this chat began;
they are not new deployment work from this chat. The deployed web build uses the
existing published `Gratenes/zui` pin, not arbitrary uncommitted sibling runtime
changes.

No commits, pushes or PR merges were performed. Changes remain in the working
tree. No product edits were made in `original/`, `wasim/` or `zui/`.

## Cloud resources and authentication

Created isolated R2 buckets:

- `zeron-web-staging-blobs`
- `zeron-web-staging-releases`

The new Worker deployment created independent namespaces for `SessionRoom`,
`DeviceRoom`, `BrowserSessionStore`, `PreviewRoom`, `RegistryRoom` and `ChatRoom`.
Existing production state and routes were not reused or changed.

The existing WorkOS credentials were found in
`target/workos-staging/edge.env`. The API key was verified and uploaded as the
Worker secret `WORKOS_API_KEY`; a new independent `BROWSER_SESSION_KEY` was
created and uploaded. Secret values were not put into source or chat. The
permission-restricted deployment secrets file is under the ignored
`target/workos-staging/` directory. Do not commit or share these files, and do
not rotate the session key on every redeploy.

The user registered both WorkOS redirect URIs:

- `https://zeron-test.embedez.com/api/browser/callback` — browser PKCE/cookie login
- `https://zeron-test.embedez.com/auth/cli/callback` — terminal paste-code login

Multiple users authenticated by this WorkOS application can now sign in. Each
user can access only their own sessions/devices. WorkOS signup/invitation
policies still apply; removing the single-owner gate does not bypass WorkOS.

## How to redeploy

This chat deployed manually with the project's installed Wrangler 4.119.0.
**No automatic staging deployment workflow was added.** The existing production
workflow is not the staging deploy command.

From `gratenes/zeron/`, rebuild if the web UI changed:

```sh
(cd apps/web && env -u NO_COLOR RUSTUP_TOOLCHAIN=nightly-2026-09-08 \
  CARGO_BUILD_JOBS=2 trunk build --release --locked)
```

Then deploy explicitly to staging (existing remote secrets are retained):

```sh
cd edge
npm run typecheck
npx wrangler deploy --dry-run --config wrangler.staging.jsonc --outdir dist/staging
npx wrangler deploy --config wrangler.staging.jsonc
node scripts/staging-smoke.mjs https://zeron-test.embedez.com
```

Always pass the staging config. See [the deployment guide](web-staging-deployment.md)
for initial resource provisioning and secure first-deploy secret setup.

## Connecting a test CLI/device

Use an interactive terminal and keep these settings for both login and headless:

```sh
export ZERON_EDGE_URL="https://zeron-test.embedez.com"
export ZERON_WORKOS_CLIENT_ID="client_01M270WMSY2KHVFJFZJMG7YTDF"
export ZERON_DATA_DIR="$HOME/.zeron-staging"
export ZERON_IPC_PORT=27650
zeron login
# Open the printed URL, authenticate, paste the callback code into this terminal,
# and complete workspace selection if prompted.
zeron status
zeron headless
```

The separate data directory avoids overwriting production login state. Leave the
headless engine running for the web app to connect to this device. No WorkOS API
key belongs in the CLI.

## What was verified, and what was not

Passed:

- Locked release Trunk build and Cloudflare asset-size check.
- TypeScript typecheck, 6 focused unit tests and 11 workerd tests (17 total).
- The new two-user test uses mocked WorkOS responses and signed test JWTs, with
  real local Durable Object execution; it is not two real production logins.
- Staging dry-run, actual deployment, HTTPS/static assets, correct MIME types,
  isolation headers, backend health and API routing.
- Live login initiation: PKCE, secure HttpOnly transaction cookie, rejection of
  an unrelated Origin, and redirect to the configured WorkOS AuthKit host.
- CLI callback is reachable; without code/state it correctly returns an error.
- Playwright loaded the deployed WASM: application-start event fired, a canvas
  was present and `crossOriginIsolated` was true, with no reported page error.
  Screenshot captured at `target/workos-staging/deployed-ui.png`; this was a
  loading check, not a comprehensive visual/interaction audit.

User-confirmed follow-up: the live browser login → signed-in CLI/headless →
browser/device flow worked on **two different accounts and machines**. This closes
that staging end-to-end gap; it is user-reported verification, not an automated
production-domain test. Logout/revocation and the broader device-switching, mobile,
attachment and reconnect matrix still need explicit acceptance checks.

Remote project previews are deliberately not enabled: `BROWSER_PREVIEW_ORIGIN`
requires a separate origin for untrusted content. `web.zeron.sh` deployment and
staging CI/CD automation remain future work.

## Follow-up: production rollout prepared for the PR

The subsequent rollout work adds `web.zeron.sh` to the existing production Worker
config, with host-restricted static serving, production browser-auth settings and
unchanged storage/migrations. It extends the existing deployment and PR validation
workflows to build/package the WASM, test routing and multiuser isolation, and
smoke-check after deployment. The web validation file existed earlier, but these
rollout extensions are new work. No production deployment was performed.
See [web-production-rollout.md](web-production-rollout.md) for prerequisites,
commands, acceptance checks and recovery notes.


## Agent tooling setup (not part of the deployed application)

Installed Cloudflare skills in `~/.agents/skills/` and registered five Cloudflare
MCP servers in `~/.mimir/agent/mcp.json`, preserving the existing server entry.
The public docs server connected; authenticated MCP servers failed initialization.
Deployment therefore used the working local Wrangler OAuth login, not MCP.
