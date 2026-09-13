# Web staging deployment

This is an isolated verification target, not production:

- Account: `f3130440b1b0daacd1b0426a3a7f6bb5`
- Worker: `zeron-web-staging`
- Custom domain: `zeron-test.embedez.com`
- Config: `edge/wrangler.staging.jsonc` (always pass `--config`)
- R2 buckets: `zeron-web-staging-blobs`, `zeron-web-staging-releases`

The existing edge Worker serves the same-origin browser BFF alongside static
`apps/web/dist` assets. Cookie auth and WebSockets stay on the app origin; no
production edge endpoint or cross-origin token bridge is needed. Durable Object
bindings belong to this new Worker, with independent storage. Never reuse
production bucket names or deploy using `edge/wrangler.jsonc` for this test.
`workers.dev` and version preview URLs are disabled.

## Auth prerequisites

Keep `AUTH_MODE=workos`. The nonsecret settings are populated for the verified
staging WorkOS application. When recreating this environment,
verify these settings against the intended WorkOS application:

- `WORKOS_CLIENT_ID`
- `WORKOS_ISSUER` and `WORKOS_JWKS_URL`: the application's actual JWT trust anchor
- `WORKOS_BROWSER_ORIGIN` is already `https://zeron-test.embedez.com`

Any user authenticated by this WorkOS application can sign in. Sessions and devices
remain scoped to the verified user ID; signing in does not grant access to another
user's devices. WorkOS signup/invitation policies still apply.

Provision `WORKOS_API_KEY` and a fresh `BROWSER_SESSION_KEY` as Worker secrets,
never in source control or command arguments. Use the existing browser-session
key format supported by `edge/src/browser-sessions.ts`. Do not copy production
session encryption keys. Missing browser auth inputs return JSON 501; the static
shell may load but must not create an authenticated session. Do not turn on dev
auth to make a public staging deployment appear healthy.

Register these exact redirect URLs in that same WorkOS application:

- `https://zeron-test.embedez.com/api/browser/callback` — browser PKCE/cookie login
- `https://zeron-test.embedez.com/auth/cli/callback` — headless CLI paste-code page

The CLI callback does not establish a browser session and does not need another
domain. Any engine/CLI used in the test must also target this staging edge rather
than its default production endpoint.

Untrusted browser previews are intentionally unavailable: there is no
`BROWSER_PREVIEW_ORIGIN`. Enabling them later requires a separately isolated
preview origin and routing; never point that setting at the app/BFF origin.

## Build and local verification

From the repository root, follow the toolchain and web build instructions in
`apps/web/README.md`, using a release Trunk build. Trunk copies `apps/web/_headers`
into `dist` via the `copy-file` link in `index.html`. Confirm that all individual
assets fit Cloudflare's 25 MiB limit; a successful Rust build alone is not enough.

```sh
cd edge
npm ci
npm run typecheck
npm run test:unit -- src/browser-auth.test.ts src/browser-routes.test.ts
npm run test:workerd -- test/workerd/browser-sessions.workerd.test.ts test/workerd/browser-discovery.workerd.test.ts test/workerd/device-browser.workerd.test.ts
npx wrangler types --config wrangler.staging.jsonc dist/staging-env.d.ts
npx wrangler deploy --dry-run --config wrangler.staging.jsonc --outdir dist/staging
npx wrangler dev --local --config wrangler.staging.jsonc --port 27641
# In another terminal, from edge/:
node scripts/staging-smoke.mjs http://127.0.0.1:27641
```

Do not load staging secrets for fail-closed local checks. The smoke
script is read-only and checks HTML, JS/WASM MIME types, isolation headers,
WorkOS mode, and unauthenticated API routing (including navigation requests).
There is deliberately **no SPA fallback**: `/` is the UI entry point and missing
paths reach the Worker. `/api`, `/auth`, their subpaths, and `/health` explicitly
run Worker-first so static files cannot mask those handlers. `_headers` applies
only to assets; it does not change API responses or the preview security model.

## Operator deployment and end-to-end checks

No provisioning or deployment is performed by adding this configuration. An
operator must first verify account access and the `embedez.com` zone, create the
two staging R2 buckets, verify the WorkOS settings, and supply both secrets through
a protected Wrangler secrets workflow. Secret changes may deploy a Worker
version, so treat them as deployment operations too.

For R2 management, explicitly select the account even when passing the staging
config; account-level commands such as bucket listing may not use its account:

```sh
CLOUDFLARE_ACCOUNT_ID=f3130440b1b0daacd1b0426a3a7f6bb5 npx wrangler r2 bucket list --config wrangler.staging.jsonc
```

Use that same account environment variable for any authorized bucket creation.
Wrangler also supports `deploy --secrets-file <protected-file>` to supply secrets
with the initial deployment; keep the file ignored, permission-restricted, and
out of logs. Check the installed CLI's help for the accepted file format.

After those prerequisites and a successful dry run, the explicit deployment
command (from `edge/`) is:

```sh
npx wrangler deploy --config wrangler.staging.jsonc
node scripts/staging-smoke.mjs https://zeron-test.embedez.com
```

Then verify in a real browser: `crossOriginIsolated === true`, the WASM UI renders,
login returns to the browser callback, two users each see only their own devices,
cross-user access is denied, a staging-connected engine completes a WebSocket
round trip, and logout clears access. Dry-run/local tests do not prove DNS/TLS,
remote resources, provider credentials, OAuth redirects, or engine connectivity.

References: [static asset headers](https://developers.cloudflare.com/workers/static-assets/headers/),
[asset routing](https://developers.cloudflare.com/workers/static-assets/routing/),
[Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/).
