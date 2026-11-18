// The fake edge: the store's HTTP contract and the web sign-in, over the
// fixtures, in memory (a restart starts over). It behaves as the plan says
// the edge does where the site can tell: who may see what, CSRF and Origin
// on every write, keyset cursors, error words. Its sign-in is a page of the
// fixtures' accounts in place of WorkOS.

import { audit, CATEGORIES, DAY, keys, NOW, plugins, reports, USERS, type FakePlugin, type FakeUser, type FakeVersion } from "./fixtures.ts";

const COOKIE = "__Host-zeron_session";

const state = {
  plugins: plugins(),
  reports: reports(),
  audit: audit(),
  keys: keys(),
  reservations: [
    { id: "zeron-tools", user: "user_grace", note: "For the team's own plugins.", created_at: NOW - 40 * DAY },
    { id: "linear", user: null, note: "Held until Linear asks for it.", created_at: NOW - 12 * DAY },
  ] as { id: string; user: string | null; note: string | null; created_at: number }[],
  /** session id → user id, csrf */
  /** `rotated`: a staff session already replaced on its first privileged use. */
  sessions: new Map<string, { user: string; csrf: string; rotated?: boolean }>(),
};

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

const latest = (p: FakePlugin) => p.versions.find((v) => v.status === "approved");
const listed = (p: FakePlugin) => p.status === "listed" && latest(p) !== undefined;

const author = (userId: string) => {
  const u = userOf(userId);
  return { id: `pub_${hash(u.id).slice(0, 32)}`, username: u.username, badges: u.badges };
};

function card(p: FakePlugin, version: FakeVersion | undefined = latest(p)) {
  const listedVersion = latest(p);
  const v = version ?? p.versions[0]!;
  const m = v.manifest;
  return {
    id: p.id,
    name: String(m.name),
    summary: String(m.description),
    icon: listedVersion && typeof m.icon === "string" ? m.icon : null,
    author: author(p.owner),
    categories: (m.categories as string[]) ?? [],
    keywords: (m.keywords as string[]) ?? [],
    version: listedVersion?.version ?? null,
    updated_at: (listedVersion ?? v).published_at,
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
  const decided = v.status === "approved" || v.status === "yanked" || v.status === "removed";
  return {
    version: v.version,
    status: v.status,
    published_at: decided ? (v.review?.decided_at ?? v.published_at) : null,
    submitted_at: v.published_at,
    notes: v.notes,
    engines: v.engines,
    yanked: v.status === "yanked",
    prerelease: v.version.includes("-"),
    key: "k5Yh3xQ2b8VtYwGm1rPz0nLc4dEa6sJu9oKi7fBg2Wq",
    hash: `b3:${hash(v.version + bytes)}`,
    sha256: hash(JSON.stringify(v.manifest)).slice(0, 64),
    bytes,
    ...(full ? { review: { state: v.status, ...(v.review ?? {}) }, checks: v.checks, blocked: v.blocked === true, via: "session" } : {}),
  };
}

function detail(p: FakePlugin) {
  const v = latest(p)!;
  const m = v.manifest;
  const bytes = Object.values(v.files).reduce((n, f) => n + f.length, 0);
  return {
    ...card(p, v),
    readme: v.files["README.md"] ?? "",
    screenshots: (m.screenshots as string[]) ?? [],
    permissions: permissions(m),
    engines: v.engines,
    homepage: (m.homepage as string) ?? null,
    repository: (m.repository as string) ?? null,
    license: (m.license as string) ?? null,
    versions: p.versions.filter((x) => x.status === "approved" || x.status === "yanked").slice(0, 5).map((x) => versionInfo(x, false)),
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

function catalog(url: URL): Response {
  const q = (url.searchParams.get("q") ?? "").trim().toLowerCase().slice(0, 100);
  const terms = q.split(/[^a-z0-9]+/).filter(Boolean);
  const category = url.searchParams.get("category");
  const keyword = url.searchParams.get("keyword")?.toLowerCase();
  const featured = url.searchParams.get("featured") === "true";
  let sort = url.searchParams.get("sort") ?? (terms.length ? "relevance" : "popular");
  if (!["relevance", "popular", "updated", "new"].includes(sort)) return fail(400, "bad_request", "Unknown sort.");
  if (sort === "relevance" && !terms.length) sort = "popular";
  if (category && !CATEGORIES.some((c) => c.id === category)) return fail(400, "bad_request", "Unknown category.");
  const limit = Math.min(50, Math.max(1, Number(url.searchParams.get("limit") ?? 24) || 24));

  const keyOf = (p: FakePlugin): Key => {
    const value = sort === "relevance" ? score(p, terms) : sort === "popular" ? p.installs : sort === "updated" ? latest(p)!.published_at : p.created_at;
    return [value, p.id];
  };
  // Descending by value, then ascending by id: a stable total order.
  const before = (a: Key, b: Key) => a[0] > b[0] || (a[0] === b[0] && a[1] < b[1]);

  const all = state.plugins
    .filter(listed)
    .filter((p) => !featured || p.featured)
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
  const body: { items: unknown[]; next_cursor?: string; prev_cursor?: string } = { items: page.map((x) => card(x.p)) };
  if (start + limit < all.length && page.length) body.next_cursor = encode({ dir: "after", key: page.at(-1)!.key, sort, q });
  if (start > 0 && page.length) body.prev_cursor = encode({ dir: "before", key: page[0]!.key, sort, q });
  return json(body, 200, { "cache-control": "public, max-age=30" });
}

// ── Files ────────────────────────────────────────────────────────────────

const TYPES: Record<string, string> = { svg: "image/svg+xml", png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", avif: "image/avif", md: "text/markdown; charset=utf-8", json: "application/json", luau: "text/plain; charset=utf-8" };

function file(request: Request, id: string, version: string, path: string): Response {
  const p = state.plugins.find((x) => x.id === id);
  const v = p?.versions.find((x) => x.version === version);
  if (!p || !v || !(path in v.files)) return notFound();
  const ext = path.split(".").pop()!.toLowerCase();
  // A picture is shown inline only when its bytes are that picture
  // (amendment 14); anything else is given as a download.
  const body = v.files[path]!;
  const MAGIC: Record<string, RegExp> = { png: /^\x89PNG/, jpg: /^\xff\xd8/, jpeg: /^\xff\xd8/, gif: /^GIF8[79]a/, webp: /^RIFF/, avif: /^....ftyp/, svg: /^\s*<svg[\s>]/ };
  const picture = Object.hasOwn(MAGIC, ext) && MAGIC[ext]!.test(body);
  const open = listed(p) && (v.status === "approved" || v.status === "yanked") && picture;
  const s = session(request);
  const allowed = open || (s && (s.user.id === p.owner || isAdmin(s.user)));
  if (!allowed) return notFound();
  return new Response(v.files[path], {
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

function mine(user: FakeUser) {
  return state.plugins
    .filter((p) => p.owner === user.id)
    .map((p) => ({ ...card(p), listing: p.status === "suspended" ? "suspended" : listed(p) ? "listed" : "unlisted", versions: p.versions.map((v) => versionInfo(v, true)) }));
}

function versionAction(request: Request, id: string, version: string, action: string): Response {
  const who = writer(request);
  if (who instanceof Response) return who;
  const p = state.plugins.find((x) => x.id === id && x.owner === who.user.id);
  const v = p?.versions.find((x) => x.version === version);
  if (!p || !v) return notFound();
  const from: Record<string, FakeVersion["status"]> = { withdraw: "pending", yank: "approved", unyank: "yanked" };
  const to: Record<string, FakeVersion["status"]> = { withdraw: "withdrawn", yank: "yanked", unyank: "approved" };
  if (!(action in from)) return notFound();
  if (v.status !== from[action]) return fail(409, "conflict", `It's ${v.status}.`);
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
  state.sessions.set(next, { user: s.user.id, csrf: s.csrf, rotated: true });
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

function queue(url: URL): Response {
  const status = url.searchParams.get("status") ?? "pending";
  const items = state.plugins.flatMap((p) =>
    p.versions
      .filter((v) => v.status === status)
      .map((v) => {
        const base = p.versions.find((x) => x.status === "approved" && x !== v && x.published_at < v.published_at);
        const asked = (v.manifest.permissions as string[]) ?? [];
        const had = (base?.manifest.permissions as string[]) ?? [];
        return {
          plugin: { id: p.id, name: String(v.manifest.name) },
          version: v.version,
          status: v.status,
          prerelease: v.version.includes("-"),
          submitted_at: v.published_at,
          publisher: { id: author(p.owner).id, username: userOf(p.owner).username },
          ...(base ? { base: base.version } : {}),
          new_asks: asked.filter((a) => !had.includes(a)),
          checks: { ok: v.checks.filter((c) => c.level === "ok").length, warn: v.checks.filter((c) => c.level === "warn").length, fail: v.checks.filter((c) => c.level === "fail").length },
        };
      }),
  );
  // Oldest waiting first.
  items.sort((a, b) => a.submitted_at - b.submitted_at);
  return json(paged(items, url, "items"));
}

function submission(id: string, version: string): Response {
  const p = state.plugins.find((x) => x.id === id);
  const v = p?.versions.find((x) => x.version === version);
  if (!p || !v) return notFound();
  const base = p.versions.find((x) => x.status === "approved" && x !== v && x.published_at < v.published_at) ?? null;
  const files = (fv: FakeVersion) => Object.entries(fv.files).map(([path, text]) => ({ path, bytes: text.length, sha256: hash(text), binary: false }));
  const owner = userOf(p.owner);
  const decided = state.plugins.filter((x) => x.owner === p.owner).flatMap((x) => x.versions.map((y) => ({ id: x.id, version: y.version, status: y.status, decided_at: y.review?.decided_at ?? (y.status === "approved" ? y.published_at : null) })));
  const permissionsNow = permissions(v.manifest);
  const permissionsBase = base ? permissions(base.manifest) : null;
  const added = <T,>(now: T[], then: T[] | undefined) => now.filter((x) => !(then ?? []).includes(x));
  const m = v.manifest;
  return json({
    plugin: card(p, v),
    version: { ...versionInfo(v, true), name: String(m.name), summary: String(m.description), icon: typeof m.icon === "string" ? m.icon : null, readme: v.files["README.md"] ?? "", screenshots: (m.screenshots as string[]) ?? [] },
    publisher: {
      id: author(p.owner).id,
      username: owner.username,
      created_at: owner.created_at,
      decisions: { approved: decided.filter((d) => d.status === "approved" || d.status === "yanked").length, rejected: decided.filter((d) => d.status === "rejected").length },
      recent: state.plugins.filter((x) => x.owner === p.owner).flatMap((x) => x.versions.map((y) => ({ plugin_id: x.id, version: y.version, status: y.status, submitted_at: y.published_at, reason: y.review?.reason ?? null }))).sort((a, b) => b.submitted_at - a.submitted_at).slice(0, 10),
    },
    manifest: v.manifest,
    permissions: permissionsNow,
    files: files(v),
    base: base ? { version: base.version, manifest: base.manifest } : null,
    base_files: base ? files(base) : [],
    // (The edge's consentDiff, against nothing for a first listing.)
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
  // submissions/:id/:version/{approve|reject}
  if (parts[0] === "submissions" && parts.length === 4) {
    const p = state.plugins.find((x) => x.id === parts[1]);
    const v = p?.versions.find((x) => x.version === parts[2]);
    if (!p || !v) return notFound();
    if (v.status !== "pending") return fail(409, "conflict", `It's ${v.status}.`);
    if (parts[3] === "approve") {
      v.status = "approved";
      v.review = { state: "approved", decided_at: Date.now() };
      if (p.status === "unlisted") p.status = "listed";
      log(who.user, "version.approve", p.id, v.version, null);
      return json(versionInfo(v, true));
    }
    if (parts[3] === "reject") {
      if (!text("reason") || !text("message")) return fail(400, "bad_request", "A reason and a message for the author are needed.");
      v.status = "rejected";
      v.review = { state: "rejected", reason: text("reason"), message: text("message"), decided_at: Date.now() };
      log(who.user, "version.reject", p.id, v.version, { reason: text("reason") });
      return json(versionInfo(v, true));
    }
    return notFound();
  }
  // plugins/:id/{suspend|restore|feature|transfer} and plugins/:id/versions/:version/remove
  if (parts[0] === "plugins") {
    const p = state.plugins.find((x) => x.id === parts[1]);
    if (!p) return notFound();
    if (parts[2] === "versions" && parts[4] === "remove") {
      const v = p.versions.find((x) => x.version === parts[3]);
      if (!v) return notFound();
      if (!text("reason")) return fail(400, "bad_request", "A reason is needed.");
      v.status = "removed";
      v.review = { state: "removed", reason: text("reason"), decided_at: Date.now() };
      log(who.user, "version.remove", p.id, v.version, { reason: text("reason"), block: input.block === true });
      v.blocked = input.block === true;
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
        p.status = latest(p) ? "listed" : "unlisted";
        log(who.user, "plugin.restore", p.id, null, null);
        return json({ status: p.status });
      case "feature":
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
    if (state.plugins.some((p) => p.id === id && p.versions.some((v) => v.status === "approved" || v.status === "pending"))) return fail(409, "invalid_state", `"${id}" is in use: transfer it instead.`);
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
  state.sessions.set(id, { user: user.id, csrf: crypto.randomUUID().replace(/-/g, "") });
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
    // (The edge's sign-out answers its refusals in its own words, not the store's.)
    if (origin !== url.origin || form?.get("csrf") !== s.csrf) return json({ error: "forbidden", reason: "bad_csrf" }, 403);
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
      return json({ user: { name: s.user.name, email: `${s.user.id.slice(5)}@example.com` }, username: s.user.username, roles: s.user.roles, csrf: s.csrf, expires_at: NOW + DAY, fresh_sign_in: false });
    }
    return notFound();
  }
  if (parts[0] !== "store" || parts[1] !== "v1") return notFound();
  const [, , a, b, c, d, e] = parts;

  if (a === "categories" && method === "GET") return json({ categories: CATEGORIES }, 200, { "cache-control": "public, max-age=300" });
  if (a === "plugins" && b === undefined && method === "GET") return catalog(url);
  if (a === "plugins" && b && c === undefined && method === "GET") {
    const p = state.plugins.find((x) => x.id === b);
    return p && listed(p) ? json(detail(p), 200, { "cache-control": "public, max-age=30" }) : notFound();
  }
  if (a === "plugins" && b && c === "versions" && d === undefined && method === "GET") {
    const p = state.plugins.find((x) => x.id === b);
    return p && listed(p) ? json({ versions: p.versions.filter((v) => v.status === "approved" || v.status === "yanked").map((v) => versionInfo(v, false)) }) : notFound();
  }
  if (a === "plugins" && b && c === "files" && d && parts.length > 6 && method === "GET") return file(request, b, d, parts.slice(6).join("/"));
  if (a === "plugins" && b && c === "versions" && d && ["withdraw", "yank", "unyank"].includes(e ?? "") && method === "POST") return versionAction(request, b, d, e!);
  if (a === "plugins" && b && c === "keys") {
    const s = session(request);
    if (!s) return fail(401, "signed_out");
    const p = state.plugins.find((x) => x.id === b && x.owner === s.user.id);
    if (!p) return notFound();
    if (d === undefined && method === "GET") return json({ keys: state.keys.filter((k) => k.plugin_id === b).map(({ plugin_id: _, ...k }) => k) });
    if (d && method === "DELETE") {
      const who = writer(request);
      if (who instanceof Response) return who;
      const k = state.keys.find((x) => x.plugin_id === b && x.key === d);
      if (!k) return notFound();
      k.revoked_at ??= Date.now();
      return json({ revoked: true });
    }
  }
  if (a === "plugins" && b && c === "reports" && method === "POST") {
    const who = writer(request);
    if (who instanceof Response) return who;
    const p = state.plugins.find((x) => x.id === b);
    if (!p || !listed(p)) return notFound();
    const input = await body(request);
    const reason = String(input.reason ?? "");
    if (!["malware", "misleading", "broken", "copyright", "other"].includes(reason)) return fail(400, "bad_request", "Pick a reason.");
    const text = String(input.text ?? "").trim().slice(0, 2000);
    state.reports.unshift({ id: (state.reports.reduce((n, r) => Math.max(n, r.id), 0) + 1), plugin_id: p.id, version: latest(p)!.version, reason: reason as "other", text, reporter: who.user.id, created_at: Date.now(), status: "open", note: null });
    return json({ ok: true }, 201);
  }
  if (a === "me" && b === "plugins" && method === "GET") {
    const s = session(request);
    return s ? json({ plugins: mine(s.user) }) : fail(401, "signed_out");
  }
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
    if (b === "submissions" && c && d) return submission(c, d);
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
      return json(paged(state.reports.filter((r) => status === "all" || r.status === status).map(reportView), url, "reports"));
    }
    if (b === "reservations" && c === undefined) {
      return json({ reservations: [...state.reservations].sort((x, y) => x.id.localeCompare(y.id)).map((r) => ({ id: r.id, to: r.user ? { username: userOf(r.user).username } : null, note: r.note, created_at: r.created_at })) });
    }
    if (b === "audit") {
      const plugin = url.searchParams.get("plugin");
      return json(paged(state.audit.filter((x) => !plugin || x.plugin_id === plugin).map((x) => ({ ...x, actor: { id: author(x.actor).id, username: userOf(x.actor).username, user_id: x.actor } })), url, "entries"));
    }
  }
  return notFound();
}

/** A page of an admin list, with cursors both ways (amendment 29). */
function paged<T>(all: T[], url: URL, field: string, limit = 20): Record<string, unknown> {
  const cursor = url.searchParams.get("cursor");
  const start = cursor ? Math.max(0, Number(atob(cursor.replace(/-/g, "+").replace(/_/g, "/"))) || 0) : 0;
  const at = (n: number) => btoa(String(n)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return {
    [field]: all.slice(start, start + limit),
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
    try {
      const response = await route(request);
      return new URL(request.url).pathname.startsWith("/store/v1/admin/") ? rotate(request, response) : response;
    } catch (error) {
      return fail(500, "unknown", error instanceof Error ? error.message : String(error));
    }
  },
};
