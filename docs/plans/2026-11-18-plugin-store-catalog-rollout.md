# Plugin store: catalog and review rollout

Operator notes for shipping the edge's store: publishing, review, the
catalog, files and update checks (`docs/plans/2026-11-17-plugin-store.md`).
This is not user documentation. The identity pieces (web sign-in, staff
roles, deploy keys) have their own notes,
`docs/plans/2026-11-18-plugin-store-identity-rollout.md`. Do those notes'
steps 1–4 first, because review needs a web session made staff. Do staging
first, then production, in this order.

Code: `edge/src/store/*` (not `identity/`), `edge/migrations/store/`, and
the cron call in `edge/src/index.ts`. Limits and measurements are in
`docs/plugin-store-limits.md`; the local run is in `docs/plugin-store-dev.md`.

`EDGE` and `SITE` mean what they mean in the identity notes: the edge's own
address and the site's origin, for the environment.

## 1. Before anything reaches main (hard gate)

`edge/wrangler.jsonc` binds `STORE_DB` to **placeholder database ids**:
`00000000-0000-0000-0000-000000000000` for production and `…0001` for
staging. A deploy with a placeholder id fails, and once the deploy job
applies migrations (step 2), a placeholder on main breaks every edge
deploy. So, before the store merges to main:

1. Create the databases and put their ids in `edge/wrangler.jsonc`
   (top level for production, `env.staging` for staging):
   ```sh
   npx wrangler d1 create zeron-store
   npx wrangler d1 create zeron-store-staging
   ```
2. Create the R2 buckets `zeron-plugins` and `zeron-staging-plugins`. They
   are private: nothing is served from R2 directly; every byte goes through
   the store's routes.
3. Check that the rate limit namespace ids don't clash with others on the
   account: `STORE_READS` is 1003 (staging 1004), 300 a minute per IP;
   `STORE_WRITES` is 1005 (staging 1006), 30 a minute per account.
4. Give the deploy token D1 edit access. The migrations step needs it.
5. Apply the migrations to staging once, by hand:
   `cd edge && npx wrangler d1 migrations apply STORE_DB --remote --env staging`.

Nothing else is new:
- No secret.
- No route: the site reaches `/store/v1/*` over its service binding, and
  the app and CLI reach the edge's own address. `zeron.sh/store/v1/*` is
  deliberately not routed (amendment 8).
- The hourly cron (`17 * * * *`) already exists. The store adds its sweep
  to it.
- `limits.cpu_ms` is pinned at 30,000 (`docs/plugin-store-limits.md` says
  why).

## 2. Deploy

The edge job in `.github/workflows/deploy.yml` gains one step before
`wrangler deploy`:
```sh
npx wrangler d1 migrations apply STORE_DB --remote
```
Add `--env staging` for staging. The coordinator adds it at the final
merge. It is idempotent: an applied migration is skipped, so it runs on
every deploy. Then deploy the edge, then the site.

Migrations are additive, never edited once applied. A new schema change is
a new numbered file in `edge/migrations/store/`.

## 3. Check after the deploy

Run each check on staging before production. Mind `STORE_WRITES`: 30
writes a minute per account, so space scripted publishes.

### a. The store answers

```sh
curl -s "EDGE/store/v1/categories" -w ' %{http_code}\n'
#   {"categories":[{"id":"agents",…},…]} 200 (eight categories)
curl -s "EDGE/store/v1/plugins" -w ' %{http_code}\n'
#   {"items":[]} 200; Cache-Control: public, max-age=60
curl -s "EDGE/store/v1/plugins/nope" -w ' %{http_code}\n'
#   {"error":"not_found",…} 404
```

`503 {"error":"unavailable"}` on every store route means the D1 or R2
binding is missing from that environment.

### b. A publish, a review, a listing

1. Make a test plugin from the app (or `zeron plugin new`) under an
   account with a username. Publish it (`zeron plugin publish`, or the
   app's Publish…). It answers `pending`, with the checks.
2. The owner's `GET EDGE/store/v1/me/plugins` (app bearer) lists it as
   `unlisted`, with the version `pending`.
3. An admin, signed in on the web, sees it in the site's review queue with
   `checks` counts and `new_asks`. They open it, read the files and
   approve it.
4. Within 60 s it lists: `GET EDGE/store/v1/plugins/<id>` gives the card
   and the detail. `icon` and `screenshots` are package paths, fetched as
   `GET EDGE/store/v1/plugins/<id>/files/<version>/<path>`. The icon is
   served as `image/svg+xml` with `Content-Security-Policy: sandbox;
   default-src 'none'`.
5. Install it from another device. The package
   (`…/versions/<version>/package`) is the published bytes, and the device
   checks its signature and hash.

### c. Updates, a yank, a takedown

1. Publish 1.0.1 and approve it. The first device's update check offers it,
   with its notes and any permission it adds.
2. The owner yanks 1.0.1. It isn't offered to new installs, and a device
   that has it keeps it. Then unyank it.
3. An admin removes 1.0.1 with a block and a reason. Within 60 s, a
   device's update check lists it under `blocked` with the reason, and the
   device turns it off. Lists and details can be up to 60 s stale; the
   package and files routes check the state on every request.

### d. A report

From a signed-in account, report the plugin. In the site's Reports tab
(`/admin/reports`) it is open, with its reporter and plugin. Resolve it.
The audit log has `report.dismissed` or `report.actioned`.

### e. The hourly sweep

After the next `:17`, watch `npx wrangler tail [--env staging]`: there is
no `store: …` error. The sweep releases ids claimed and never used for 90
days (a reservation is always kept), and deletes R2 objects that publishes
recorded but no version names, an hour after.

### f. Clean up

An admin can't delete a plugin. A test plugin on production can be
suspended, or its versions removed. On staging, leave it.

## 4. Watching it

- `wrangler tail`: the store logs one line per failure:
  - `store: a request failed <method> <path> <error>` (the answer is a plain
    500);
  - `store: couldn't record possibly orphaned objects` (only garbage left in
    R2; the sweep can't see it).
- D1: `npx wrangler d1 execute STORE_DB --remote --command "SELECT status,
  COUNT(*) FROM versions GROUP BY status"` gives the queue's size at a
  glance.
- R2: objects are content-addressed (`pkg/<id>/<version>/<sha256>…`,
  `files/…`) and never overwritten. A version's objects stay while any row
  names them.

## 5. Incidents

- **A harmful plugin**:
  - remove the version with `block: true` and a reason (devices turn it
    off at their next update check);
  - suspend the plugin, which takes it out of every list;
  - then revoke its deploy keys (identity notes, step 5).
- **A leaked account**: suspend its plugins; the identity notes cover its
  keys.
- **The store itself misbehaving**: deploy the edge with the `STORE_DB`
  binding removed from that environment's `d1_databases`. Every store route
  then answers `503 unavailable`, and the rest of the edge is untouched. The
  app says the store is unreachable, and installed plugins keep working.

## 6. Rolling back

- Older edge code has no store routes: they answer 404. The app and site
  show the store as unreachable. Installed plugins are unaffected.
- D1 and R2 are left as they are. Rolling forward again resumes where it
  was, and an applied migration is never re-run.
- A bad data change can be undone with D1 Time Travel:
  `npx wrangler d1 time-travel restore zeron-store --timestamp <before>`.
  It restores the whole database to that moment. R2 objects are never
  rewritten, so they need no restore.
- Deploy keys are the one thing not safe to leave live past a rollback
  (identity notes, step 6).
