// The fake edge: the store's HTTP contract and the web sign-in, over the
// fixtures, in memory (a restart starts over). It behaves as the plan says
// the edge does where the site can tell: who may see what, CSRF and Origin
// on every write, keyset cursors, error words. Its sign-in is a page of the
// fixtures' accounts in place of WorkOS.

import { SITE_IDS } from "../src/lib/store/names.ts";
import { moreTemplates, templates } from "./templates.ts";
import { PNG_OF, pngBytes } from "./png.ts";
import { makeToken, tokenHash, tokenId, TOKEN_ID } from "./token.ts";
import { audit, CATEGORIES, DAY, tokens, MANY_IDS, manyMore, NOW, plugins, reports, startClock, USERS, type FakePlugin, type FakeUser, type FakeVersion } from "./fixtures.ts";

const COOKIE = "__Host-zeron_session";

/** The fake's state, made on its first request (its clock, the fixtures' "now", starts then). */
const freshState = () => ({
  // (Both kinds, one set of ids: each public route reads its own kind.)
  plugins: [...plugins(), ...templates()],
  reports: reports(),
  audit: audit(),
  tokens: tokens(),
  reservations: [
    { id: "zeron-tools", user: "user_grace", note: "For the team's own plugins.", created_at: NOW - 40 * DAY },
    { id: "linear", user: null, note: "Held until Linear asks for it.", created_at: NOW - 12 * DAY },
  ] as { id: string; user: string | null; note: string | null; created_at: number }[],
  /** session id → user id, csrf */
  /** `rotated`: a staff session already replaced on its first privileged use. */
  /** `fresh_until`: the step-up ticket a sign-in earned (spent by making a deploy token), good until then. */
  sessions: new Map<string, { user: string; csrf: string; rotated?: boolean; fresh_until?: number | null }>(),
  /** The queue as a screenshot asks for it (POST /_fake/queue, astro dev's /_dev/queue): empty, with every staff list, or this long. Null: as it is. */
  queueAs: null as null | "empty" | number,
  /** A version answered with another's record (POST /_fake/loose?on=1): a page must say only what the record names. */
  loose: false,
  /** The store as if it were down (POST /_fake/down?on=1): every /store/v1 read and write a 503. */
  down: false,
  /** A plugin shown with no pictures (POST /_fake/bare?id=…, empty for none): Featured with one that has none. */
  bare: null as string | null,
  /** The store with nothing listed (POST /_fake/empty?on=1): a page's emptiest. */
  empty: false,
});
let state = undefined as unknown as ReturnType<typeof freshState>;

// Programs the host rates (crates/plugin-guard/src/commands.rs), enough for the fixtures.
const CONTAINED = new Set(["cat", "date", "jq", "ls", "uname", "wc", "echo", "grep", "head", "tail"]);
const FULL_CONTROL = new Set(["bash", "sh", "zsh", "fish", "node", "python", "python3", "ruby", "perl", "deno", "bun", "pwsh", "powershell", "osascript", "lua"]);
const isLocal = (host: string) => /^(localhost|127\.|10\.|192\.168\.|\[?::1\]?)/.test(host) || /\.(local|internal|lan|ts\.net)(:\d+)?$/.test(host);

// ── Responses ────────────────────────────────────────────────────────────

const json = (body: unknown, status = 200, headers: HeadersInit = {}) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", ...headers } });
const fail = (status: number, error: string, message?: string) => json(message ? { error, message } : { error }, status);
const notFound = () => fail(404, "not_found");

// ── Who's asking ─────────────────────────────────────────────────────────

function cookie(request: Request, name: string): string | null {
  for (const part of (request.headers.get("cookie") ?? "").split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=");
  }
  return null;
}

function session(request: Request): { id: string; user: FakeUser; csrf: string } | null {
  const id = cookie(request, COOKIE);
  const found = id ? state.sessions.get(id) : undefined;
  const user = found && USERS.find((u) => u.id === found.user);
  return id && found && user ? { id, user, csrf: found.csrf } : null;
}

/** A write: signed in, from the site's own origin, with the session's token. */
function writer(request: Request): { user: FakeUser } | Response {
  const s = session(request);
  if (!s) return fail(401, "signed_out");
  const origin = request.headers.get("origin");
  if (!origin || origin !== new URL(request.url).origin) return fail(403, "bad_csrf", "This request didn't come from the site.");
  if (request.headers.get("x-csrf-token") !== s.csrf) return fail(403, "bad_csrf");
  return { user: s.user };
}

const isAdmin = (user: FakeUser) => user.roles.includes("admin");
const userOf = (id: string) => USERS.find((u) => u.id === id)!;

// ── The store's views of a plugin ───────────────────────────────────────

type Kind = "plugin" | "template";
const kindOf = (p: FakePlugin): Kind => p.kind ?? "plugin";
/** An item by its id, of this kind: the other kind's id isn't there. */
const itemOf = (kind: Kind, id: string) => state.plugins.find((x) => x.id === id && kindOf(x) === kind);

const prerelease = (v: FakeVersion) => v.version.includes("-");
/** What the public sees of a plugin's versions: listed and yanked (amendment 45). */
const shown = (v: FakeVersion) => v.status === "listed" || v.status === "yanked";
/** The newest listed version, a release before a pre-release, verified or not (amendment 45). */
const latest = (p: FakePlugin) => p.versions.find((v) => v.status === "listed" && !prerelease(v)) ?? p.versions.find((v) => v.status === "listed");
/** The newest verified listed version, a release before a pre-release. */
const verifiedVersion = (p: FakePlugin) => p.versions.find((v) => v.status === "listed" && v.verified && !prerelease(v)) ?? p.versions.find((v) => v.status === "listed" && v.verified);
const listed = (p: FakePlugin) => !state.empty && p.status !== "suspended" && latest(p) !== undefined;
/** When it was first listed (amendment 39): its first version's day, never moved by a later one; null with no listed version. */
const listedAt = (p: FakePlugin) => (latest(p) ? Math.min(...p.versions.filter(shown).map((v) => v.published_at)) : null);

const author = (userId: string) => {
  const u = userOf(userId);
  return { id: `pub_${hash(u.id).slice(0, 32)}`, username: u.username, badges: u.badges };
};

function card(p: FakePlugin, version: FakeVersion | undefined = latest(p)) {
  const listedVersion = latest(p);
  const v = version ?? p.versions[0]!;
  const m = v.manifest;
  return {
    kind: kindOf(p),
    id: p.id,
    name: String(m.name),
    summary: String(m.description),
    icon: listedVersion && typeof m.icon === "string" ? m.icon : null,
    // (Its first picture's path in the package, of the version it shows; null when that has none: the edge's
    // card, as asked of it.)
    cover: p.kind === "template" ? (listedVersion ? "preview.png" : null) : listedVersion && state.bare !== p.id ? (((listedVersion.manifest.screenshots as string[] | undefined) ?? [])[0] ?? null) : null,
    author: author(p.owner),
    categories: (m.categories as string[]) ?? [],
    keywords: (m.keywords as string[]) ?? [],
    version: listedVersion?.version ?? null,
    verified: listedVersion?.verified ?? false,
    verified_version: verifiedVersion(p)?.version ?? null,
    verified_at: verifiedVersion(p)?.verified_at ?? null,
    updated_at: (listedVersion ?? v).published_at,
    listed_at: listedAt(p),
    installs: p.installs,
    featured: p.featured,
  };
}

function permissions(m: Record<string, unknown>) {
  const commands = (m.commands as string[] | undefined) ?? [];
  const hosts = ((m.network as { hosts?: string[] } | undefined)?.hosts) ?? [];
  const tools = ((m.tools as { name: string; description?: string }[] | undefined) ?? []).map(({ name, description }) => ({ name, description }));
  const ui = m.ui as { change?: string[]; replace?: string[] } | undefined;
  return {
    asks: (m.permissions as string[]) ?? [],
    ...(ui ? { ui } : {}),
    commands,
    commands_anything: commands.filter((c) => !CONTAINED.has(c)),
    commands_full_control: commands.filter((c) => FULL_CONTROL.has(c)),
    network_hosts: hosts,
    network_local: hosts.filter(isLocal),
    network_named: hosts.filter((h) => h !== "*" && !isLocal(h) && !/^[\d.:[\]]+$/.test(h)),
    tools,
  };
}

function versionInfo(v: FakeVersion, full: boolean) {
  const bytes = Object.values(v.files).reduce((n, f) => n + f.length, 0);
  return {
    version: v.version,
    status: v.status,
    published_at: v.published_at,
    verified: v.verified,
    verified_at: v.verified_at,
    via: v.via,
    notes: v.notes,
    engines: v.engines,
    yanked: v.status === "yanked",
    // (Why it takes no new installs, in public too: the contract after e3daa69b4.)
    yanked_by: v.status === "yanked" ? v.yanked_by : null,
    superseded_by: v.status === "yanked" && v.yanked_by === "superseded" ? (v.superseded?.by ?? null) : null,
    prerelease: v.version.includes("-"),
    // (A device's key signs a version; a deploy token's version is sent unsigned, and names no key.)
    key: v.via === "deploy_token" ? null : "k5Yh3xQ2b8VtYwGm1rPz0nLc4dEa6sJu9oKi7fBg2Wq",
    hash: `b3:${hash(v.version + bytes)}`,
    sha256: hash(JSON.stringify(v.manifest)).slice(0, 64),
    bytes,
    // (The owner's and staff's: the reviewer's decision, the token that published it, why staff stopped it; never the public's.)
    ...(full
      ? {
          review: v.review,
          checks: v.checks,
          blocked: v.blocked === true,
          deploy_token: v.deploy_token,
          superseded: v.superseded,
          ...(v.removal ? { removal: v.removal } : {}),
        }
      : {}),
  };
}

function detail(p: FakePlugin) {
  const v = latest(p)!;
  const m = v.manifest;
  const bytes = Object.values(v.files).reduce((n, f) => n + f.length, 0);
  return {
    ...card(p, v),
    readme: v.files["README.md"] ?? "",
    screenshots: state.bare === p.id ? [] : ((m.screenshots as string[]) ?? []),
    // (A template asks for nothing, and has no such key.)
    ...(p.kind === "template" ? {} : { permissions: permissions(m) }),
    engines: v.engines,
    homepage: (m.homepage as string) ?? null,
    repository: (m.repository as string) ?? null,
    license: (m.license as string) ?? null,
    // (The first page of its versions; the versions route has the rest.)
    versions: p.versions.filter(shown).slice(0, 5).map((x) => versionInfo(x, false)),
    ...(p.versions.filter(shown).length > 5 ? { versions_next_cursor: btoa("5").replace(/=+$/, "") } : {}),
    package: { bytes, sha256: "9f2c1e4b7a0d3c6f8e1b4a7d0c3f6e9b2a5d8c1f4e7b0a3d6c9f2e5b8a1d4c7f", hash: "b3:7d1e4a9c2f5b8e1d4a7c0f3b6e9d2a5c", key: "k5Yh3xQ2b8VtYwGm1rPz0nLc4dEa6sJu9oKi7fBg2Wq" },
  };
}

// ── The catalog ──────────────────────────────────────────────────────────

type Key = [number, string];
const encode = (value: unknown) => btoa(JSON.stringify(value)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
function decode(cursor: string): { dir: "after" | "before"; key: Key; sort: string; q: string } | null {
  try {
    const value = JSON.parse(atob(cursor.replace(/-/g, "+").replace(/_/g, "/")));
    return Array.isArray(value?.key) ? value : null;
  } catch {
    return null;
  }
}

function score(p: FakePlugin, terms: string[]): number {
  const c = card(p);
  const fields: [string, number][] = [
    [c.name.toLowerCase(), 5],
    [c.summary.toLowerCase(), 3],
    [c.keywords.join(" ").toLowerCase(), 3],
    [c.author.username ?? "", 2],
    [(latest(p)!.files["README.md"] ?? "").toLowerCase(), 1],
  ];
  let total = 0;
  for (const term of terms) {
    let hit = 0;
    for (const [text, weight] of fields) if (text.split(/[^a-z0-9]+/).some((w) => w.startsWith(term))) hit = Math.max(hit, weight);
    if (!hit) return 0;
    total += hit;
  }
  return total;
}

function catalog(url: URL, kind: Kind = "plugin"): Response {
  const q = (url.searchParams.get("q") ?? "").trim().toLowerCase().slice(0, 100);
  const terms = q.split(/[^a-z0-9]+/).filter(Boolean);
  const category = url.searchParams.get("category");
  const keyword = url.searchParams.get("keyword")?.toLowerCase();
  const featured = url.searchParams.get("featured") === "true";
  // (A filter, never a sort: amendment 45 and the coordinator's ruling.)
  const verified = url.searchParams.get("verified") === "1";
  const authorName = url.searchParams.get("author");
  let sort = url.searchParams.get("sort") ?? (terms.length ? "relevance" : "popular");
  if (!["relevance", "popular", "updated", "new", "verified"].includes(sort)) return fail(400, "bad_request", "Unknown sort.");
  if (sort === "relevance" && !terms.length) sort = "popular";
  if (category && !CATEGORIES.some((c) => c.id === category)) return fail(400, "bad_request", "Unknown category.");
  const limit = Math.min(50, Math.max(1, Number(url.searchParams.get("limit") ?? 24) || 24));

  const keyOf = (p: FakePlugin): Key => {
    const value = sort === "relevance" ? score(p, terms) : sort === "popular" ? p.installs : sort === "updated" ? latest(p)!.published_at : sort === "verified" ? (verifiedVersion(p)?.verified_at ?? 0) : listedAt(p)!;
    return [value, p.id];
  };
  // Descending by value, then ascending by id: a stable total order.
  const before = (a: Key, b: Key) => a[0] > b[0] || (a[0] === b[0] && a[1] < b[1]);

  const all = state.plugins
    .filter((p) => kindOf(p) === kind)
    .filter(listed)
    // (Featured holds only plugins whose newest version is verified.)
    .filter((p) => !featured || (p.featured && latest(p)!.verified))
    .filter((p) => !verified || latest(p)!.verified)
    // (Newly verified: only plugins with a verified listed version, at its time: contract ee380cca3.)
    .filter((p) => sort !== "verified" || verifiedVersion(p) !== undefined)
    .filter((p) => !authorName || userOf(p.owner).username === authorName)
    .filter((p) => !category || card(p).categories.includes(category))
    .filter((p) => !keyword || card(p).keywords.some((k) => k.toLowerCase() === keyword))
    .filter((p) => !terms.length || score(p, terms) > 0)
    .map((p) => ({ p, key: keyOf(p) }))
    .sort((a, b) => (before(a.key, b.key) ? -1 : before(b.key, a.key) ? 1 : 0));

  const cursorText = url.searchParams.get("cursor");
  let start = 0;
  if (cursorText) {
    const cursor = decode(cursorText);
    if (!cursor || cursor.sort !== sort || cursor.q !== q) return fail(400, "bad_request", "That page's cursor doesn't fit this search.");
    if (cursor.dir === "after") {
      start = all.findIndex((x) => before(cursor.key, x.key));
      if (start === -1) start = all.length;
    } else {
      const end = all.findIndex((x) => !before(x.key, cursor.key));
      start = Math.max(0, (end === -1 ? all.length : end) - limit);
    }
  }
  const page = all.slice(start, start + limit);
  // (total: what the query matches, every page of it: amendment 38.)
  const body: { items: unknown[]; total: number; next_cursor?: string; prev_cursor?: string } = { items: page.map((x) => card(x.p)), total: all.length };
  if (start + limit < all.length && page.length) body.next_cursor = encode({ dir: "after", key: page.at(-1)!.key, sort, q });
  if (start > 0 && page.length) body.prev_cursor = encode({ dir: "before", key: page[0]!.key, sort, q });
  return json(body, 200, { "cache-control": "public, max-age=30" });
}

// ── Files ────────────────────────────────────────────────────────────────

const TYPES: Record<string, string> = { svg: "image/svg+xml", png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", avif: "image/avif", md: "text/markdown; charset=utf-8", json: "application/json", luau: "text/plain; charset=utf-8" };

async function file(request: Request, kind: Kind, id: string, version: string, path: string): Promise<Response> {
  const p = itemOf(kind, id);
  const v = p?.versions.find((x) => x.version === version);
  if (!p || !v || !(path in v.files)) return notFound();
  const ext = path.split(".").pop()!.toLowerCase();
  // A picture is shown inline only when its bytes are that picture
  // (amendment 14); anything else is given as a download.
  const body = v.files[path]!;
  const MAGIC: Record<string, RegExp> = { png: /^\x89PNG/, jpg: /^\xff\xd8/, jpeg: /^\xff\xd8/, gif: /^GIF8[79]a/, webp: /^RIFF/, avif: /^....ftyp/, svg: /^\s*<svg[\s>]/ };
  const picture = Object.hasOwn(MAGIC, ext) && MAGIC[ext]!.test(body);
  const open = listed(p) && shown(v) && picture;
  const s = session(request);
  const allowed = open || (s && (s.user.id === p.owner || isAdmin(s.user)));
  if (!allowed) return notFound();
  return new Response(body.startsWith(PNG_OF) ? await pngBytes(body) : body, {
    headers: {
      "content-type": picture ? TYPES[ext]! : "application/octet-stream",
      "content-disposition": picture ? "inline" : "attachment",
      "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; sandbox",
      "x-content-type-options": "nosniff",
      // (The edge checks the version's state before it serves a file, and
      // keeps that state at most a minute: amendment 23.)
      "cache-control": open ? "public, max-age=60" : "private, no-store",
    },
  });
}

// ── The author's own ─────────────────────────────────────────────────────

function mine(user: FakeUser, kind: Kind = "plugin") {
  return state.plugins
    .filter((p) => p.owner === user.id && kindOf(p) === kind)
    // (Each version's publisher_id is the account that published it: here always its owner, nothing being transferred in.)
    .map((p) => ({ ...card(p), listing: p.status === "suspended" ? "suspended" : listed(p) ? "listed" : "unlisted", versions: p.versions.map((v) => ({ ...versionInfo(v, true), publisher_id: publisherOf(p.owner) })) }));
}

/** An account's opaque publisher id (amendment 33), as the edge spells one. */
const publisherOf = (userId: string) => `pub_${[...userId].map((c) => c.charCodeAt(0).toString(16)).join("").padEnd(32, "0").slice(0, 32)}`;

function versionAction(request: Request, id: string, version: string, action: string): Response {
  const who = writer(request);
  if (who instanceof Response) return who;
  const p = state.plugins.find((x) => x.id === id && x.owner === who.user.id);
  const v = p?.versions.find((x) => x.version === version);
  if (!p || !v) return notFound();
  // (Amendment 45: withdraw goes; a version is listed at once, and an author can stop and allow new installs.)
  const from: Record<string, FakeVersion["status"]> = { yank: "listed", unyank: "yanked" };
  const to: Record<string, FakeVersion["status"]> = { yank: "yanked", unyank: "listed" };
  if (!(action in from)) return notFound();
  if (v.status !== from[action]) return fail(409, "conflict", `It's ${v.status}.`);
  // (A superseded version is the store's to stop, for good: verifying it allows installs again, its author can't.)
  if (action === "unyank" && v.yanked_by === "superseded") return fail(409, "invalid_state", "It was superseded.");
  v.yanked_by = action === "yank" ? "author" : null;
  v.status = to[action]!;
  return json(versionInfo(v, true));
}

// ── Admin ────────────────────────────────────────────────────────────────

function admin(request: Request): { user: FakeUser } | Response {
  const s = session(request);
  if (!s) return fail(401, "signed_out");
  if (!isAdmin(s.user)) return fail(403, "forbidden");
  return { user: s.user };
}

/**
 * A staff session is replaced on its first privileged use, as the edge's is
 * (edge/src/web-session.ts; it rotates again every 10 minutes of use, which
 * no test here lasts): the new token goes out on that answer, the old one
 * stays good (for the edge, a minute; here, for the fake's life). Nobody
 * else's ever rotates.
 */
function rotate(request: Request, response: Response): Response {
  const s = session(request);
  const found = s && state.sessions.get(s.id);
  if (!s || !found || found.rotated || !isAdmin(s.user)) return response;
  found.rotated = true;
  const next = crypto.randomUUID();
  state.sessions.set(next, { user: s.user.id, csrf: s.csrf, rotated: true, fresh_until: found.fresh_until ?? null });
  const headers = new Headers(response.headers);
  headers.append("set-cookie", `${COOKIE}=${next}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=1800`);
  return new Response(response.body, { status: response.status, headers });
}

function adminWriter(request: Request): { user: FakeUser } | Response {
  const who = writer(request);
  if (who instanceof Response) return who;
  return isAdmin(who.user) ? who : fail(403, "forbidden");
}

const person = (id: string) => ({ user_id: id, username: userOf(id).username });

function log(actor: FakeUser, action: string, plugin: string | null, version: string | null, detail: Record<string, unknown> | null) {
  state.audit.unshift({ seq: (state.audit[0]?.seq ?? 0) + 1, at: Date.now(), actor: actor.id, action, plugin_id: plugin, version, detail });
}

/** The newest verified other version (amendment 45): what a version is weighed against. */
const baseOf = (p: FakePlugin, v: FakeVersion) => p.versions.find((x) => x !== v && x.verified && shown(x)) ?? null;

/**
 * The staff queue (amendment 45): versions to verify (no reviewer has
 * decided), or those verified, or declined, or all; one row per plugin in
 * the undecided view (its newest, standing for its earlier undecided ones);
 * oldest first, plugins with open reports first.
 */
function queue(url: URL): Response {
  const view = url.searchParams.get("state") ?? "undecided";
  if (!["undecided", "verified", "declined", "all", "taken_down"].includes(view)) return fail(400, "bad_request", "Unknown state.");
  // (Undecided: listed, not yanked by the superseding rule; nothing that leaves the queue unread can be installed.
  // Taken down: those not restored, edge 566c2a5ba.)
  const fits = (v: FakeVersion) =>
    view === "taken_down"
      ? v.status === "taken_down"
      : shown(v) && (view === "all" || (view === "undecided" ? !v.review.decided && v.yanked_by !== "superseded" : view === "verified" ? v.verified : v.review.decision === "declined"));
  const row = (p: FakePlugin, v: FakeVersion, earlier: number) => {
    const base = baseOf(p, v);
    const asked = (v.manifest.permissions as string[]) ?? [];
    const had = (base?.manifest.permissions as string[]) ?? [];
    // (Amendment 40: inside an ask both have, a host, program, place or tool's words differ.)
    const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
    const now = permissions(v.manifest);
    const was = base ? permissions(base.manifest) : null;
    const sorted = (list: string[]) => [...list].sort();
    const bound = base ? binds(base.manifest, v.manifest) : null;
    const asksChanged = was !== null && (!same(sorted(now.commands), sorted(was.commands)) || !same(sorted(now.network_hosts), sorted(was.network_hosts)) || !same(now.tools, was.tools) || !same(now.ui ?? null, was.ui ?? null) || bound!.mcp_changed !== null || bound!.service_changed !== null || bound!.settings_changed);
    return {
      plugin: { id: p.id, name: String(v.manifest.name) },
      version: v.version,
      icon: typeof v.manifest.icon === "string" ? v.manifest.icon : null,
      asks_changed: asksChanged,
      status: v.status,
      review: v.review,
      reports: state.reports.filter((r) => r.plugin_id === p.id && r.status === "open").length,
      earlier_undecided: earlier,
      prerelease: prerelease(v),
      submitted_at: v.published_at,
      published_at: v.published_at,
      publisher: { id: author(p.owner).id, username: userOf(p.owner).username },
      // (The plugin's installs, in every state: edge 566c2a5ba.)
      installs: p.installs,
      ...(base ? { base: base.version } : {}),
      new_asks: asked.filter((a) => !had.includes(a)),
      checks: { ok: v.checks.filter((c) => c.level === "ok").length, warn: v.checks.filter((c) => c.level === "warn").length, fail: v.checks.filter((c) => c.level === "fail").length },
    };
  };
  // (How many earlier undecided versions a row superseded.)
  const earlierOf = (p: FakePlugin, v: FakeVersion) => p.versions.filter((x) => x.superseded?.by === v.version && !x.review.decided).length;
  // (Templates join the queue with the review pages that draw them.)
  const items = state.plugins.filter((p) => kindOf(p) === "plugin").flatMap((p) => {
    const matching = p.versions.filter(fits);
    if (view !== "undecided") return matching.map((v) => row(p, v, earlierOf(p, v)));
    // At most two a plugin: its newest undecided release, and its newest undecided pre-release when that is newer.
    const release = matching.find((v) => !prerelease(v));
    const pre = matching.find((v) => prerelease(v));
    const rows = [release, pre && (!release || pre.published_at > release.published_at) ? pre : undefined].filter((v): v is FakeVersion => v !== undefined);
    return rows.map((v) => row(p, v, earlierOf(p, v)));
  });
  if (state.queueAs === "empty") items.length = 0;
  if (view === "undecided" && typeof state.queueAs === "number") items.push(...filler(state.queueAs - items.length));
  // Plugins with open reports first; then oldest waiting first. (Taken down: the newest take-down first.)
  const downAt = (item: { plugin: { id: string }; version: string }) => state.plugins.find((p) => p.id === item.plugin.id)?.versions.find((v) => v.version === item.version)?.removal?.at ?? 0;
  if (view === "taken_down") items.sort((a, b) => downAt(b) - downAt(a));
  else items.sort((a, b) => Number(b.reports > 0) - Number(a.reports > 0) || a.submitted_at - b.submitted_at);
  return json(paged(items, url, "items"));
}

/**
 * Amendment 41, as the edge has it (capability.rs Diff): the MCP server and
 * the service, before and after when they differ, and whether a setting
 * that grants (a permission or a folder) is new or different.
 */
function binds(was: Record<string, unknown> | null, now: Record<string, unknown>) {
  const mcp = (m: Record<string, unknown> | null) => {
    const spec = m?.mcp as { command?: string; args?: string[]; env?: Record<string, string> } | undefined;
    return spec?.command ? { command: spec.command, args: spec.args ?? [], env: Object.fromEntries(Object.entries(spec.env ?? {}).sort(([a], [b]) => (a < b ? -1 : 1))) } : null;
  };
  const service = (m: Record<string, unknown> | null) => (typeof m?.service === "string" ? m.service : null);
  const granting = (m: Record<string, unknown> | null) => ((m?.settings as { kind?: string }[] | undefined) ?? []).filter((x) => x.kind === "permission" || x.kind === "folder").map((x) => JSON.stringify(x));
  const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
  const before = granting(was);
  return {
    mcp_changed: same(mcp(was), mcp(now)) ? null : { before: mcp(was), after: mcp(now) },
    service_changed: same(service(was), service(now)) ? null : { before: service(was), after: service(now) },
    settings_changed: granting(now).some((x) => !before.includes(x)),
  };
}

/** Versions that fill a long queue for its picture: made up, each a plausible one (none opens). */
function filler(n: number) {
  const NAMES = ["Commit coach", "Diff digest", "Branch tidy", "Standup bot", "Tab groups", "Prompt shelf", "Log lens", "Issue triage", "Cron watch", "Snippet box", "Focus music", "Daily notes", "Review buddy", "Docker peek", "Env check", "Port finder", "Token budget", "Release notes", "Sprint board", "Lint on save", "Model picker", "Chat export", "Quiet hours", "Time zones", "PR labels", "Test runner", "Changelog bot", "Rate limits", "Spell check", "Code owners", "Weekly recap", "Icon pack", "Pair mode", "Night owl", "Ticket links", "Build badge", "Gist share", "Error lens", "Merge queue", "Inbox zero"];
  const PEOPLE = ["ada", "kai", "mo-dev", "noor", null];
  return Array.from({ length: Math.max(0, n) }, (_, i) => {
    const name = NAMES[i % NAMES.length]!;
    const update = i % 3 !== 0;
    const username = PEOPLE[i % PEOPLE.length]!;
    return {
      plugin: { id: name.toLowerCase().replace(/ /g, "-"), name },
      version: update ? `1.${(i % 4) + 1}.${i % 3}` : "0.1.0",
      status: "listed" as const,
      review: { decided: false, decision: null },
      reports: i % 11 === 4 ? 1 : 0,
      earlier_undecided: i % 6 === 5 ? 2 : 0,
      prerelease: false,
      submitted_at: NOW - (i * 5.5 + 1) * 3_600_000,
      published_at: NOW - (i * 5.5 + 1) * 3_600_000,
      publisher: { id: `pub_${i}`, username },
      installs: (i * 137) % 5000,
      ...(update ? { base: `1.${i % 4}.0` } : {}),
      icon: null,
      new_asks: update && i % 5 === 1 ? (i % 2 ? ["commands"] : ["network"]) : [],
      asks_changed: update && i % 7 === 3,
      checks: { ok: 6, warn: i % 4 === 0 ? 1 : 0, fail: 0 },
    };
  });
}

function submission(id: string, version: string): Response {
  const p = state.plugins.find((x) => x.id === id);
  const v = p?.versions.find((x) => x.version === version);
  if (!p || !v) return notFound();
  const base = baseOf(p, v);
  const files = (fv: FakeVersion) => Object.entries(fv.files).map(([path, text]) => ({ path, bytes: text.length, sha256: hash(text), binary: false }));
  const owner = userOf(p.owner);
  const theirs = state.plugins.filter((x) => x.owner === p.owner).flatMap((x) => x.versions.map((y) => ({ plugin: x, v: y })));
  const permissionsNow = permissions(v.manifest);
  const permissionsBase = base ? permissions(base.manifest) : null;
  const added = <T,>(now: T[], then: T[] | undefined) => now.filter((x) => !(then ?? []).includes(x));
  const m = v.manifest;
  return json({
    plugin: card(p, v),
    version: { ...versionInfo(v, true), name: String(m.name), summary: String(m.description), icon: typeof m.icon === "string" ? m.icon : null, readme: v.files["README.md"] ?? "", screenshots: (m.screenshots as string[]) ?? [] },
    superseded: v.superseded,
    publisher: {
      id: author(p.owner).id,
      username: owner.username,
      created_at: owner.created_at,
      // (When the account first listed a version, and its listed plugins, how many with a verified version: edge 566c2a5ba.)
      first_published_at: theirs.length ? Math.min(...theirs.map(({ v: y }) => y.published_at)) : null,
      plugins: { total: state.plugins.filter((x) => x.owner === p.owner && listed(x)).length, verified: state.plugins.filter((x) => x.owner === p.owner && listed(x) && x.versions.some((y) => y.verified)).length },
      decisions: { verified: theirs.filter(({ v: y }) => y.review.decision === "verified").length, declined: theirs.filter(({ v: y }) => y.review.decision === "declined").length },
      recent: theirs
        .map(({ plugin: x, v: y }) => ({ plugin_id: x.id, version: y.version, status: y.status, verified: y.verified, published_at: y.published_at }))
        .sort((a, b) => b.published_at - a.published_at)
        .slice(0, 10),
    },
    manifest: v.manifest,
    permissions: permissionsNow,
    files: files(v),
    base: base ? { version: base.version, manifest: base.manifest } : null,
    base_files: base ? files(base) : [],
    // (The edge's consentDiff, against the newest verified other version, or nothing.)
    permissions_diff: (() => {
      const was = permissionsBase ?? { asks: [] as string[], commands: [] as string[], network_hosts: [] as string[], tools: [] as { name: string }[], ui: undefined };
      const ui = (x: { ui?: { change?: string[]; replace?: string[] } }) => ({ change: x.ui?.change ?? [], replace: x.ui?.replace ?? [] });
      return {
        added: added(permissionsNow.asks, was.asks),
        removed: added(was.asks, permissionsNow.asks),
        added_commands: added(permissionsNow.commands, was.commands),
        removed_commands: added(was.commands, permissionsNow.commands),
        network_added: added(permissionsNow.network_hosts, was.network_hosts),
        network_removed: added(was.network_hosts, permissionsNow.network_hosts),
        tools_added: permissionsNow.tools.filter((t) => !was.tools.some((b) => b.name === t.name)),
        tools_removed: added(was.tools.map((t) => t.name), permissionsNow.tools.map((t) => t.name)).sort(),
        tools_changed: permissionsNow.tools.flatMap((t) => {
          const before = (was.tools as { name: string; description?: string }[]).find((b) => b.name === t.name);
          return before && before.description !== t.description ? [{ name: t.name, description_before: before.description, description_after: t.description, input_changed: false }] : [];
        }),
        ui_change_added: added(ui(permissionsNow).change, ui(was).change),
        ui_replace_added: added(ui(permissionsNow).replace, ui(was).replace),
        ...binds(base?.manifest ?? null, v.manifest),
      };
    })(),
    checks: v.checks,
  });
}

function hash(text: string): string {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619) >>> 0;
  return h.toString(16).padStart(8, "0").repeat(8);
}

async function body(request: Request): Promise<Record<string, unknown>> {
  try {
    const value = await request.json();
    return value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

async function adminAction(request: Request, parts: string[]): Promise<Response> {
  const who = adminWriter(request);
  if (who instanceof Response) return who;
  const input = await body(request);
  const text = (key: string) => (typeof input[key] === "string" ? (input[key] as string).trim() : "");
  // versions/:id/:version/{verify|decline} (amendment 45)
  if (parts[0] === "versions" && parts.length === 4) {
    const p = state.plugins.find((x) => x.id === parts[1]);
    const v = p?.versions.find((x) => x.version === parts[2]);
    if (!p || !v) return notFound();
    if (!shown(v)) return fail(409, "invalid_state", `It's ${v.status}.`);
    if (parts[3] === "verify") {
      if (v.verified) return fail(409, "invalid_state", "It's verified.");
      v.verified = true;
      v.verified_at = Date.now();
      v.review = { decided: true, decision: "verified", decided_at: v.verified_at };
      // (Every verified version can be installed: a superseded stop goes; an author's own stays.)
      if (v.yanked_by === "superseded") {
        v.status = "listed";
        v.yanked_by = null;
      }
      log(who.user, "version.verify", p.id, v.version, null);
      return json(versionInfo(v, true));
    }
    if (parts[3] === "decline") {
      if (v.review.decided) return fail(409, "invalid_state", "A reviewer already decided.");
      if (!text("reason") || !text("message")) return fail(400, "bad_request", "A reason and a message for the author are needed.");
      v.review = { decided: true, decision: "declined", reason: text("reason"), message: text("message"), decided_at: Date.now() };
      log(who.user, "version.decline", p.id, v.version, { reason: text("reason") });
      return json(versionInfo(v, true));
    }
    return notFound();
  }
  // plugins/:id/{suspend|restore|feature|transfer} and plugins/:id/versions/:version/{take-down|remove}
  if (parts[0] === "plugins") {
    const p = state.plugins.find((x) => x.id === parts[1]);
    if (!p) return notFound();
    if (parts[2] === "versions" && parts[4] === "restore") {
      // (A taken-down version back to listed, unverified, its block lifted; superseded again if anything newer on its line is listed.)
      const v = p.versions.find((x) => x.version === parts[3]);
      if (!v) return notFound();
      if (v.status !== "taken_down") return fail(409, "invalid_state", `It's ${v.status}.`);
      if (!text("reason")) return fail(400, "bad_request", "A reason is needed.");
      v.status = "listed";
      v.blocked = false;
      v.removal = null;
      v.verified = false;
      v.verified_at = null;
      v.review = { decided: false, decision: null };
      const newer = p.versions.find((x) => x !== v && x.status === "listed" && x.published_at > v.published_at && (!prerelease(x) || prerelease(v)));
      if (newer) {
        v.status = "yanked";
        v.yanked_by = "superseded";
        v.superseded = { at: Date.now(), by: newer.version };
      }
      log(who.user, "version.restore", p.id, v.version, { reason: text("reason") });
      return json(versionInfo(v, true));
    }
    if (parts[2] === "versions" && (parts[4] === "take-down" || parts[4] === "remove")) {
      const v = p.versions.find((x) => x.version === parts[3]);
      if (!v) return notFound();
      if (!shown(v)) return fail(409, "invalid_state", `It's ${v.status}.`);
      if (!text("reason")) return fail(400, "bad_request", "A reason is needed.");
      // (Taken down: harmful, turned off on devices, and no longer verified. Removed: neither.)
      const harmful = parts[4] === "take-down";
      v.status = harmful ? "taken_down" : "removed";
      v.blocked = harmful;
      v.yanked_by = null;
      v.removal = { at: Date.now(), reason: text("reason") };
      if (harmful) {
        v.verified = false;
        v.verified_at = null;
      }
      log(who.user, harmful ? "version.take_down" : "version.remove", p.id, v.version, { reason: text("reason") });
      return json(versionInfo(v, true));
    }
    switch (parts[2]) {
      case "suspend":
        if (!text("reason")) return fail(400, "bad_request", "A reason is needed.");
        p.status = "suspended";
        log(who.user, "plugin.suspend", p.id, null, { reason: text("reason") });
        p.suspend_reason = text("reason");
        return json({ status: p.status });
      case "restore":
        p.status = "listed";
        log(who.user, "plugin.restore", p.id, null, null);
        return json({ status: p.status });
      case "feature":
        // (Only a plugin whose newest version is verified can be featured: amendment 45.)
        if (input.featured === true && !latest(p)?.verified) return fail(409, "not_verified", "Its newest version isn't verified.");
        p.featured = input.featured === true;
        log(who.user, p.featured ? "plugin.feature" : "plugin.unfeature", p.id, null, null);
        return json({ featured: p.featured });
      case "transfer": {
        const to = USERS.find((u) => u.username === text("to_username"));
        if (!to) return fail(404, "not_found", "No account has that username.");
        log(who.user, "plugin.transfer", p.id, null, { from: userOf(p.owner).username, to: to.username });
        p.owner = to.id;
        return json({ owner: person(to.id) });
      }
    }
  }
  // reservations { id, to_username?, note? }
  if (parts[0] === "reservations" && parts.length === 1) {
    const id = text("id");
    if (!/^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$/.test(id)) return fail(400, "bad_request", "id is a plugin id.");
    // (The site's own page words: nobody can be granted one, with an account or without.)
    if (SITE_IDS.includes(id)) return fail(400, "site_id", `"${id}" is one of the site's pages: no plugin can have it.`);
    if (state.plugins.some((p) => p.id === id && p.versions.some(shown))) return fail(409, "invalid_state", `"${id}" is in use: transfer it instead.`);
    let user: string | null = null;
    if (text("to_username")) {
      user = USERS.find((u) => u.username === text("to_username"))?.id ?? null;
      if (!user) return fail(400, "bad_request", "to_username is a member's username.");
    }
    const note = text("note") || null;
    state.reservations = state.reservations.filter((r) => r.id !== id);
    state.reservations.push({ id, user, note, created_at: Date.now() });
    log(who.user, "reservation.set", id, null, { to: user ? userOf(user).username : null, note });
    return json({ id, to: user ? { username: userOf(user).username } : null, note, released: false });
  }
  // reports/:id/resolve
  if (parts[0] === "reports" && parts[2] === "resolve") {
    const r = state.reports.find((x) => String(x.id) === parts[1]);
    if (!r) return notFound();
    const resolution = text("resolution");
    if (resolution !== "dismissed" && resolution !== "actioned") return fail(400, "bad_request");
    r.status = resolution;
    r.note = text("note") || null;
    log(who.user, `report.${resolution}`, r.plugin_id, null, { report: r.id, note: r.note });
    return json({ status: r.status });
  }
  return notFound();
}

// ── Deploy tokens ───────────────────────────────────────────────────────

const STEP_UP_MS = 10 * 60_000;
const MOST_LIVE_TOKENS = 20;
const MOST_TOKENS_A_DAY = 10;
const MOST_SCOPE = 50;
const PLUGIN_ID = /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/;

/** Whether this session holds a step-up ticket it hasn't spent (a sign-in in the last ten minutes). */
const fresh = (id: string) => {
  const until = state.sessions.get(id)?.fresh_until;
  return typeof until === "number" && until > Date.now();
};

const wireToken = ({ owner: _, hash: __, ...t }: (typeof state.tokens)[number]) => ({ ...t, scope: "all" in t.scope ? { all: true, all_kinds: t.scope.all_kinds ?? ["plugin"] } : t.scope });
const refuse = (status: number, error: string, message: string, extra: Record<string, unknown> = {}, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify({ error, message, ...extra }), { status, headers: { "content-type": "application/json", "cache-control": "no-store", ...headers } });

/** `/store/v1/me/tokens[/:id]`, as the edge answers it (edge/src/store/identity/deploy-tokens.ts). */
async function tokenRoute(request: Request, method: string, id: string | undefined, depth: number): Promise<Response> {
  if (depth > 5) return notFound();
  const allowed = id === undefined ? ["GET", "POST"] : ["DELETE"];
  if (!allowed.includes(method)) return new Response(JSON.stringify({ error: "method_not_allowed" }), { status: 405, headers: { "content-type": "application/json", allow: allowed.join(", ") } });
  // (A token manages nothing, its own kind included.)
  if (request.headers.get("authorization")?.match(/^Bearer zdt_/i)) return refuse(401, "session_required", "This needs a signed-in session, not a deploy token.");
  const s = session(request);
  if (!s) return fail(401, "signed_out");
  if (method === "GET") return json({ tokens: state.tokens.filter((t) => t.owner === s.user.id).map(wireToken) });
  const who = writer(request);
  if (who instanceof Response) return who;
  if (method === "DELETE") {
    const t = TOKEN_ID.test(id!) ? state.tokens.find((x) => x.owner === s.user.id && x.id === id) : undefined;
    if (!t || t.revoked_at !== null) return refuse(404, "unknown_token", "You have no such deploy token.");
    t.revoked_at = Date.now();
    return json({ revoked: true });
  }
  const body = (await request.json().catch(() => null)) as Record<string, unknown> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  if (name.length < 1 || name.length > 64 || /[\u0000-\u001f\u007f]/.test(name)) return refuse(400, "invalid_name", "Name the token in 1–64 characters.");
  const now = Date.now();
  const expires = body?.expires ?? null;
  if (expires !== null && !(Number.isSafeInteger(expires) && (expires as number) > now && (expires as number) <= now + 5 * 365 * DAY)) return refuse(400, "invalid_expiry", "expires must be a future time in epoch milliseconds, within 5 years.");
  const raw = body?.scope as Record<string, unknown> | undefined;
  let scope: { all: true; all_kinds: Kind[] } | { plugins: string[] };
  const badScope = (message: string, ids?: unknown[]) => refuse(400, "invalid_scope", message, ids?.length ? { ids: ids.map(String) } : {});
  // (`all` names its kinds; left out, plugins alone, and always said on an answer.)
  const kinds = JSON.stringify(raw?.all_kinds ?? ["plugin"]);
  if (raw && typeof raw === "object" && !Array.isArray(raw) && raw.all === true && Object.keys(raw).every((k) => k === "all" || k === "all_kinds") && (kinds === '["plugin"]' || kinds === '["plugin","template"]')) scope = { all: true, all_kinds: JSON.parse(kinds) as Kind[] };
  else if (raw && typeof raw === "object" && Object.keys(raw).length === 1 && Array.isArray(raw.plugins)) {
    const ids = raw.plugins as unknown[];
    if (ids.length < 1 || ids.length > MOST_SCOPE) return badScope(`A token's scope lists 1 to ${MOST_SCOPE} plugins.`);
    const bad = ids.filter((x) => typeof x !== "string" || !PLUGIN_ID.test(x));
    if (bad.length) return badScope("Each plugin is a plugin id.", bad);
    const repeated = ids.filter((x, i) => ids.indexOf(x) !== i);
    if (repeated.length) return badScope("Name each plugin once.", [...new Set(repeated)]);
    scope = { plugins: (ids as string[]).slice().sort() };
    const notOwned = scope.plugins.filter((p) => state.plugins.find((x) => x.id === p)?.owner !== s.user.id);
    if (notOwned.length) return refuse(403, "not_owner", `You don't own ${notOwned.join(", ")} in the store.`, { ids: notOwned });
  } else return badScope("scope is { all: true } or { plugins: [id, …] }.");
  // The account's limits, then the fresh sign-in, spent only by a token made (the edge's order: user-directory.ts).
  const own = state.tokens.filter((t) => t.owner === s.user.id);
  if (own.filter((t) => t.created_at > now - DAY).length >= MOST_TOKENS_A_DAY) return refuse(429, "rate_limited", "Too many new deploy tokens today (10). Try again tomorrow.", {}, { "retry-after": "3600" });
  if (own.filter((t) => t.revoked_at === null && (t.expires_at === null || t.expires_at > now)).length >= MOST_LIVE_TOKENS) return refuse(409, "too_many_tokens", "This account has as many live deploy tokens as it can (20). Revoke one first.");
  if (!fresh(s.id)) return refuse(403, "step_up_required", "Sign in again to do this. A fresh sign-in is needed, not just this device's session.");
  state.sessions.get(s.id)!.fresh_until = null;
  const { token, short } = makeToken(crypto.getRandomValues(new Uint8Array(32)));
  const row = { owner: s.user.id, id: tokenId(crypto.getRandomValues(new Uint8Array(11))), short, hash: await tokenHash(token), scope, last_used_plugin: null, name, created_at: now, last_used_at: null, expires_at: expires as number | null, revoked_at: null };
  state.tokens.unshift(row);
  // The secret, this once.
  return new Response(JSON.stringify({ token, id: row.id, short, name, scope, created_at: now, expires_at: row.expires_at }), { status: 201, headers: { "content-type": "application/json", "cache-control": "no-store" } });
}

// ── Sign-in (the fake's own: pick an account) ───────────────────────────

const safeReturn = (value: string | null) => (value && value.startsWith("/") && !value.startsWith("//") && !value.startsWith("/\\") ? value : "/");
const escape = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

function login(url: URL): Response {
  const to = safeReturn(url.searchParams.get("return_to"));
  const rows = USERS.map((u) => `<li><a href="/auth/web/callback?as=${u.id}&amp;return_to=${encodeURIComponent(to)}">${escape(u.name)} ${u.username ? `(@${escape(u.username)})` : "(no username)"}${u.roles.length ? ` · ${u.roles.join(", ")}` : ""}</a></li>`).join("");
  return new Response(`<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width"><title>Sign in (fake)</title><body style="font:16px system-ui;background:#0e0e10;color:#ededef;padding:40px"><h1>Sign in to the fake edge</h1><p>Development only. Pick an account:</p><ul>${rows}</ul>`, {
    headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store", "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'" },
  });
}

function callback(url: URL): Response {
  const user = USERS.find((u) => u.id === url.searchParams.get("as"));
  if (!user) return fail(400, "bad_request");
  const id = crypto.randomUUID();
  // (Every web sign-in asks for credentials, so it earns a step-up ticket: ten minutes, used once. The fake's
  // `fresh=0` signs in without one, as a session whose ticket is spent or old.)
  const ticket = url.searchParams.get("fresh") === "0" ? null : Date.now() + STEP_UP_MS;
  state.sessions.set(id, { user: user.id, csrf: crypto.randomUUID().replace(/-/g, ""), fresh_until: ticket });
  return new Response(null, {
    status: 303,
    // (The edge sends the browser back to an absolute address on the site.)
    headers: { location: new URL(safeReturn(url.searchParams.get("return_to")), url.origin).href, "set-cookie": `${COOKIE}=${id}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=${user.roles.includes("admin") ? 3600 : 30 * 86400}`, "cache-control": "no-store" },
  });
}

async function logout(request: Request, url: URL): Promise<Response> {
  const s = session(request);
  const form = await request.formData().catch(() => null);
  const origin = request.headers.get("origin");
  if (s) {
    if (origin !== url.origin || form?.get("csrf") !== s.csrf) return fail(403, "bad_csrf", "This sign-out didn't come from the site.");
    for (const [id, other] of state.sessions) if (other.user === s.user.id) state.sessions.delete(id);
  }
  // Always the site's front page: the edge takes no return_to here.
  return new Response(null, {
    status: 303,
    headers: { location: new URL("/", url.origin).href, "set-cookie": `${COOKIE}=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0`, "cache-control": "no-store" },
  });
}

// ── Routing ──────────────────────────────────────────────────────────────

async function route(request: Request): Promise<Response> {
  const url = new URL(request.url);
  const method = request.method;
  const parts = url.pathname.split("/").filter(Boolean).map((p) => decodeURIComponent(p));

  if (parts[0] === "auth" && parts[1] === "web") {
    if (parts[2] === "login" && method === "GET") return login(url);
    if (parts[2] === "callback" && method === "GET") return callback(url);
    if (parts[2] === "logout" && method === "POST") return logout(request, url);
    if (parts[2] === "session" && method === "GET") {
      const s = session(request);
      if (!s) return fail(401, "signed_out");
      return json({ user: { name: s.user.name, email: `${s.user.id.slice(5)}@example.com` }, username: s.user.username, roles: s.user.roles, csrf: s.csrf, expires_at: NOW + DAY, fresh_sign_in: fresh(s.id) });
    }
    return notFound();
  }
  // The release the installer and the site offer (the edge's /releases/*).
  if (parts[0] === "releases" && parts[1] === "latest.txt" && method === "GET") return new Response("0.3.4\n", { headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-cache" } });
  // (The fake's own: the queue as a screenshot needs it.)
  if (parts[0] === "_fake" && parts[1] === "queue" && method === "POST") {
    const as = url.searchParams.get("as");
    state.queueAs = as === "empty" ? "empty" : as && /^\d{1,3}$/.test(as) ? Number(as) : null;
    return json({ queue: state.queueAs });
  }
  if (parts[0] === "_fake" && parts[1] === "bare" && method === "POST") {
    state.bare = url.searchParams.get("id") || null;
    return json({ bare: state.bare });
  }
  // (Every account's deploy tokens as the fixtures have them, those a test made gone: POST /_fake/tokens.)
  if (parts[0] === "_fake" && parts[1] === "tokens" && method === "POST") {
    state.tokens = tokens();
    return json({ tokens: state.tokens.length });
  }
  if (parts[0] === "_fake" && parts[1] === "empty" && method === "POST") {
    state.empty = url.searchParams.get("on") === "1";
    return json({ empty: state.empty });
  }
  // (One plugin's version under another name, everywhere it is named: POST /_fake/rename?id=…&from=…&to=…. A version the app's link can't carry, without a fixture of its own.)
  // (Ada's thirty more, and her "Every one" token naming all fifty: POST /_fake/many?on=1; without on=1, as before.)
  if (parts[0] === "_fake" && parts[1] === "many" && method === "POST") {
    const on = url.searchParams.get("on") === "1";
    const extra = new Set(MANY_IDS);
    state.plugins = state.plugins.filter((p) => !extra.has(p.id));
    if (on) state.plugins.push(...manyMore());
    const every = state.tokens.find((k) => k.owner === "user_ada" && k.name === "Every one");
    if (every) every.scope = { plugins: state.plugins.filter((p) => p.owner === "user_ada" && kindOf(p) === "plugin").map((p) => p.id).sort() };
    return json({ many: on });
  }
  if (parts[0] === "_fake" && parts[1] === "loose" && method === "POST") {
    state.loose = url.searchParams.get("on") === "1";
    return json({ loose: state.loose });
  }
  if (parts[0] === "_fake" && parts[1] === "rename" && method === "POST") {
    const [id, from, to] = ["id", "from", "to"].map((k) => url.searchParams.get(k) ?? "");
    const i = state.plugins.findIndex((p) => p.id === id);
    if (i < 0 || !from || !to) return fail(400, "bad_request", "id, from and to.");
    state.plugins[i] = JSON.parse(JSON.stringify(state.plugins[i]).replaceAll(JSON.stringify(from), JSON.stringify(to)));
    return json({ renamed: to });
  }
  if (parts[0] === "_fake" && parts[1] === "templates" && method === "POST") {
    state.plugins = state.plugins.filter((p) => !p.id.startsWith("house-"));
    if (url.searchParams.get("on") === "1") state.plugins.push(...moreTemplates());
    return json({ plugins: state.plugins.length });
  }
  if (parts[0] === "_fake" && parts[1] === "down" && method === "POST") {
    state.down = url.searchParams.get("on") === "1";
    return json({ down: state.down });
  }
  if (parts[0] !== "store" || parts[1] !== "v1") return notFound();
  if (state.down) return fail(503, "unknown", "The store is down (the fake's own switch).");
  const [, , a, b, c, d, e] = parts;

  // (The store-wide routes take `?kind=`, plugins unasked; the rest are under each kind's own prefix.)
  const asked = url.searchParams.get("kind");
  if (asked !== null && asked !== "plugin" && asked !== "template") return fail(400, "bad_request", "kind is plugin or template.");
  const over: Kind = asked ?? "plugin";
  const here: Kind = a === "templates" ? "template" : "plugin";
  const ofKind = (p: FakePlugin) => kindOf(p) === over && listed(p);
  if (a === "categories" && b === undefined && method === "GET") {
    if (over === "template") return json({ categories: [] }, 200, { "cache-control": "public, max-age=60" });
    // (Each with how many listed plugins it holds; with `top`, its n most installed, as the list's popular order has them: ac4fc5d3f.)
    const topText = url.searchParams.get("top");
    if (topText !== null && !/^[1-4]$/.test(topText)) return fail(400, "bad_request", "top is 1 to 4.");
    const counted = await Promise.all(
      CATEGORIES.map(async (c) => ({
        ...c,
        count: state.plugins.filter((p) => ofKind(p) && card(p).categories.includes(c.id)).length,
        ...(topText ? { top: ((await catalog(new URL(`https://fake.invalid/?category=${c.id}&sort=popular&limit=${topText}`)).json()) as { items: unknown[] }).items } : {}),
      })),
    );
    return json({ categories: counted }, 200, { "cache-control": "public, max-age=60" });
  }
  if (a === "keywords" && b === undefined && method === "GET") {
    // (Most used first, ties by keyword; each count the list's total for ?keyword=: ac4fc5d3f.)
    const category = url.searchParams.get("category");
    const limitText = url.searchParams.get("limit");
    if (category !== null && !CATEGORIES.some((c) => c.id === category)) return fail(400, "bad_request", "Unknown category.");
    if (limitText !== null && !(/^\d{1,2}$/.test(limitText) && Number(limitText) >= 1 && Number(limitText) <= 24)) return fail(400, "bad_request", "limit is 1 to 24.");
    const counts = new Map<string, number>();
    for (const p of state.plugins.filter(ofKind).filter((p) => !category || card(p).categories.includes(category))) {
      for (const k of new Set(card(p).keywords.map((k) => k.toLowerCase()))) counts.set(k, (counts.get(k) ?? 0) + 1);
    }
    const keywords = [...counts].map(([keyword, count]) => ({ keyword, count })).sort((x, y) => y.count - x.count || (x.keyword < y.keyword ? -1 : x.keyword > y.keyword ? 1 : 0));
    return json({ keywords: keywords.slice(0, Number(limitText ?? 24)) }, 200, { "cache-control": "public, max-age=60" });
  }
  if (a === "stats" && method === "GET") {
    const all = state.plugins.filter(ofKind);
    // (new_7d: listed plugins first listed in the last seven days: ac4fc5d3f.)
    return json({ plugins: all.length, verified: all.filter((p) => latest(p)!.verified).length, new_7d: all.filter((p) => listedAt(p)! >= NOW - 7 * DAY).length }, 200, { "cache-control": "public, max-age=30" });
  }
  if ((a === "plugins" || a === "templates") && b === undefined && method === "GET") return catalog(url, here);
  if ((a === "plugins" || a === "templates") && b && c === undefined && method === "GET") {
    const p = itemOf(here, b);
    return p && listed(p) ? json(detail(p), 200, { "cache-control": "public, max-age=30" }) : notFound();
  }
  if ((a === "plugins" || a === "templates") && b && c === "versions" && d === undefined && method === "GET") {
    // (Every listed and yanked version, newest first, paged: amendment 45.)
    const p = itemOf(here, b);
    return p && listed(p) ? json(paged(p.versions.filter(shown).map((v) => versionInfo(v, false)), url, "versions"), 200, { "cache-control": "public, max-age=30" }) : notFound();
  }
  if ((a === "plugins" || a === "templates") && b && c === "versions" && d && e === undefined && method === "GET") {
    const p = itemOf(here, b);
    const v = p && listed(p) ? p.versions.find((x) => (state.loose || x.version === d) && shown(x)) : undefined;
    return v ? json(versionInfo(v, false), 200, { "cache-control": "public, max-age=30" }) : notFound();
  }
  if ((a === "plugins" || a === "templates") && b && c === "files" && d && parts.length > 6 && method === "GET") return file(request, here, b, d, parts.slice(6).join("/"));
  if ((a === "plugins" || a === "templates") && b && (here === "plugin" || itemOf(here, b)) && c === "versions" && d && ["yank", "unyank"].includes(e ?? "") && method === "POST") return versionAction(request, b, d, e!);
  if ((a === "plugins" || a === "templates") && b && c === "reports" && method === "POST") {
    const who = writer(request);
    if (who instanceof Response) return who;
    const p = itemOf(here, b);
    if (!p || !listed(p)) return notFound();
    const input = await body(request);
    const reason = String(input.reason ?? "");
    if (!["malware", "misleading", "broken", "copyright", "other"].includes(reason)) return fail(400, "bad_request", "Pick a reason.");
    const text = String(input.text ?? "").trim().slice(0, 2000);
    state.reports.unshift({ id: (state.reports.reduce((n, r) => Math.max(n, r.id), 0) + 1), plugin_id: p.id, version: latest(p)!.version, reason: reason as "other", text, reporter: who.user.id, created_at: Date.now(), status: "open", note: null });
    return json({ ok: true }, 201);
  }
  if (a === "me" && (b === "plugins" || b === "templates") && method === "GET") {
    const s = session(request);
    // (`{publisher_id, plugins}`: the contract after e3daa69b4.)
    return s ? json({ publisher_id: publisherOf(s.user.id), ...(b === "plugins" ? { plugins: mine(s.user) } : { templates: mine(s.user, "template") }) }) : fail(401, "signed_out");
  }
  // The account's deploy tokens (contract a315eef5a): listed, made with a fresh sign-in, revoked.
  if (a === "me" && b === "tokens") return tokenRoute(request, method, c, parts.length);
  if (a === "admin") {
    if (b === "reservations" && method === "DELETE" && c) {
      const who = adminWriter(request);
      if (who instanceof Response) return who;
      const before = state.reservations.length;
      state.reservations = state.reservations.filter((r) => r.id !== c);
      if (state.reservations.length === before) return notFound();
      log(who.user, "reservation.release", c, null, null);
      return json({ id: c, released: true });
    }
    if (method === "POST") return adminAction(request, parts.slice(3));
    const who = admin(request);
    if (who instanceof Response) return who;
    if (b === "queue") return queue(url);
    if (b === "versions" && c && d) return submission(c, d);
    if (b === "plugins" && c) {
      const p = state.plugins.find((x) => x.id === c);
      if (!p) return notFound();
      return json({
        plugin: { ...card(p), listed_version: latest(p)?.version ?? null, created_at: p.created_at, suspend_reason: p.suspend_reason ?? null },
        owner: { id: author(p.owner).id, username: userOf(p.owner).username, user_id: p.owner, created_at: userOf(p.owner).created_at },
        versions: p.versions.map((v) => versionInfo(v, true)),
        reports: state.reports.filter((r) => r.plugin_id === p.id).map(reportView),
        featured: p.featured,
        status: p.status === "suspended" ? "suspended" : listed(p) ? "listed" : "unlisted",
      });
    }
    if (b === "reports") {
      const status = url.searchParams.get("status") ?? "open";
      return json(paged(state.reports.filter((r) => state.queueAs !== "empty" && (status === "all" || r.status === status)).map(reportView), url, "reports"));
    }
    if (b === "reservations" && c === undefined) {
      return json({ reservations: [...(state.queueAs === "empty" ? [] : state.reservations)].sort((x, y) => x.id.localeCompare(y.id)).map((r) => ({ id: r.id, to: r.user ? { username: userOf(r.user).username } : null, note: r.note, created_at: r.created_at })) });
    }
    if (b === "audit") {
      const plugin = url.searchParams.get("plugin");
      return json(paged(state.audit.filter((x) => state.queueAs !== "empty" && (!plugin || x.plugin_id === plugin)).map((x) => ({ ...x, actor: { id: author(x.actor).id, username: userOf(x.actor).username, user_id: x.actor } })), url, "entries"));
    }
  }
  return notFound();
}

/** A page of an admin list, with cursors both ways (amendment 29). */
function paged<T>(all: T[], url: URL, field: string): Record<string, unknown> {
  // (`limit`: 1 to 50, the edge's own most and default; 20 here unasked, so a short list still has pages.)
  const asked = Number(url.searchParams.get("limit"));
  const limit = Number.isInteger(asked) && asked >= 1 && asked <= 50 ? asked : 20;
  const cursor = url.searchParams.get("cursor");
  const start = cursor ? Math.max(0, Number(atob(cursor.replace(/-/g, "+").replace(/_/g, "/"))) || 0) : 0;
  const at = (n: number) => btoa(String(n)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return {
    [field]: all.slice(start, start + limit),
    // (Every paged list's count, beside its items: amendment 38.)
    total: all.length,
    ...(start + limit < all.length ? { next_cursor: at(start + limit) } : {}),
    ...(start > 0 ? { prev_cursor: at(Math.max(0, start - limit)) } : {}),
  };
}

function reportView(r: (typeof state.reports)[number]) {
  const p = state.plugins.find((x) => x.id === r.plugin_id);
  const { plugin_id: _, reporter, ...rest } = r;
  return { ...rest, plugin: { id: r.plugin_id, name: p ? String(p.versions[0]!.manifest.name) : null }, reporter: { id: author(reporter).id, username: userOf(reporter).username }, resolved_by: null, resolved_at: null };
}

export default {
  async fetch(request: Request): Promise<Response> {
    if (!state) {
      startClock(Date.now());
      state = freshState();
    }
    try {
      const response = await route(request);
      return new URL(request.url).pathname.startsWith("/store/v1/admin/") ? rotate(request, response) : response;
    } catch (error) {
      return fail(500, "unknown", error instanceof Error ? error.message : String(error));
    }
  },
};
