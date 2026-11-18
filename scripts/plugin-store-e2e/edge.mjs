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
import { createHmac, generateKeyPairSync, randomUUID, sign } from "node:crypto";
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

const workos = async (request) => {
  const url = new URL(request.url);
  if (request.method === "GET" && url.pathname === `/sso/jwks/${CLIENT_ID}`) return reply(200, { keys: [jwk] });
  if (request.method === "POST" && url.pathname === "/user_management/authenticate") {
    const body = await request.json();
    const raw = String(body.grant_type === "authorization_code" ? body.code : body.refresh_token);
    const prefix = body.grant_type === "authorization_code" ? "code:" : "rt:";
    if (!raw.startsWith(prefix)) return reply(400, { error: "invalid_grant", error_description: "Unknown code." });
    const id = raw.slice(prefix.length);
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
const bindings = {
  ...config.vars,
  AUTH_MODE: "workos",
  WORKOS_CLIENT_ID: CLIENT_ID,
  WORKOS_API_KEY: "sk_e2e",
  WORKOS_ISSUER: ISSUER,
  CLOUD_EDGE_URL: "http://127.0.0.1",
  ADMIN_TOKEN: randomUUID(),
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

writeFileSync(readyFile, JSON.stringify({ url, client_id: CLIENT_ID }));
console.error(`edge e2e: serving ${url}`);

const stop = async () => {
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
