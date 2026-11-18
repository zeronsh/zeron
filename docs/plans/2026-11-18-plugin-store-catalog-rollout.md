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
every deploy.

The order is migrations, then the edge, then the site. New code needs the
tables. Old code still running during the deploy never reads them, so
applying a migration first is always safe.

Migrations are additive, never edited once applied. A new schema change is
a new numbered file in `edge/migrations/store/`.

Until the first deploy applies them, nothing has, so the schema is one
file, `0001_store.sql` (amendments 44 and 45 folded into it; it replaced
`0001_init.sql` and `0002_audit_plugin.sql`). A database that had those
two applied by hand (a local `.wrangler/state`, or a staging trial) is
recreated, not migrated: delete it and apply again. From the first deploy
on, the rule above holds.

## 3. Check after the deploy

Run each check on staging before production. Mind `STORE_WRITES`: 30
writes a minute per account, so space scripted publishes.

### a. The store answers

```sh
curl -s "EDGE/store/v1/categories" -w ' %{http_code}\n'
#   {"categories":[{"id":"agents","name":…,"count":0},…]} 200 (eight categories)
curl -s "EDGE/store/v1/plugins" -w ' %{http_code}\n'
#   {"items":[],"total":0} 200; Cache-Control: public, max-age=60
curl -s "EDGE/store/v1/stats" -w ' %{http_code}\n'
#   {"plugins":0,"verified":0} 200
curl -s "EDGE/store/v1/takedowns" -w ' %{http_code}\n'
#   {"events":[],"head":0,"reset":true} 200
curl -s "EDGE/store/v1/plugins/nope" -w ' %{http_code}\n'
#   {"error":"not_found",…} 404
```

`503 {"error":"unavailable"}` on every store route means the D1 or R2
binding is missing from that environment.

### b. A publish, a review, a listing

1. Make a test plugin from the app (or `zeron plugin new`) under an
   account with a username. Publish it (`zeron plugin publish`, or the
   app's Publish…). It answers `listed`, `verified: false`, with the
   checks: publishing is immediate (amendment 45).
2. At once, `GET EDGE/store/v1/plugins/<id>` gives the card and the
   detail, `verified: false`. The owner's `GET EDGE/store/v1/me/plugins`
   (app bearer) lists the version.
3. An admin, signed in on the web, sees it in the site's review queue
   (`state=undecided`) with `checks` counts and `new_asks`. They open it
   (`/admin/versions/<id>/<version>`), read the files and verify it.
4. Within 60 s the card says `verified: true` and `verified_version`, and
   `?verified=1` lists it. `/stats` counts it under both `plugins` and
   `verified`, and its categories' `count` include it: the counts always
   equal what the lists show. `?author=<username>` lists it too. `icon` and `screenshots` are package paths, fetched as
   `GET EDGE/store/v1/plugins/<id>/files/<version>/<path>`. The icon is
   served as `image/svg+xml` with `Content-Security-Policy: sandbox;
   default-src 'none'`.
5. Install it from another device. The package
   (`…/versions/<version>/package`) is the published bytes, and the device
   checks its signature and hash.

### c. Updates, a yank, a takedown

1. Publish 1.0.1. The first device's update check offers it at once,
   `verified: false`, with its notes and any permission it adds, and
   `verified_update` naming 1.0.0 if that one was verified.
2. Publish 1.0.2 without verifying 1.0.1: 1.0.1 becomes `yanked`,
   `yanked_by: "superseded"` (unless verified), and the queue shows 1.0.2
   alone, with `earlier_undecided`.
3. The owner yanks 1.0.2. It isn't offered to new installs, and a device
   that has it keeps it. Then unyank it.
4. An admin takes 1.0.2 down with a reason. Within 60 s, a device's update
   check lists it under `blocked` with the reason, and the device turns it
   off; `GET EDGE/store/v1/takedowns?after=0` has the event, with its
   `seq` (and `head` is that seq). Restore it: a `restore` event follows, and the version comes back
   unverified. Lists and details can be up to 60 s stale; the package and
   files routes check the state on every request.

A device asking with `verified_only` for the plugin (an agent's install,
or an author's choice) is offered only verified versions: holding 0.9.0,
with 1.0.0 verified and 1.0.1 not, it is offered 1.0.0, where a device
without it is offered 1.0.1 with `verified_update` naming 1.0.0.

### d. A report

From a signed-in account, report the plugin. In the site's Reports tab
(`/admin/reports`) it is open, with its reporter and plugin. Resolve it.
The audit log has `report.dismissed` or `report.actioned`.

### e. The hourly sweep

The cron at `:17` runs one sweep, in four parts:
- it finishes what earlier releases still owe (R2 deletes);
- it deletes R2 objects that publishes recorded but that no version names,
  an hour after they were recorded;
- it drops take-down feed events older than 30 days, always keeping the
  newest (a device behind that gets `reset: true`);
- it releases ids that were claimed and never published to for 90 days.
  An id reserved for an account is never released.

To see it ran, watch `npx wrangler tail [--env staging]` across a `:17`.
Each run logs one line:

```
store: sweep: released 0 unused ids, deleted 0 orphaned objects, 0 owed left
```

`store: the sweep failed …` means it threw. The next hour retries,
because all of its work is recorded in D1 first. Afterwards:

```sh
npx wrangler d1 execute STORE_DB --remote --command \
  "SELECT (SELECT COUNT(*) FROM object_orphans WHERE at < (unixepoch() - 7200) * 1000) AS stale_orphans,
          (SELECT COUNT(*) FROM object_purges) AS owed"
#   both 0 once a sweep has run (or close to it, while one is still catching up)
```

A released id is in the audit log as `plugin.release`, with the versions
it had.

### f. Seed the reserved ids

The tree already holds back some ids:
- the built-in plugins' ids and the engine's reserved names
  (`docs/plugin-store-data.json`, `builtin_ids` and `reserved_ids`);
- the username rules' brand, staff and lookalike words
  (`edge/src/username.ts`);
- any id too close to one already taken.

A reservation decides before the first two: one naming an account lets
that account take even a held-back id.

Reservations cover the rest: names held for a partner or for Zeron's own
later plugins. For each one, an admin's web session (cookie and `csrf` from
`SITE/auth/web/session`, as in the identity notes, step 3d) calls:

```sh
curl -s -X POST "EDGE/store/v1/admin/reservations" \
  -H "cookie: __Host-zeron_session=$COOKIE" -H "origin: SITE" -H "x-csrf-token: $CSRF" \
  -H "content-type: application/json" \
  -d '{"id":"<plugin-id>","to_username":"<their username, or leave out to hold it for no one>","note":"<why>"}'
#   {"id":"…","to":{"username":"…"}|null,"note":"…","released":false}
```

- A reserved id can be claimed only by the account it names. With no
  account, no one can claim it.
- An id someone claimed and never used is released for the reservation
  (`"released": true`). An id in use is refused with `409`: move it with a
  transfer instead.
- `GET EDGE/store/v1/admin/reservations` lists them, and
  `DELETE …/reservations/<id>` frees one.
- Each change is in the audit log (`reservation.set`,
  `reservation.release`).
- The staff session rotates its cookie on first use. Take the new
  `__Host-zeron_session` from each answer's `Set-Cookie`. The old one works
  for a minute.

### g. Clean up

An admin can't delete a plugin. A test plugin on production can be
suspended, or its versions removed. On staging, leave it.

## 4. Watching it

- `wrangler tail`: the store logs one line per failure:
  - `store: a request failed <method> <path> <error>` (the answer is a plain
    500);
  - `store: couldn't record possibly orphaned objects` (only garbage left in
    R2; the sweep can't see it).
- D1: `npx wrangler d1 execute STORE_DB --remote --command "SELECT status,
  verification, COUNT(*) FROM versions GROUP BY 1, 2"` gives the queue's
  size at a glance (`listed`/`yanked` and `unverified`).
- R2: objects are content-addressed (`pkg/<id>/<version>/<sha256>…`,
  `files/…`) and never overwritten. A version's objects stay while any row
  names them.

## 5. Incidents

- **A harmful plugin**:
  - take the version down with a reason (devices turn it off at their
    next update check, and the take-down feed has it at once);
  - suspend the plugin, which takes it out of every list;
  - then revoke its deploy keys (identity notes, step 5).
- **A leaked account**: suspend its plugins; the identity notes cover its
  keys.
- **Zeron's own badge (`maintainer`) taken back** (`POST
  EDGE/admin/badges/revoke`, operator token): never refused. It answers
  `{revoked, suspended: [ids]}`: each of the account's plugins whose
  listing says it's Zeron's own (`claims_zeron`) is suspended, in the audit
  as `plugin.suspend` by `operator`; installed copies keep running. Granting
  the badge back relists nothing. A suspended plugin takes no new version,
  so its owner can't rename it: staff bring it back with a restore, once it
  is transferred to an account with the badge or the versions that say so
  are taken down (until then the restore answers 409 `claims_zeron`).
- **The store itself misbehaving**: deploy the edge with the `STORE_DB`
  binding removed from that environment's `d1_databases`. Every store route
  then answers `503 unavailable`, and the rest of the edge is untouched. The
  app says the store is unreachable, and installed plugins keep working.

## 6. Rolling back

- Older edge code has no store routes: they answer 404. The app and site
  show the store as unreachable. Installed plugins are unaffected.
- D1 and R2 are left as they are: every table, row and object. Rolling
  forward again resumes where it was.
- **Migrations are never undone.** There are no down migrations, and none
  is needed. Older code doesn't read the store's tables, and the next
  deploy of this code finds them applied and skips them. Dropping them
  would only lose the catalog.
- A bad data change can be undone with D1 Time Travel:
  `npx wrangler d1 time-travel restore zeron-store --timestamp <before>`.
  It restores the whole database to that moment. R2 objects are never
  rewritten, so they need no restore.
- Deploy keys are the one thing not safe to leave live past a rollback
  (identity notes, step 6).
