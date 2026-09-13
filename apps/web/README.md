# Zeron web client

The browser client renders the shared `zeron_ui` application and automatically selects one online account device for its initial transport through the edge browser-session and DeviceRoom APIs. The shared app still routes device-targeted work to other account devices. It does not start an engine, use local IPC, or connect to a loopback gateway.

## Local rendering versus full integration

`trunk serve` verifies that the WASM UI renders locally. It does not start the edge worker, authenticate a user, enumerate devices, or connect to a remote engine. Full integration requires the configured edge worker and an online device; a successful local render is not evidence of that integration.

## Toolchain

The web crate uses nightly because [`apps/web/.cargo/config.toml`](.cargo/config.toml) enables `build-std` and WASM shared-memory linker flags. The current checkpoint uses `nightly-2026-09-08`:

```sh
rustup toolchain install nightly-2026-09-08 --profile minimal
rustup component add rust-src --toolchain nightly-2026-09-08
rustup target add wasm32-unknown-unknown --toolchain nightly-2026-09-08
cargo install trunk --locked --version 0.21.14  # if needed
```

## Build and serve

The following block starts at the repository root, enters `apps/web` once, and runs either the build or local renderer. Validation commands below are root-relative.

```sh
cd apps/web

# Reproducible release bundle:
env -u NO_COLOR RUSTUP_TOOLCHAIN=nightly-2026-09-08 CARGO_BUILD_JOBS=2 \
  trunk build --release --locked

# Or, instead of the build above, render locally with reloads:
env -u NO_COLOR RUSTUP_TOOLCHAIN=nightly-2026-09-08 \
  trunk serve --port 8080
```

`index.html` selects the `comet-web` binary. The bundle is written to `apps/web/dist/` (ignored by git); `--locked` rejects unreviewed dependency or lockfile changes. The manifest pins the published [`Gratenes/zui@aca6b04288396d26a0b080ba9a21a50446443834`](https://github.com/Gratenes/zui/commit/aca6b04288396d26a0b080ba9a21a50446443834), including bundled color emoji, core touch-focus tracking and browser keyboard/input bridging. All web runtime packages resolve from Git, not the sibling checkout or an ignored staging snapshot.

[`trunk.toml`](trunk.toml) binds development serving to `127.0.0.1:8080` and sets the GPUI/WASM headers:

```text
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

A real host must return those headers for the document, scripts, fonts, and WASM, use HTTPS for secure cookies, and route the same-origin browser API paths (`/api/browser/session`, `/api/browser/devices`, auth routes, and `/api/browser/device/:deviceId/ws`) to the edge worker. COEP requires assets to be same-origin or explicitly compatible with the deployment's CORP/CORS policy.

### Static HTTP preview

`python3 -m http.server` can inspect generated `dist/` files, but it provides neither the required headers, edge APIs, authenticated device socket, nor Trunk's reload socket. It is not an application smoke test. `trunk serve --no-autoreload` retains the required headers but still provides only local bundle serving, not edge authentication or a remote device.

### Remote project preview

Remote project preview is separate from Trunk serving the web bundle. `edge/src/browser-routes.ts` sends each request through an authenticated DeviceRoom preview frame, and `crates/preview/src/remote.rs` forwards an ordinary HTTP request/response to the selected local service. This seam is HTTP-only: WebSocket upgrades, persistent sockets, and Vite HMR are not supported. An initial page response may work while a project's WebSocket/HMR features do not.

## Edge configuration for authenticated integration

The public browser BFF is fail-closed unless the worker has these non-secret values: `AUTH_MODE=workos`, `WORKOS_CLIENT_ID`, exact `WORKOS_BROWSER_ORIGIN`, `WORKOS_ISSUER`, and `WORKOS_JWKS_URL`. Register `<WORKOS_BROWSER_ORIGIN>/api/browser/callback` as the WorkOS redirect URI. Multiple users can authenticate through this WorkOS application; each user's sessions and devices remain scoped to their verified WorkOS `sub`. WorkOS signup/invitation policies still apply.

Set these as Wrangler secrets, never in this README or browser code: `WORKOS_API_KEY` (WorkOS exchange/revocation) and `BROWSER_SESSION_KEY` (browser session credential-encryption key). Public browser routes also need the `BROWSER_SESSIONS` and `DEVICE_ROOMS` Durable Object bindings. For remote project preview, configure `BROWSER_PREVIEW_ORIGIN` as a dedicated HTTPS origin (the current config uses `https://preview.edge.zeron.sh`) with wildcard routing for ticketed preview hosts, and provision `PREVIEW_ROOMS`.

For loopback worker tests only, `AUTH_MODE=dev` requires a loopback `BROWSER_DEV_ORIGIN`, `BROWSER_DEV_OWNER_SUBJECT`, and `BROWSER_SESSION_KEY`; optional `BROWSER_DEV_ORGANIZATION_ID` and trusted-proxy `BROWSER_DEV_PROXY_KEY` are also supported. Never use dev mode in a deployed integration. The separate private `/auth/browser/*` broker routes additionally use `BROWSER_BROKER_TOKEN`; the public web BFF does not send that token.

## Production deployment

See [the production rollout checklist](../../docs/web-production-rollout.md) for
`web.zeron.sh`, WorkOS prerequisites and the existing edge deployment workflow.
The production Worker keeps its existing device/session storage; PR validation
builds and packages it without deploying. [Staging](../../docs/web-staging-deployment.md)
remains a separate, manually deployed verification target.


## Runtime-first landing and final pin

The web checkpoint uses the published fork revision from [zui PR #8](https://github.com/zeronsh/zui/pull/8), so it can be reviewed and built before that PR lands. The native workspace retains its existing upstream runtime pin until landing.

Before merging the app PR, merge the runtime PR first. Use its actual published upstream merge/squash SHA (not a guessed SHA) for root `Cargo.toml` dependencies `gpui`, `gpui_platform`, `gpui_tokio` and the web direct dependencies. Remove the temporary web fork patches once all sources converge on `zeronsh/zui`. Regenerate and review both `Cargo.lock` and `apps/web/Cargo.lock`, verify Cargo metadata resolves `gpui_web` and `gpui_wgpu` to that Git revision, and rerun locked native/web checks. Never commit local runtime path overrides, snapshots or generated assets.

## Validation commands

From the repository root:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --manifest-path apps/web/tests/lifecycle/Cargo.toml
(cd edge && npm ci)
(cd edge && npm run typecheck)
(cd edge && npm run test:unit -- src/browser-auth.test.ts src/browser-routes.test.ts src/device-frame.test.ts)
(cd edge && npm run test:workerd -- test/workerd/browser-discovery.workerd.test.ts test/workerd/browser-sessions.workerd.test.ts test/workerd/device-browser.workerd.test.ts)
```

The web CI workflow runs the locked Trunk WASM build, lifecycle tests, and the same focused edge checks. Existing native CI remains responsible for native application and UI coverage.

## Smoke, review, and merge checklist

- [ ] `trunk build --release --locked` succeeds with the stated nightly and pin.
- [ ] The served origin reports `crossOriginIsolated === true`; COOP/COEP are present on document and static responses.
- [ ] Full edge smoke passes: sign in, list only the account's devices, switch sessions and devices, complete the RPC handshake, then log out.
- [ ] Revoke a session/device; verify its old cookie, socket, and request cannot be replayed, while another active session is unaffected.
- [ ] Upload and read back 200 KiB and 1 MiB attachments without disconnects or duplicate submission. The attachment limit is 24 MiB; the encoded relay-frame ceiling is 1 MiB, so larger attachments use multiple frames.
- [x] Phone taps, scrolling, keyboard behavior, and popovers: manually verified by the user. Device/browser versions and tested revision were not recorded.
- [x] Terminal input/focus, mobile drawers, emoji and reconnect interactions: user-verified passing on staging on 2026-09-13 with the local-runtime bundle. This is user acceptance, not automated evidence for a later merge build.
- [ ] Run the lifecycle and focused edge auth/device tests above.
- [ ] Review manifest/lockfile sources; after runtime landing, repin to the exact published `zeronsh/zui` commit.
- [ ] Keep native CI and unrelated upload/lifecycle implementation changes in their owning changesets.
