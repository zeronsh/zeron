// The real edge worker, run locally for the plugin store's end-to-end run
// (scripts/plugin-store-e2e/run.sh): the bundle `wrangler deploy --dry-run`
// makes, in Miniflare (the runtime `wrangler dev` uses), with the store's D1
// migrations applied, local R2, in memory only.
//
// It's Miniflare rather than `wrangler dev` for one reason: WorkOS. Sign-in
// is a code exchange the edge makes with WorkOS, and only that exchange
// mints the step-up ticket a new signing key or deploy key needs. Miniflare
// takes an outbound service, as the edge's own workerd tests do
// (vitest.workerd.config.ts), so WorkOS is a local fake here: it signs real
// ES256 access tokens and serves their JWKS, and the edge runs in WorkOS
// mode, verifying them as it does in production. Every other outbound fetch
// is refused: nothing leaves this machine.
//
// Usage: node edge.mjs <edge dir> <bundle dir> <ready file>
//   Writes {"url": "http://127.0.0.1:<port>"} to <ready file> once serving;
//   runs until SIGTERM / SIGINT.
//
// Test sign-ins: a WorkOS code is `code:<user id>` (any id, e.g. `user_ada`);
// the fake answers with that user, in the organization `org_<user id>`.
// Store admins (the edge-store lane's stand-in session): STORE_ADMIN_USERS.

import { createRequire } from "node:module";
import { createHash, createHmac, generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { createServer } from "node:http";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const [edgeDir, bundleDir, readyFile] = process.argv.slice(2).map((p) => resolve(p));
if (process.argv.length < 5) {
  console.error("usage: node edge.mjs <edge dir> <bundle dir> <ready file>");
  process.exit(2);
}
const require = createRequire(join(edgeDir, "package.json"));
const { Miniflare } = require("miniflare");

// ── The WorkOS fake ─────────────────────────────────────────────────────────

const CLIENT_ID = "client_e2e";
const ISSUER = `https://api.workos.com/user_management/${CLIENT_ID}`;
const { privateKey, publicKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
const KID = "e2e-key";
const jwk = { ...publicKey.export({ format: "jwk" }), kid: KID, alg: "ES256", use: "sig" };
const b64url = (b) => Buffer.from(b).toString("base64url");

const accessToken = (userId) => {
  const now = Math.floor(Date.now() / 1000);
  const header = b64url(JSON.stringify({ alg: "ES256", kid: KID, typ: "JWT" }));
  const payload = b64url(JSON.stringify({ iss: ISSUER, sub: userId, org_id: `org_${userId}`, sid: randomUUID(), iat: now, exp: now + 3600 }));
  const sig = sign("sha256", Buffer.from(`${header}.${payload}`), { key: privateKey, dsaEncoding: "ieee-p1363" });
  return `${header}.${payload}.${b64url(sig)}`;
};

const user = (id) => {
  const at = new Date(0).toISOString();
  return { object: "user", id, email: `${id}@example.test`, first_name: id, last_name: null, email_verified: true, profile_picture_url: null, created_at: at, updated_at: at };
};

// WorkOS tells the edge of a new user by webhook (user.created), signed
// with the webhook secret: delivered here the first time a user signs in,
// before the sign-in answers.
const WEBHOOK_SECRET = randomUUID();
const known = new Set();
const deliverUserCreated = async (id) => {
  if (known.has(id)) return;
  known.add(id);
  const payload = JSON.stringify({ id: `event_${randomUUID()}`, event: "user.created", data: user(id), created_at: new Date().toISOString() });
  const t = Date.now();
  const v1 = createHmac("sha256", WEBHOOK_SECRET).update(`${t}.${payload}`).digest("hex");
  const res = await mf.dispatchFetch(`${url}/workos/webhook`, { method: "POST", headers: { "content-type": "application/json", "workos-signature": `t=${t}, v1=${v1}` }, body: payload });
  if (!res.ok) console.error(`edge e2e: the user.created webhook for ${id} answered ${res.status}: ${await res.text()}`);
};

const reply = (status, body) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

// Codes the authorize page handed out (a browser's sign-in): code → the user
// and the PKCE challenge it was asked with. Each is good once.
const issued = new Map();

/** AuthKit's authorize page, as a browser reaches it through the site port:
 * signs in `e2e_user` and sends the browser back to `redirect_uri` with a
 * code and the `state` it was given. */
const authorize = (url) => {
  const user = url.searchParams.get("e2e_user");
  const redirect = url.searchParams.get("redirect_uri");
  const state = url.searchParams.get("state");
  if (!user || !redirect || !state) return new Response("authorize needs e2e_user, redirect_uri and state", { status: 400 });
  const code = `web_${randomUUID()}`;
  issued.set(code, { user, challenge: url.searchParams.get("code_challenge"), method: url.searchParams.get("code_challenge_method") });
  const back = new URL(redirect);
  back.searchParams.set("code", code);
  back.searchParams.set("state", state);
  return new Response(null, { status: 302, headers: { location: back.toString() } });
};

/** A code's user: an authorize page's (its PKCE verifier checked against
 * its challenge, as WorkOS does), or a device's test code `code:<user>`. */
const userOfCode = (code, verifier) => {
  const asked = issued.get(code);
  if (asked) {
    issued.delete(code);
    if (asked.challenge) {
      const proof = verifier && createHash("sha256").update(verifier).digest("base64url");
      if (asked.method !== "S256" || proof !== asked.challenge) return { error: "invalid_grant", error_description: "PKCE verification failed." };
    }
    return { user: asked.user };
  }
  return code.startsWith("code:") ? { user: code.slice(5) } : { error: "invalid_grant", error_description: "Unknown code." };
};

const workos = async (request) => {
  const url = new URL(request.url);
  if (request.method === "GET" && url.pathname === `/sso/jwks/${CLIENT_ID}`) return reply(200, { keys: [jwk] });
  if (request.method === "POST" && url.pathname === "/user_management/sessions/revoke") return reply(200, {});
  if (request.method === "POST" && url.pathname === "/user_management/authenticate") {
    const body = await request.json();
    let id;
    if (body.grant_type === "authorization_code") {
      const found = userOfCode(String(body.code), body.code_verifier);
      if (found.error) return reply(400, found);
      id = found.user;
    } else {
      const raw = String(body.refresh_token);
      if (!raw.startsWith("rt:")) return reply(400, { error: "invalid_grant", error_description: "Unknown refresh token." });
      id = raw.slice(3);
    }
    await deliverUserCreated(id);
    return reply(200, { user: user(id), organization_id: `org_${id}`, access_token: accessToken(id), refresh_token: `rt:${id}` });
  }
  if (request.method === "GET" && url.pathname === "/user_management/organization_memberships") {
    const id = url.searchParams.get("user_id");
    return reply(200, { data: [{ id: `om_${id}`, organization_id: `org_${id}`, organization_name: `${id}'s`, status: "active", role: { slug: "admin" } }], list_metadata: {} });
  }
  const userPath = url.pathname.match(/^\/user_management\/users\/([^/]+)$/);
  if (request.method === "GET" && userPath) return reply(200, user(decodeURIComponent(userPath[1])));
  console.error(`edge e2e: WorkOS fake has no ${request.method} ${url.pathname}`);
  return reply(404, { code: "not_found", message: "The fake has no such route." });
};

const outbound = async (request) => {
  const host = new URL(request.url).hostname;
  if (host === "api.workos.com") return workos(request);
  console.error(`edge e2e: refused an outbound fetch to ${host}`);
  return new Response("This end-to-end run reaches nothing outside this machine.", { status: 502 });
};

// ── The worker ──────────────────────────────────────────────────────────────

// Its vars as deployed, then what this run sets.
const config = JSON.parse(
  readFileSync(join(edgeDir, "wrangler.jsonc"), "utf8")
    .split("\n")
    .filter((line) => !line.trim().startsWith("//"))
    .join("\n")
);
// The web site's origin (https, as the edge requires of it). Requests to it
// are served by the site port below, which forwards them to the worker as
// the site's own (staging: the Astro site, over its service binding).
const SITE = "https://site.e2e.test";
const ADMIN_TOKEN = randomUUID();
const bindings = {
  ...config.vars,
  AUTH_MODE: "workos",
  WORKOS_CLIENT_ID: CLIENT_ID,
  WORKOS_API_KEY: "sk_e2e",
  WORKOS_ISSUER: ISSUER,
  CLOUD_EDGE_URL: "http://127.0.0.1",
  ADMIN_TOKEN,
  WEB_ORIGIN: SITE,
  WORKOS_WEBHOOK_SECRET: WEBHOOK_SECRET,
  STORE_ADMIN_USERS: process.env.STORE_ADMIN_USERS ?? "user_admin"
};

const mf = new Miniflare({
  modules: true,
  scriptPath: join(bundleDir, "index.js"),
  modulesRoot: bundleDir,
  modulesRules: [{ type: "Text", include: ["**/*.sh"] }],
  compatibilityDate: config.compatibility_date,
  compatibilityFlags: config.compatibility_flags,
  host: "127.0.0.1",
  port: 0,
  bindings,
  durableObjects: Object.fromEntries(config.durable_objects.bindings.map((b) => [b.name, { className: b.class_name, useSQLite: true }])),
  workflows: Object.fromEntries(config.workflows.map((w) => [w.binding, { name: `e2e-${w.name}`, className: w.class_name }])),
  d1Databases: Object.fromEntries(config.d1_databases.map((d) => [d.binding, `e2e-${d.database_name}`])),
  r2Buckets: config.r2_buckets.map((b) => b.binding),
  ratelimits: Object.fromEntries(config.ratelimits.map((r) => [r.name, { namespace_id: r.namespace_id, simple: { limit: 100000, period: 60 } }])),
  serviceBindings: { VAULT: async () => new Response("No vault in this run.", { status: 503 }) },
  outboundService: outbound
});

const url = (await mf.ready).toString().replace(/\/$/, "");
// (The webhook above is sent to it.)

// The store's schema, as the deploy job applies it.
const db = await mf.getD1Database("STORE_DB");
const migrations = join(edgeDir, config.d1_databases[0].migrations_dir);
for (const file of readdirSync(migrations).filter((f) => f.endsWith(".sql")).sort()) {
  const sql = readFileSync(join(migrations, file), "utf8");
  // D1's exec takes one statement per line: run them one by one.
  for (const statement of splitStatements(sql)) await db.prepare(statement).run();
}

// The site port: what a browser on the site reaches. `/__workos/authorize`
// is AuthKit's page (the fake's); everything else goes to the worker as a
// request to the site's origin, redirects and cookies passed back as sent.
const siteServer = createServer(async (req, res) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const path = req.url ?? "/";
  let answer;
  if (path.startsWith("/__workos/authorize")) {
    answer = authorize(new URL(path, "http://site"));
  } else {
    const headers = new Headers();
    for (const [name, value] of Object.entries(req.headers)) {
      if (name !== "host" && value !== undefined) headers.set(name, Array.isArray(value) ? value.join(", ") : value);
    }
    const body = ["GET", "HEAD"].includes(req.method ?? "GET") ? undefined : Buffer.concat(chunks);
    answer = await mf.dispatchFetch(`${SITE}${path}`, { method: req.method, headers, body, redirect: "manual" });
  }
  const out = {};
  answer.headers.forEach((value, name) => {
    if (name !== "set-cookie") out[name] = value;
  });
  const cookies = answer.headers.getSetCookie?.() ?? [];
  if (cookies.length) out["set-cookie"] = cookies;
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
});
await new Promise((ok) => siteServer.listen(0, "127.0.0.1", ok));
const site = `http://127.0.0.1:${siteServer.address().port}`;

writeFileSync(readyFile, JSON.stringify({ url, site, site_origin: SITE, admin_token: ADMIN_TOKEN, client_id: CLIENT_ID }));
console.error(`edge e2e: serving ${url}, the site at ${site}`);

const stop = async () => {
  siteServer.close();
  await mf.dispose();
  process.exit(0);
};
process.on("SIGTERM", stop);
process.on("SIGINT", stop);

/** SQL statements, split on `;` at the end of a statement (outside quotes,
 * comments dropped; a trigger's BEGIN … END kept whole). */
function splitStatements(sql) {
  const out = [];
  let current = "";
  let depth = 0;
  for (const raw of sql.split("\n")) {
    const line = raw.replace(/--.*$/, "");
    if (!line.trim()) continue;
    current += line + "\n";
    if (/\bBEGIN\b/i.test(line)) depth++;
    if (/^\s*END\b/i.test(line)) depth--;
    if (depth === 0 && line.trim().endsWith(";")) {
      out.push(current.trim());
      current = "";
    }
  }
  if (current.trim()) out.push(current.trim());
  return out;
}
