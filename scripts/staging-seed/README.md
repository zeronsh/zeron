# A seeded catalog for staging

`seed.mjs` gives a store a realistic catalog through its real paths only. Each
author's own signed-in Zeron publishes their plugins' first versions, signed
with that device's key (`zeron plugin publish <id>`). A CI-style checkout
publishes the later ones with the author's deploy token, unsigned
(`ZERON_DEPLOY_TOKEN=… zeron plugin publish <folder>`). The author's Zeron
yanks what the catalog says to (`zeron plugin yank`). Nothing but the CLI
writes to the store: no database writes, no edge internals, no admin token.

`catalog.mjs` is what gets published: 30 plugins by six authors, 49 versions.

- **Coverage:** every category, keywords that overlap for search and filters,
  and two pages of 24.
- **Version cases:** a plugin with four versions, and a chain published before
  anyone verified it (superseded). Also a release candidate its author yanked,
  a pre-release, and an update that asks for more (`network`).
- **Permissions:** programs a plugin runs (`date`, `python3`, `git`), named
  hosts, and 3D scenes (`scene3d`).
- **Pictures:** READMEs with screenshots, one README picture the store can't
  show (a GIF), and one plugin with no screenshots.

## The sessions come from outside

The seed never signs anyone in. Each author is a running Zeron (`zeron
headless`, or the app), signed in as that account, with a username. On one
machine, each author has their own `HOME`, `ZERON_DATA_DIR` and
`ZERON_IPC_PORT`. Each author signs in once, by any real path:
`zeron login`'s headless code, or the app. Their accounts and usernames are the logins file's: none is in this folder.

The accounts file names them:

```json
{
  "edge": "<edge>",
  "workos_client_id": "<client id>",
  "zeron": "/path/to/zeron",
  "authors": [
    { "slot": 1, "home": "/seed/ada/home", "data_dir": "/seed/ada/data", "ipc_port": 27701, "token_env": "SEED_TOKEN_1" }
  ]
}
```

`slot` is the catalog's author (1–6), and `zeron` defaults to the one on
`PATH`. Deploy tokens are never in the file, only in the environment, under
each author's `token_env`.

## Where it may write (where.mjs)

An id published is never released, so the seeds and `sessions.mjs` write only
to an edge the operator allows by name: `--edge` must be one of
`SEED_ALLOWED_EDGES` (addresses, separated by commas). Production's hosts are
refused even when they're listed.

What an author's Zeron publishes goes to its own edge, whatever the seed was
told. So the accounts file says which edge that is (the same as `--edge`), and
both seeds ask each Zeron for its session first: it must be of the WorkOS the
file names, and never of production's, which is read from `edge/wrangler.jsonc`
(a seed that can't read it refuses). Nothing names an account in what's printed.

```sh
export SEED_ALLOWED_EDGES=<edge>
```

## Making the sessions (sessions.mjs)

`sessions.mjs` makes them through the real paths, on one machine. It runs one
`zeron headless` per author, with its own `HOME`, data folder and IPC port
(27711–27716). Its `up` command:
- signs each author in with the engine's headless sign-in: AuthKit's hosted
  page, then the edge's `/auth/cli/callback` code;
- gives each a workspace of its own (`CreateOrg`, as the app's first run does;
  the engine counts a session as signed in only in a workspace);
- claims each author's username.

Its `tokens` command makes each author's deploy token with the CLI. A headless
browser (Playwright) fills the hosted page with each account's email and
password: nothing is bypassed.

```sh
node scripts/staging-seed/sessions.mjs up --zeron <zeron build> --edge <edge> \
  --client-id <client id> --logins <logins.json> --dir <folder> --playwright <folder with playwright and ws>
node scripts/staging-seed/seed.mjs --accounts <folder>/accounts.json --edge <edge>   # pass 1, exit 3
node scripts/staging-seed/sessions.mjs tokens …the same…                        # writes <folder>/tokens.env (0600)
node scripts/staging-seed/sessions.mjs up …the same…                            # restarts the engines `tokens` stopped
(set -a; . <folder>/tokens.env; set +a; node scripts/staging-seed/seed.mjs --accounts <folder>/accounts.json --edge <edge>)
node scripts/staging-seed/sessions.mjs down --dir <folder>                      # stops the engines by their own pids
```

The logins file lists the accounts in the catalogs' order (six authors of
plugins, then two of templates), as `{username, email, user_id, password}`.
`--only 7,8` does some authors only. Passwords are read only: never printed,
logged or written.

The folder keeps each author's home and data, `accounts.json` and `tokens.env`,
for reruns. `up` signs in only who isn't signed in, and `tokens` makes only the
tokens not yet made.

A fresh sign-in comes back to the engine's loopback, on the one port the
redirect list names (27641, every engine's). So `tokens` runs one author's
engine at a time.

## Running it

```sh
node scripts/staging-seed/seed.mjs --accounts accounts.json --edge <edge>
```

1. **The first pass** publishes every plugin's first version from its author's
   Zeron, then stops (exit 3). It stops because Zeron makes a deploy token
   only once the account has a plugin in the store.
2. **Each author makes a token** on their own Zeron:
   `zeron plugin token create --all --name "Seed CI"`. It asks for a fresh
   sign-in. Set the token as that author's `token_env`.
3. **Run it again.** It publishes the rest and yanks what it should (exit 0).

The seed reads the store's public version lists to skip what is already
there, so a run that stopped part way picks up where it was. Those lists are
cached for up to 60 s, so wait a minute before running it again.

`--only 1,3` seeds some authors only. `--dry-run` checks every version with
`zeron plugin check`, in folders of its own: it sends nothing and writes in no
Zeron's folder (it still asks each Zeron for its session first).

Exit codes:
- 0: everything is published;
- 1: a refusal, with what the CLI said;
- 2: a usage or set-up problem;
- 3: the first versions are published, and tokens are needed for the rest.

The store's limits shape the catalog:
- an account claims at most 5 new ids in 7 days, so 5 plugins an author;
- it publishes at most 10 versions an hour and 50 a day, so at most 10
  versions an author;
- it takes 30 writes a minute, so the seed waits 2.5 s between writes.

## Templates (seed-templates.mjs)

`templates.mjs` is eight templates by two more authors (slots 7 and 8 of the
logins file). They have accounts of their own because an account claims at
most 5 new ids in 7 days, plugins and templates together. `pictures.mjs` draws
each one's preview and two screenshots (1280×800) in a headless browser.

- Each has a palette of its own (`theme.luau`), and some a file of Zeron's made
  over with the few lines they mean to change (tokens, the new-chat page, a
  preference), or one unchanged. A review then has files that are the same as
  Zeron's, changed and new.
- First versions come from the author's own Zeron, as the app publishes one
  (`PrepareTemplatePublish`, `PublishTemplate` over its IPC). The CLI publishes
  a template only with a deploy token (`zeron plugin token create --all
  --templates`).
- A first set of versions was published with drawn pictures (`pictures.mjs`)
  that showed more than the files change. The versions after put that right:
  files, words and pictures (`rendered`), the pictures being the app's own.
- `wide-margin` has a pre-release ahead of its release.
- For staff: never verify `green-phosphor`; take `slate-grid` down.

```sh
git archive <commit> apps/zgx/ui | tar -x -C <folder>     # Zeron's own files, as the app they run in has them
node scripts/staging-seed/sessions.mjs up --only 7,8 --edge <edge> …as above…   # writes accounts-templates.json
node scripts/staging-seed/seed-templates.mjs --accounts <folder>/accounts-templates.json --ui <folder>/apps/zgx/ui --playwright <folder> --edge <edge>   # pass 1, exit 3
node scripts/staging-seed/sessions.mjs tokens --only 7,8 --templates …
node scripts/staging-seed/sessions.mjs up --only 7,8 …
(set -a; . <folder>/tokens.env; set +a; node scripts/staging-seed/seed-templates.mjs …the same…)
node scripts/staging-seed/sessions.mjs down --only 7,8 --dir <folder>
```

The versions that correct a template's pictures and words use the app's own
renderings (`--pictures <folder>`) and wait without them.

`--dry-run` checks every version with `zeron template check`. The engine must
be one that publishes templates. `local-templates.mjs` runs it all against a
local edge first, as `local.mjs` does for the plugins.

## What's left for staff (in the browser)

The seed publishes and yanks. Staff marks are a person's job: Wing, signed in
on the site with the admin role.

- Verify about half the catalog in the review queue. That gives the verified
  marks, `?verified=1` and `sort=verified`.
- Take one version down with a reason, then restore it.
- Resolve a report.
- Set a reservation.

A version verified after something newer superseded it stays superseded. To
see a verified version beside a newer unverified one, verify a plugin's
listed version, then publish one more with its token.

## Checked locally

```sh
node scripts/staging-seed/local.mjs --zeron target/debug/zeron
```

It needs `scripts/plugin-store-e2e/edge.mjs`, which is on `plugin-store/all`.
It runs the real edge in Miniflare with that file's WorkOS fake. It starts six
real `zeron headless` engines, signs each in as a device does, and claims its
username. It runs the seed's first pass, makes each author's token with the
CLI (playing the fresh sign-in), and runs the second pass. Then it checks:

- 30 listed, 24 and 6 across two pages, 5 for each author;
- every category, a search and a keyword filter;
- all 49 versions, the devices' signed and the tokens' unsigned (`key: null`);
- the superseded chain, the author's yank, the pre-release;
- screenshots served as PNG and icons as SVG, and the GIF warned of
  (`picture_unshown`);
- the update offered with what it adds;
- the token's name on the owner's versions;
- a token for two plugins refused on a third (exit 3, `token_not_allowed`);
- a second run that writes nothing.
