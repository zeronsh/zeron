# Plugin store: identity rollout

Operator notes for shipping the edge's identity pieces of the plugin store
(`docs/plans/2026-11-17-plugin-store.md`): web sign-in, staff roles, deploy
keys and signed requests. Not user documentation. Do staging first, then
production, in this order.

Code: `edge/src/web-auth-routes.ts`, `web-session.ts`, `web-cookies.ts`,
`roles.ts`, `store/principal.ts`, `store/identity/*`; tables in
`edge/src/user-directory.ts` (`users1`).

Below, `EDGE` is the edge's own address for the environment (production
`https://edge.zeron.sh`; staging, the edge's address in `env.staging` of
`edge/wrangler.jsonc`), and `SITE` is that environment's `WEB_ORIGIN` (the
web site's origin, also in `edge/wrangler.jsonc`: production
`https://zeron.sh`).

## 1. Before the deploy

### WorkOS redirect address

Each environment's WorkOS application must list its callback as a redirect
URI (dashboard: Applications, Redirects):

| Environment | WorkOS environment | Redirect URI |
|---|---|---|
| Production | production (`WORKOS_CLIENT_ID` in `vars`) | `https://zeron.sh/auth/web/callback` |
| Staging | staging (`WORKOS_CLIENT_ID` in `env.staging.vars`) | `SITE/auth/web/callback`, with staging's `WEB_ORIGIN` |

- WorkOS's API replaces the whole list of redirect URIs at once. If you
  set them through the API, send every existing entry with the new one.
  Read the list first and compare after.
- No sign-out redirect is needed. Signing out ends the WorkOS session
  server-side (`POST /user_management/sessions/revoke`); the browser never
  visits WorkOS's logout page.

### Variables and secrets

| Name | Where | Value |
|---|---|---|
| `WEB_ORIGIN` | `vars` / `env.staging.vars` in `edge/wrangler.jsonc` (already set) | the site's origin, `https://` and no path. Unset: no web sign-in and no cookie is accepted anywhere. |
| `WEB_SIGNIN_PKCE` | not set | Leave unset (PKCE on). `"off"` only as the fallback in step 3b. |
| `WORKOS_API_KEY` | secret, existing | Required: the code exchange and session revocation use it. |
| `ADMIN_TOKEN` | secret, existing | Required for the operator routes below. |

No new secrets.

### Routes

`edge/wrangler.jsonc` adds path routes on the `zeron.sh` zone:
- production: `zeron.sh/auth/web/*`
- staging: the staging site's host followed by `/auth/web/*`

Like `zeron.sh/install.sh`, they take precedence over the landing worker's
custom domain for those paths only. `zeron.sh/store/v1/*` is deliberately
not routed to the edge: the site reaches the store over its service binding.

### Durable Object storage

Nothing to run. On start, `users1` creates `web_sessions`, `user_roles`,
`deploy_tokens` (each token's SHA-256 and short id, never the token) and
`deploy_token_plugins` (a token's listed scope). `plugin_keys` is unchanged:
it holds devices' keys only.

## 2. Deploy

Deploy the edge as usual (`.github/workflows/deploy.yml`, or
`npx wrangler deploy --env staging` for staging), then the site.

## 3. Check after the deploy

Run each check on staging before production.

### a. Routing

```sh
curl -s -o /dev/null -w '%{http_code}\n' "SITE/auth/web/session"
#   401 (JSON {"error":"signed_out"}), from the edge, not the landing page's 404
curl -s -o /dev/null -w '%{http_code} %{redirect_url}\n' "SITE/auth/web/login?return_to=/plugins"
#   302 https://api.workos.com/user_management/authorize?...&code_challenge=...&code_challenge_method=S256&max_age=0
curl -s -o /dev/null -w '%{http_code}\n' "SITE/auth/web/login?return_to=//evil.example"
#   302 as well; after sign-in this lands on SITE/, never elsewhere
```

### b. One real sign-in (in a browser)

1. Open `SITE/auth/web/login?return_to=/plugins`. AuthKit asks for
   credentials even if you're signed in to it elsewhere (`max_age=0`).
2. Sign in. You land on `SITE/plugins`.
3. In the browser's storage view, the cookie `__Host-zeron_session` is
   HttpOnly, Secure, SameSite=Lax, Path `/`, with no Domain.
4. `SITE/auth/web/session` in the same browser answers your `user`,
   `username`, `roles`, `csrf` and `"fresh_sign_in": true`.

If step 2 shows "The sign-in code was refused" on a first, fresh attempt,
and the edge's logs show `auth/web: code exchange failed 400`, WorkOS is
refusing a PKCE verifier sent beside the client secret. Fall back:

```sh
cd edge && npx wrangler deploy --env staging --var WEB_SIGNIN_PKCE:off
```

Or set `"WEB_SIGNIN_PKCE": "off"` in that environment's `vars`. Then:
- Sign in again; the authorize URL then has no `code_challenge`.
- Report it to the plugin store's coordinator.
- Remove the fallback once resolved: the state cookie still stops login
  CSRF, but an intercepted code could be redeemed.

### c. A session read through the site

Open a site page that shows who is signed in (the site asks
`GET /auth/web/session` over its service binding). It shows your name and
username. After step 4 below, it shows the admin role.

### d. A refused cross-site change

With the session cookie from step b (copy its value):

```sh
# A foreign Origin: refused.
curl -s -X POST "SITE/auth/web/logout" -H "origin: https://evil.example" \
  -H "cookie: __Host-zeron_session=$COOKIE" -H "x-csrf-token: $CSRF" -w ' %{http_code}\n'
#   {"error":"bad_csrf","message":"This request didn't come from the Zeron site."} 403
# The right Origin without the session's CSRF token: refused.
curl -s -X POST "SITE/auth/web/logout" -H "origin: SITE" \
  -H "cookie: __Host-zeron_session=$COOKIE" -w ' %{http_code}\n'
#   {"error":"bad_csrf","message":"This page is out of date. Reload it and try again."} 403
```

The browser is still signed in afterwards (re-check
`SITE/auth/web/session`). Through the site, a store change without the
token answers 403 `bad_csrf` the same way.

### e. A refused bearer on an admin route

Once an admin is granted (step 4), take that account's app bearer (an
access token from the app's sign-in) and call an admin route the site
reaches over its service binding. Through the edge's own address:

```sh
curl -s "EDGE/store/v1/admin/queue" -H "authorization: Bearer $ADMIN_ACCOUNTS_ACCESS_TOKEN" -w ' %{http_code}\n'
#   {"error":"forbidden","message":"Admin is by signing in on the web only."} 403
```

The same account's web session opens the admin dashboard; its bearer never
opens an admin route.

### f. Sign out

Use the site's Sign out button (a form post with the `csrf` field). It lands
on `SITE/`, and `SITE/auth/web/session` answers 401 `signed_out`.

## 4. Grant the first admin

The account must have signed in to Zeron at least once, so the directory has
it.

1. Find their user id:
   ```sh
   curl -s "EDGE/admin/users?format=csv" -H "authorization: Bearer $ADMIN_TOKEN" | grep -i '<their email>'
   ```
   The second column is the `user_…` id.
2. Grant the role, naming yourself in `by` (the token is shared, so the
   record says who):
   ```sh
   curl -s -X POST "EDGE/admin/roles/grant" -H "authorization: Bearer $ADMIN_TOKEN" \
     -H "content-type: application/json" \
     -d '{"userId":"user_…","role":"admin","by":"<you>","note":"first reviewer"}'
   #   {"granted":true}
   ```
3. Check it: `curl -s "EDGE/admin/roles" -H "authorization: Bearer $ADMIN_TOKEN"`.
4. They sign in on the web (step 3b). Their `roles` is `["admin"]` and the
   admin dashboard opens.
   - Admin is by web session only: the app's bearer never opens admin routes.
   - Staff sessions end after 30 minutes idle and 12 hours at most.

A WorkOS organization role never makes anyone an admin; only this record
does. To take it back:

```sh
curl -s -X POST "EDGE/admin/roles/revoke" -H "authorization: Bearer $ADMIN_TOKEN" \
  -H "content-type: application/json" -d '{"userId":"user_…","role":"admin","by":"<you>"}'
```

Revoking takes effect on their next request. `GET EDGE/admin/roles?all=1`
lists revoked grants too.

## 5. Deploy tokens (after the store is merged)

- An account makes tokens from the app, the CLI or the site (`POST
  EDGE/store/v1/me/tokens`, contract a315eef5a). A token is a plain bearer
  secret, `zdt_` and 49 characters, shown once. It belongs to the account
  and carries a scope: a list of plugins it owns, or all of them (those it
  owns then and those it claims later, never one transferred in). Making
  one needs a fresh sign-in. A token never publishes a plugin's first
  version: that comes from a device. An account has at most 20 live tokens
  and makes at most 10 a day.
- CI then publishes with `Authorization: Bearer zdt_…`.
- Check, with a test account that owns two plugins, A and B:
  1. Create a token scoped to A (`{"plugins":["A"]}`). `GET
     EDGE/store/v1/me/tokens` lists it with that `scope`, its `short` id and
     `last_used_at: null`, and never the token.
  2. Publish a version of A with it: it lists at once, unverified. The
     owner's `/me/plugins` shows that version with `via: "deploy_token"`
     and `deploy_token: {short, name}`, and the token's `last_used_plugin`
     is A.
  3. Publish B with it, and a new id with it: both 403
     `token_not_allowed`. A mistyped token is 401 `invalid_token`; the token
     on any other route (`GET EDGE/store/v1/me/tokens`) is 401
     `session_required`. Scoping a token to a plugin the account doesn't
     own is 403 `not_owner`.
  4. Revoke it (`DELETE EDGE/store/v1/me/tokens/<id>` → `{"revoked":true}`)
     and see the next publish answer 401 `invalid_token`.

Operator routes for incidents:

```sh
curl -s "EDGE/admin/deploy-tokens" -H "authorization: Bearer $ADMIN_TOKEN"
#   {"live": n}
curl -s -X POST "EDGE/admin/deploy-tokens/revoke" -H "authorization: Bearer $ADMIN_TOKEN" \
  -H "content-type: application/json" -d '{"pluginId":"<id>"}'
#   {"revoked": n}: its owner's tokens whose scope may hold it (listed, or all), e.g. a leaked token
curl -s -X POST "EDGE/admin/deploy-tokens/revoke" -H "authorization: Bearer $ADMIN_TOKEN" \
  -H "content-type: application/json" -d '{"all":true}'
#   {"revoked": n}: every live token
```

## 6. Rolling back

Web sessions, roles and deploy tokens are ignored by older edge code: it
has no token routes and never reads `deploy_tokens`, so a token publishes
nothing while it runs. Nothing to do before rolling back. Tokens work again
when the release comes back, unless an operator revoked them meanwhile.
