# Production rollout: web.zeron.sh

## PR scope and deployment status

This PR prepares production hosting and CI/CD. It does **not** establish that
`web.zeron.sh` has been deployed or tested: production account access is required.
The user verified the staging browser login → native CLI/headless → browser device
flow on **two accounts and two machines** at `zeron-test.embedez.com`.
That is evidence for the application flow, not production DNS, credentials or state.

## Reuse the existing production backend

`edge/wrangler.jsonc` keeps:

- Worker `comet-native-edge` and account `b57dde68ee5964dccc025437b5478a5b`;
- existing native, install and release routes, including `edge.zeron.sh`;
- the original Durable Object bindings and migration history (`v1` through `v4`);
- R2 buckets `comet-native-blobs` and `comet-native-releases`;
- the production WorkOS client ID and compatibility date.

Relative to upstream, it adds `web.zeron.sh`, preview origin/wildcard routes,
`BROWSER_SESSIONS`, and migration `v5` creating `BrowserSessionStore`, together
with the built `apps/web/dist` assets on this same Worker. Before deploying,
verify the actual remote migration history; do not assume `v5` is already applied
or alter an existing migration tag. Existing device/session namespaces and R2
buckets must remain intact.
`src/production.ts` is a small entry-point wrapper that re-exports the exact existing
DO classes. It serves only `/`, `/index.html` and the hashed Trunk JS/WASM files,
only at `WORKOS_BROWSER_ORIGIN`, for GET/HEAD without an upgrade header. All other
requests go to the existing edge handler. `assets.run_worker_first=true` is
necessary so Cloudflare does not bypass these checks on preview/native hostnames.
If the build later emits other public asset paths, extend this explicit static
route policy and its regression checks together.

This does not create a second set of production devices. Existing native clients
continue using `edge.zeron.sh`; browser requests at `web.zeron.sh` reach the same
Worker and DO namespaces. Do not point production clients at the staging backend,
rename the production Worker, copy staging's fresh migration, or replace buckets.
Existing device registration behavior is unchanged; an old native client may need
to reconnect/update if it predates browser device registration.

## Maintainer prerequisites — before merging into an auto-deploying main

1. Verify access to the production account and `zeron.sh` zone; check that
   `web.zeron.sh` is available for a Worker custom domain. Review existing dashboard
   variables/secrets against source: Wrangler deploy reconciles configuration.
2. Verify the existing production WorkOS app and its issuer/JWKS URLs in
   `edge/wrangler.jsonc`. Do not substitute the staging WorkOS client or key.
3. In that WorkOS app, register:
   `https://web.zeron.sh/api/browser/callback`.
   Keep existing native/CLI callbacks, including the callback used at
   `https://edge.zeron.sh/auth/cli/callback`. Native clients need no hostname change;
   a new `web.zeron.sh/auth/cli/callback` registration is only needed if deliberately
   configuring a CLI to use the web hostname instead of the normal edge hostname.
4. Verify remote `WORKOS_API_KEY` on `comet-native-edge`. Provision a production
   `BROWSER_SESSION_KEY` if absent (32 random bytes encoded base64url). Retain any
   valid existing key; changing it invalidates stored encrypted browser credentials.
   Do not copy staging's key, commit keys or put keys in CLI arguments. Secret writes
   are deployment operations and should be scheduled accordingly.
5. Configure the repository `CLOUDFLARE_API_TOKEN` for the production account with
   Workers Scripts/Routes and required R2 permissions. Never put it into the app.
6. Merge the runtime dependency PR first and follow `apps/web/README.md` to repin
   to the published upstream runtime revision and regenerate both Cargo lockfiles.
   Run the locked build again after changing the pin.

No single-user allowlist is required. WorkOS authenticates users and the backend
scopes sessions/devices to each verified user. Signup/invitation policy belongs
in the production WorkOS app.

## Build, CI and release behavior

The existing `.github/workflows/deploy.yml` is extended, rather than adding a
parallel deployment system. Changes in edge, web, shared crates/vendor, root Cargo
inputs and relevant workflow configuration trigger the combined edge/web build.
It retains manual dispatch and serialized production deployments.

Before deployment it installs the pinned nightly toolchain and Trunk, builds with
`--release --locked`, verifies the generated entry point, `_headers`, WASM and
25 MiB asset limit, and runs edge typecheck/tests and local production routing
checks. On successful deployment it runs the web smoke check. Missing Cloudflare
credentials retain the workflow's existing skip-deploy behavior; a green build
without credentials is not a deployment.

`.github/workflows/web-validation.yml` builds/packages the production bundle on
PRs without Cloudflare credentials, checks routing locally, and includes the
multiuser regression. PR checks do not provision resources or register domains.

Manual commands, from the repository root (after the toolchain setup in the web
README):

```sh
(cd apps/web && env -u NO_COLOR RUSTUP_TOOLCHAIN=nightly-2026-09-08 \
  CARGO_BUILD_JOBS=2 trunk build --release --locked)
cd edge
npm ci
npm run typecheck
npm test
npx wrangler deploy --dry-run --config wrangler.jsonc --outdir dist/production
node scripts/production-routing.mjs
# Only an authorized production operator, after the checklist above:
npx wrangler deploy --config wrangler.jsonc
node scripts/staging-smoke.mjs https://web.zeron.sh
```

The smoke script accepts an explicit origin despite its historical staging name.
It verifies public assets/headers and unauthenticated API responses, not completion
of WorkOS login. Keep `AUTH_MODE=workos`; missing browser secrets must fail closed.

## Post-deployment acceptance and recovery

- Verify TLS, root/JS/WASM responses and `crossOriginIsolated === true`.
- Complete WorkOS login/logout on `web.zeron.sh` with two users.
- Confirm each user's existing production device appears and RPC works; verify
  one account cannot access the other's sessions/devices.
- Verify native `edge.zeron.sh` traffic, install/releases and preview routes still
  behave as before. Web assets must not be exposed on preview hostnames.
- Record the deployed version and operator verification in the PR/release notes.

Keep the previously deployed Worker version available. A failing post-deploy check
reports a failed workflow but does not automatically undo the deployment. An
operator should inspect the failure and use Cloudflare's supported version rollback
if needed. A code rollback does not roll back DO/R2 data, secrets, DNS changes or
WorkOS callback settings; do not delete namespaces/buckets as a rollback shortcut.
This rollout retains the compatibility date and existing storage identities,
but adds the `v5` browser-session namespace migration. Review Cloudflare's
migration rollback restrictions before deploying; rolling back code does not
remove the new namespace or reverse stored data.

## Local verification for this change

- Locked release WASM build, TypeScript typecheck and production Wrangler dry-run passed.
- Earlier workerd runs reported passing while emitting `Expected global Vitest
  state` assertions. Investigation traced these to pre-initialization HTTP probes
  reaching the Vitest pool entrypoint. `edge/scripts/patch-vitest-pool.mjs` applies
  a temporary, layout-checked startup guard during `npm ci`; dependency versions
  are unchanged. `npm run test:workerd` also fails on uncaught runner diagnostics
  even when Vitest reports exit 0. Repeated local runs passed 44 unit + 24 workerd
  tests without those diagnostics. Revalidate hosted CI; remove the patch when
  an upstream release fixes this startup behavior.
- 39 real local Worker routing checks passed, including static response bytes,
  isolation headers, native/preview host exclusion and backend route preservation.
- Workflow actionlint and production storage/configuration preservation checks passed.
- No production deployment or live `web.zeron.sh` verification was performed.