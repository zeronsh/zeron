# Plugin store

Status: in progress on `zeron/plugin-store` (integration branch). Owner: the
coordinator chat. Lanes work on `plugin-store/<lane>` branches cut from the
integration branch; the coordinator merges them.

This is the plan and the shared contract. A lane that needs the contract
changed says so to the coordinator first; it never changes a shared shape on
its own.

## What we are building

1. A public catalog of plugins, on the web (`zeron.sh/plugins`) and in the
   desktop app (Settings → Plugins → Browse).
2. A plugin page for each plugin, written by its author as `README.md`.
3. Publishing with review: every publish, updates included, is reviewed by a
   Zeron admin before anyone else sees it. Its author can use it on all of
   their own devices straight away. Everyone else can still sideload a plugin
   from a file, a link or GitHub, as today.
4. Deploy keys, so an author (or their GitHub Action) can push updates
   without a signed-in session.
5. A real versioning system: strict semver, immutable versions, pre-releases,
   compatibility with a Zeron version, changelogs, yank, takedown.
6. An admin dashboard on the web, signed in with a WorkOS account, open only
   to accounts that hold the `admin` role at the edge.
7. Free-form settings pages: a plugin draws its own settings page, with
   strong guidance (docs, scaffold, example, status snapshot) to match
   Zeron's own pages.
8. "Install in Zeron" on the web: a `zeron://` link that opens the app on
   that plugin's install dialog.

## What exists today (facts the design builds on)

- A plugin is a folder with a signed `zeron-plugin.json`; it travels as one
  `.zeron-plugin` file (`zeron-plugin:v1`: signed manifest + every file as
  base64; 2,000 files, 50 MB). `crates/engine/src/plugins/{package,install,
  signing,manifest}.rs`.
- Identity is the manifest `id` plus the ed25519 key that signed it. Each
  device has one key, registered to the signed-in account at the edge
  (`edge/src/plugin-keys.ts`, table `plugin_keys` in the `users1` Durable
  Object). "by @username" comes from that lookup, never from the manifest.
- Install sources: a file, a link, a GitHub release, a git repository. Each
  device polls a plugin's own source every 6 hours. There is no catalog, no
  central version index, no review, no publish command.
- `version` is any text; `install::newer` compares semver when both parse.
  No compatibility field.
- Consent is per device and bound to author key (or content hash),
  permissions and the capability record; anything that grows asks again.
- Settings pages are declarative only (`settings` in the manifest; Zeron
  draws them in its own VM; plugins can read values, never draw).
- The app already handles `zeron://` links (`apps/zgx/src/links.rs`).
- The edge has no D1, no browser session, and no role beyond the shared
  `ADMIN_TOKEN`. zeron.sh is a static Astro site; its README names the plan:
  add `@astrojs/cloudflare` and mark server routes `prerender = false`.
- WorkOS roles are per organization and every user is `admin` of their own
  personal organization, so the WorkOS role can never gate staff access.

## Decisions

### Names
- A store id is the plugin's manifest `id`, unique across the store. The
  first account to submit an id owns it. Reserved: built-in ids (`git`,
  `github`, `parallax-scenes`), `zeron*`, and the same brand and lookalike
  rules usernames use (`edge/src/username.ts`).
- Publishing needs a username (so every listing reads "by @username").
- Store URL: `https://zeron.sh/plugins/<id>`.
- The catalog carries a `kind` column (`plugin` today) so templates can join
  later without a new system.

### Where data lives
- **D1** (`zeron-store`, plus a staging database): plugins, versions, tags,
  full-text index (FTS5), review decisions, audit log, reports, install
  counts, name reservations. A catalog is shared relational data read by
  everyone; that is what D1 is for. Migrations live in
  `edge/migrations/store/` and are applied by `wrangler d1 migrations apply`
  in the deploy job, before the worker deploys.
- **R2** (`zeron-plugins`): packages, content-addressed and immutable
  (`pkg/<id>/<version>/<sha256>.zeron-plugin`), and each version's unpacked
  files (`files/<id>/<version>/<path>`) so README pictures, icons,
  screenshots and the reviewer's file viewer are served without unpacking.
- **`users1` Durable Object** (existing): the `admin` role and deploy keys,
  beside the device keys they extend.
- Public reads go through the Workers Cache API with short TTLs and are
  purged on approve, yank and takedown. Writes are rate limited.

### Listing metadata (manifest additions, all optional except where said)
```json
{
  "categories": ["git"],                 // 1–2, from the fixed list
  "keywords": ["github", "review"],      // up to 8, free text, lowercase
  "homepage": "https://…", "repository": "https://…", "license": "MIT",
  "screenshots": ["shots/board.png"],    // up to 8, files in the package
  "engines": { "zeron": ">=0.4.0" }      // semver range of Zeron versions
}
```
- `README.md` in the plugin's folder is its page. `CHANGELOG.md` gives each
  version's notes (the section for that version). Both are ordinary signed
  files, so what was reviewed is what is shown.
- The store needs: `README.md`, `description`, an `icon`, one category, and
  a strict semver `version`. Sideloading needs none of these.
- README rules: CommonMark + GFM. Raw HTML is dropped. Pictures must be
  files in the package (relative paths); a remote picture is shown as a
  link, because what is shown must be what was reviewed. Links open in the
  browser with `rel="nofollow noopener ugc"`.
- Unknown manifest fields are already kept and signed, so old apps install
  new packages unchanged.

### Versions
- Strict SemVer 2.0.0 for anything submitted. A version, once submitted,
  belongs to that content hash for good, approved or not: a rejected author
  submits a new version. (Devices already refuse "same version, different
  content".)
- Version states: `pending` → `approved` | `rejected` | `withdrawn`;
  `approved` → `yanked` (author: no new installs, existing keep working) |
  `removed` (admin takedown, optionally `blocked`).
- "Latest" for a device is the highest approved, not yanked version whose
  `engines.zeron` accepts that device's Zeron, pre-releases left out unless
  that device opted in for that plugin.
- A blocked version is turned off on devices at their next update check,
  with the reason shown; the person can turn it back on (they own it).
- Update checks are one batched request for every store-installed plugin.
  Installing an update stays a person's click, as today, and shows the
  changelog and the permissions it adds or removes.

### Who may publish, and how it is proven
Two ways, one model: every package is signed by a key the edge has bound to
the publishing account.
- **From your machine** (app or CLI): the package is signed with this
  device's key and uploaded with your signed-in session.
- **From CI**: a **deploy key**. It is an ed25519 key pair made on your
  device, scoped to one plugin id, registered at the edge with a fresh
  sign-in (step-up) and a proof of possession. The secret (`zpk_…`, shown
  once, never sent to the edge) goes in the CI's secrets. In CI,
  `zeron plugin publish` signs the package with it and signs the upload
  request with it. One secret does both; nothing else to leak. Keys have a
  name, a last-used time, an optional expiry, and can be revoked.
- A revoked key can publish nothing. Versions already approved stay good:
  for a store install, the store's record (publisher, version, content
  hash, approval) is what the device trusts, and the package's signature
  and hash must match that record.
- Designed for, built after the core: GitHub trusted publishing (the Action
  proves its repository with GitHub's OIDC token and registers a one-use
  key), so a repository needs no secret at all.

### Review
- Every submission lands in the queue as `pending` with automated checks
  already run (manifest rules, signature, sizes, strict semver, newer than
  the last, README present, binary and bytecode files flagged, new
  permissions / commands / hosts since the last approved version).
- The reviewer sees: the publisher (username, account age, past
  decisions), what it asks for with the risky asks called out, the README
  as it will show, every file, and a diff against the last approved
  version. Approve, or reject with a reason the author reads.
- The author sees each version's state and reason in the app, the CLI and
  the web. Their own devices offer their pending versions as updates
  ("In review"); nobody else's do.
- Everything an admin does is in an audit log.
- Anyone signed in can report a listed plugin; reports land in the
  dashboard.

### Admin role
- A row in `users1` (`user_roles`: user id, role, granted at, granted by),
  granted and revoked with operator routes behind `ADMIN_TOKEN`
  (`/admin/roles`, `/admin/roles/grant`, `/admin/roles/revoke`). The WorkOS
  token only says who you are; the edge's own record says you are an admin.

### Web sign-in
- New, owned by the edge: `/auth/web/login`, `/auth/web/callback`,
  `/auth/web/logout`, `/auth/web/session` on `zeron.sh`. WorkOS AuthKit
  authorization-code flow, then a `__Host-` session cookie (HttpOnly,
  Secure, SameSite=Lax), server-side revocable, short-lived for admins.
  State checked on callback; every mutation checks Origin and a CSRF token.
- The site (Astro with `@astrojs/cloudflare`) reads the session through a
  service binding to the edge. It never holds a WorkOS secret.

### Search, filters, paging
- Search: FTS5 over name, summary, keywords, author and README (weighted in
  that order), prefix matching, bm25 ranking.
- Filters: one category (fixed list), any keyword. Sort: Relevance (with a
  query), Popular (installs), Recently updated, Newest.
- Paging: keyset cursors (`cursor`, `limit` ≤ 50) on a stable sort key. The
  app loads more as you scroll (virtualized); the web has Next / Previous
  links that carry the cursor, so pages are real URLs.
- Installs are counted once per account per plugin; only the total shows.

### In the app
- Settings → Plugins gains a **Browse** tab beside Installed and Yours (the
  gallery mock `ui/demos/example_plugins.luau` already imagines it), built
  on `ds.Settings.Catalog`. A plugin opens as a sub-page: README (native
  markdown, pictures from the package, lightbox), what it asks for,
  versions and changelog, Install / Update / Open settings.
- Installing from the store is a new source kind `store` in the engine; the
  package is verified exactly as a file is, then the same consent window.
- Yours gains Publish… (checks, then submit), each version's review state,
  and deploy keys.
- `zeron://open/plugin/<id>` opens that plugin's page with Install ready.
  It only ever names a store id; it never installs by itself.

### Free-form settings pages
- `zeron.settings_page { build = function(page) … end, keywords = {…} }`:
  the plugin draws its page, in its own VM, under the header Zeron draws
  (icon, name, description, Unverified badge). Declared `settings` rows
  still work and are drawn first; a page may also set its own declared
  settings, so a custom control and a declared row stay one value.
- Guidance, all four: a "Settings pages" section in `docs/plugins.md` with
  the rules and a model page; the agents' guide summary names it; the
  scaffold can start one; `plugin_status { snapshot = true }` can capture
  the settings page so an agent sees what it drew. A new example,
  `plugins-dev/settings-page`, is the page to copy.

## Contract (edge HTTP)

Base: the edge host. JSON unless said. Errors are `{ error: "<word>",
message }` with stable words. Times are ms since the epoch.

```
Public (no sign-in; cached; rate limited)
GET  /store/v1/plugins?q&category&keyword&sort&cursor&limit
       → { items: PluginCard[], next_cursor? }
GET  /store/v1/plugins/:id              → PluginDetail
GET  /store/v1/plugins/:id/versions     → { versions: VersionInfo[] }
GET  /store/v1/plugins/:id/versions/:version/package   → the .zeron-plugin
       (approved: public, immutable; pending: its owner or an admin)
GET  /store/v1/plugins/:id/files/:version/*path        → a packaged file
       (pictures and the icon publicly; any file for owner or admin)
GET  /store/v1/categories               → { categories: [{ id, name }] }
POST /store/v1/updates  { zeron, plugins: [{ id, version, hash, prerelease? }] }
       → { updates: [{ id, version, hash, notes, added, removed, in_review? }],
           blocked: [{ id, version, reason }] }
       (with a session, the caller's own pending versions are offered)

Publisher (session bearer, or a deploy key's signed request)
POST /store/v1/plugins/:id/versions     body: the .zeron-plugin
       → 201 { id, version, status: "pending", checks }
       errors: needs_username, id_taken, id_reserved, not_semver,
       version_exists, not_newer, bad_package, bad_signature,
       key_not_allowed, missing_readme, missing_listing, too_large,
       rate_limited, suspended
GET  /store/v1/me/plugins               → the caller's plugins, every version
POST /store/v1/plugins/:id/versions/:version/{withdraw|yank|unyank}
GET  /store/v1/plugins/:id/keys         → deploy keys (never the secret)
POST /store/v1/plugins/:id/keys { key, name, expires?, proof, stepUp }
DELETE /store/v1/plugins/:id/keys/:key
POST /store/v1/plugins/:id/installed    (counts once per account)
POST /store/v1/plugins/:id/reports { reason, text }

Admin (web session or bearer; the account must hold `admin`)
GET  /store/v1/admin/queue?status&cursor
GET  /store/v1/admin/submissions/:id/:version      (with its diff base)
POST /store/v1/admin/submissions/:id/:version/approve
POST /store/v1/admin/submissions/:id/:version/reject { reason, message }
POST /store/v1/admin/plugins/:id/{suspend|restore|feature|transfer}
POST /store/v1/admin/plugins/:id/versions/:version/remove { block, reason }
GET  /store/v1/admin/reports   POST /store/v1/admin/reports/:id/resolve
GET  /store/v1/admin/audit?cursor

Web session (zeron.sh)
GET /auth/web/login?return_to   GET /auth/web/callback
POST /auth/web/logout           GET /auth/web/session → { user, username, roles }

Operator (ADMIN_TOKEN)
GET /admin/roles   POST /admin/roles/grant   POST /admin/roles/revoke
```

Deploy-key request signature: header
`Authorization: ZeronKey key=<b64url>, ts=<ms>, sig=<b64url>`, where `sig`
signs `"zeron-plugin-publish:v1\n" + method + "\n" + path + "\n" +
sha256hex(body) + "\n" + ts`. Five minutes of clock skew at most.

Shapes (lanes fill in fields; these names are fixed):
- `PluginCard`: `id, name, summary, icon, author { username, verified,
  badges }, categories, keywords, version, updated_at, installs, featured`.
- `PluginDetail`: `PluginCard` + `readme` (markdown), `screenshots`,
  `permissions` (as the consent sheet groups them), `engines`, `homepage`,
  `repository`, `license`, `versions` (latest few), `package { bytes,
  sha256, hash }`.
- `VersionInfo`: `version, status, published_at, notes, engines, yanked`.

Edge principals (the seam between the two edge lanes), in
`edge/src/store/principal.ts`:
```ts
type Publisher = { userId: string; username: string | null;
                   via: "session" | "deploy_key"; key?: string; pluginScope?: string };
authenticatePublisher(req, env): Promise<Publisher | Response>
requireAdmin(req, env): Promise<{ userId: string } | Response>
optionalUser(req, env): Promise<{ userId: string } | null>
```

Package verification is implemented twice (Rust in the engine, TypeScript
at the edge) and held together by shared vectors in
`docs/plugin-package-vectors.json`, generated by the Rust side and run by
both test suites: canonical JSON, content hash, signature, path rules,
limits, strict semver ordering.

## Lanes

| Lane | Owns | Branch |
|---|---|---|
| Edge: store | D1 schema and migrations, R2, package verification, publish, catalog, search, versions, updates, review state machine, admin routes, counts, reports, audit, cache, limits | `plugin-store/edge-store` |
| Edge: identity | web sign-in session, `admin` role and operator routes, deploy keys, signed-request auth, `principal.ts` | `plugin-store/edge-identity` |
| Engine and CLI | manifest additions, strict semver for publishing, `store` source (install, batched updates, blocks, own pending), publish from the engine, deploy keys, standalone `zeron plugin publish` for CI, the GitHub Action, MCP tools, shared vectors, docs | `plugin-store/engine` |
| App: store | Browse, plugin page, install and update from the store, Publish and review state in Yours, deploy keys UI, `zeron://open/plugin/<id>` | `plugin-store/app-store` |
| App: settings pages | `zeron.settings_page`, hosting, search, guidance, example, tests | `plugin-store/app-settings` |
| Web | Astro server routes, `/plugins`, `/plugins/<id>`, README rendering, "Install in Zeron", account pages (my plugins, keys), the admin dashboard, PR CI for the site | `plugin-store/web` |

Then, with everything merged: an end-to-end run on staging (publish →
pending → author's second device installs → approve → listed → web link
installs → update → yank → takedown), an adversarial security review, a
design review of every screen, and the rollout (D1, R2, WorkOS redirect
addresses, deploy, grant the first admin).

## Rules for every lane

- No shortcuts. The idiomatic, correct way, tested. If the right way needs
  something another lane owns, ask the coordinator; do not work around it.
- Fixes and additions never change how existing things look or behave
  unless this plan says so. Sideloading keeps working exactly as today.
- Words people say. No "Zeron's own". No plumbing words in the UI.
- Design: Zeron's components and tokens only; concentric corners; align
  text, not boxes; look at pixels (inspect snapshots), not only boxes.
- Unannounced feature in a public repository: neutral commit messages, no
  screenshots committed, no staging hostnames or account names.
- Tests with every piece: edge unit + workerd, Rust unit + integration,
  `zgx test ui` with a fixture per screen, site tests on the built pages.
- Report to the coordinator: what is done, what is proven and how, what is
  open. Do not stop on a question that has a safe default: take it, say so.
