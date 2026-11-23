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
`zeron login`'s headless code, or the app. The usernames the catalog's
authors are listed under are in `catalog.mjs` (`stg-ada` … `stg-fen`).

The accounts file names them:

```json
{
  "edge": "https://staging.zeron.sh",
  "workos_client_id": "client_01KWD0E9SYPV2WE4QN9TP9DP0W",
  "zeron": "/path/to/zeron",
  "authors": [
    { "slot": 1, "home": "/seed/ada/home", "data_dir": "/seed/ada/data", "ipc_port": 27701, "token_env": "SEED_TOKEN_1" }
  ]
}
```

`slot` is the catalog's author (1–6), and `zeron` defaults to the one on
`PATH`. Deploy tokens are never in the file, only in the environment, under
each author's `token_env`.

## Running it

```sh
node scripts/staging-seed/seed.mjs --accounts accounts.json
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

`--only 1,3` seeds some authors only. `--dry-run` checks and packs every
version without sending anything.

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
