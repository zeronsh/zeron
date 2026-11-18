# Plugin store: running it locally

The store is part of the edge worker (`edge/src/store/`). Locally it runs
under `wrangler dev` with a local D1 database and a local R2 bucket.

```sh
cd edge
npm ci
# The store's schema, into the local D1 (once, and after any new migration):
npx wrangler d1 migrations apply STORE_DB --local --persist-to .wrangler/state
# The worker, in dev auth (a bearer is its user id: `Authorization: Bearer user_abc`):
npm run dev -- --persist-to .wrangler/state
```

`npm run dev` builds the card pages' assets and starts `wrangler dev` on
port 27640 with `AUTH_MODE=dev`. Both commands must use the same
`--persist-to`, or the worker starts with an empty database. The
placeholder D1 ids in `wrangler.jsonc` are fine locally.

To publish, an account needs a username and a registered device key, as in
production:

1. A directory user: in dev, `POST /workos/webhook` needs the webhook
   secret, so tests create users with the user directory's `upsert`
   (`test/workerd/store-helpers.ts` `member`). Locally, set
   `--var WORKOS_WEBHOOK_SECRET:dev` and send a signed `user.created`, or
   use the workerd tests' helpers.
2. `POST /username {username}` with the user's bearer.
3. `POST /plugin-keys {key, proof, stepUp}` (a step-up ticket comes from a
   fresh sign-in; `/auth/exchange` mints it).

Admin routes take a web session only (amendment 27). Locally that means the
web sign-in flow (`/auth/web/*`, which needs WorkOS) and an operator grant:
`POST /admin/roles/grant {userId, role: "admin"}` with
`Authorization: Bearer <ADMIN_TOKEN>` (set `--var ADMIN_TOKEN:…`).

The hourly sweep (released ids, owed and orphaned objects) runs from the
worker's `scheduled` handler: trigger it locally with
`curl "http://localhost:27640/__scheduled?cron=17+*+*+*+*"` when
`wrangler dev` runs with `--test-scheduled`.

Tests: `npm test` runs both suites (Node and workerd); the workerd suite
applies the migrations itself (`test/workerd/store-setup.ts`).
