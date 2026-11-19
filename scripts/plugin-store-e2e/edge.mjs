// The real edge worker, run locally: the bundle `wrangler deploy --dry-run`
// makes, in Miniflare (the runtime `wrangler dev` uses), with the store's D1
// migrations applied, local R2 and Durable Objects, in memory only. The
// plugin store's end-to-end run uses it (scripts/plugin-store-e2e/run.sh),
// and a site can sit in front of it, reaching it through its real `EDGE`
// service binding.
//
// It's Miniflare rather than `wrangler dev` for one reason: WorkOS. Sign-in
// is a code exchange the edge makes with WorkOS, and only that exchange
// mints the step-up ticket a new signing key or deploy token needs. Miniflare
// takes an outbound service, as the edge's own workerd tests do
// (vitest.workerd.config.ts), so WorkOS is a local fake here: it signs real
// ES256 access tokens and serves their JWKS, and the edge runs in WorkOS
// mode (AUTH_MODE=workos), verifying them as it does in production. Every
// other outbound fetch is refused: nothing leaves this machine.
//
// ── Standalone ──────────────────────────────────────────────────────────────
//
//   cd edge && npm ci && mkdir -p public
//   npx wrangler deploy --dry-run --outdir /tmp/edge-bundle
//   node scripts/plugin-store-e2e/edge.mjs edge /tmp/edge-bundle /tmp/edge.json \
//     [--site-origin https://zeron.sh] [--port N] [--site-port N|off] [--workos-port N]
//
// Runs until SIGTERM / SIGINT. Once serving it writes <ready file>:
//   url          the edge (http://127.0.0.1:<port>): what devices and the
//                CLI reach (ZERON_EDGE_URL / ZERON_STORE_URL)
//   site         the site port (http://127.0.0.1:<port>), or null with
//                --site-port off: a browser's requests to the site, each
//                forwarded as a request to `site_origin` (below)
//   site_origin  the site's origin, the edge's WEB_ORIGIN (default
//                https://site.e2e.test; https, as the edge requires)
//   workos       the WorkOS fake's port, for its AuthKit page (below)
//   admin_token  the edge's ADMIN_TOKEN (the operator's /admin/* routes)
//   client_id    the WorkOS client id the edge is configured with
// Ports are free ones picked at start unless given.
//
// The site port: with nothing in front, it stands in for the site's
// service binding, sending the browser's request to the edge as a request
// to `site_origin` (the edge's web sign-in and store routes answer only
// there). It forwards everything and serves nothing of its own, so a real
// site worker can take its place (startEdge's `workers` and `siteWorker`,
// below), or turn it off and serve the site yourself.
//
// The WorkOS fake:
//   - Web sign-in: the edge's /auth/web/login redirects to
//     https://api.workos.com/user_management/authorize?…; send the browser
//     to the same path and query at `workos` instead, plus `&e2e_user=<id>`
//     (any id, e.g. user_ada). It signs that user in and redirects to the
//     edge's callback with a code good once (the PKCE challenge is checked
//     at the exchange, as WorkOS does). A browser test can route
//     https://api.workos.com/** there.
//   - Devices and tests: the code `code:<user id>` is good for that user
//     at any time; refresh tokens are `rt:<user id>`.
//   - Each user is in its own organization, `org_<user id>`, as its admin.
//   - The first time a user signs in, the fake delivers the signed
//     `user.created` webhook to the edge before the sign-in answers.
//
// A staff account: grant the role with the operator's token, and sign in on
// the site as that user (either order):
//   curl -X POST "$url/admin/roles/grant" -H "authorization: Bearer $admin_token" \
//     -H 'content-type: application/json' -d '{"userId":"user_admin","role":"admin","by":"e2e"}'
// Store admin calls (/store/v1/admin/*) are then the site session's: its
// cookie, `Origin: <site_origin>` and `X-CSRF-Token` from /auth/web/session.
// Staff sessions rotate their cookie: keep the newest Set-Cookie.
//
// ── As a module ─────────────────────────────────────────────────────────────
//
//   import { startEdge } from "…/scripts/plugin-store-e2e/edge.mjs";
//   const edge = await startEdge({
//     edgeDir, bundleDir, siteOrigin: "https://zeron.sh",
//     workers: [{ name: "site", …, serviceBindings: { EDGE: EDGE_NAME } }],
//     siteWorker: "site",
//   });
//
// The edge worker is first in the Miniflare (so `url` is the edge's) and is
// named as deployed (wrangler.jsonc's `name`, `comet-native-edge`, exported
// as EDGE_NAME), so a site's `EDGE` binding finds it by that name. `workers`
// are further Miniflare 5 worker options, run beside it; each gets the same
// refusing outbound service unless it names its own. `siteWorker` names the
// one that serves the site: it's given the route `<site host>/*` (as it's
// routed when deployed) unless it has routes of its own, so the site port's
// requests, and `mf.dispatchFetch(siteOrigin + path)`, reach it; without one
// they reach the edge. `site: false` turns the site port off. It answers
// { mf, name, url, site, siteOrigin, workos, adminToken, clientId,
// grantRole(userId, role), stop() }.

import { createRequire } from "node:module";
import { createHash, createHmac, generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { createServer } from "node:http";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export const EDGE_NAME = "comet-native-edge";
const DEFAULT_SITE_ORIGIN = "https://site.e2e.test";

// ── The WorkOS fake ─────────────────────────────────────────────────────────

const reply = (status, body) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const b64url = (b) => Buffer.from(b).toString("base64url");

/** A WorkOS for one run: `deliver(request)` sends a webhook to the edge. */
const workosFake = (clientId, webhookSecret, deliver) => {
  const issuer = `https://api.workos.com/user_management/${clientId}`;
  const { privateKey, publicKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const kid = "e2e-key";
  const jwk = { ...publicKey.export({ format: "jwk" }), kid, alg: "ES256", use: "sig" };

  const accessToken = (userId) => {
    const now = Math.floor(Date.now() / 1000);
    const header = b64url(JSON.stringify({ alg: "ES256", kid, typ: "JWT" }));
    const payload = b64url(JSON.stringify({ iss: issuer, sub: userId, org_id: `org_${userId}`, sid: randomUUID(), iat: now, exp: now + 3600 }));
    const sig = sign("sha256", Buffer.from(`${header}.${payload}`), { key: privateKey, dsaEncoding: "ieee-p1363" });
    return `${header}.${payload}.${b64url(sig)}`;
  };

  const user = (id) => {
    const at = new Date(0).toISOString();
    return { object: "user", id, email: `${id}@example.test`, first_name: id, last_name: null, email_verified: true, profile_picture_url: null, created_at: at, updated_at: at };
  };

  // WorkOS tells the edge of a new user by webhook (user.created), signed
  // with the webhook secret: delivered the first time a user signs in,
  // before the sign-in answers.
  const known = new Set();
  const userCreated = async (id) => {
    if (known.has(id)) return;
    known.add(id);
    const payload = JSON.stringify({ id: `event_${randomUUID()}`, event: "user.created", data: user(id), created_at: new Date().toISOString() });
    const t = Date.now();
    const v1 = createHmac("sha256", webhookSecret).update(`${t}.${payload}`).digest("hex");
    const res = await deliver("/workos/webhook", { method: "POST", headers: { "content-type": "application/json", "workos-signature": `t=${t}, v1=${v1}` }, body: payload });
    if (!res.ok) console.error(`edge: the user.created webhook for ${id} answered ${res.status}: ${await res.text()}`);
  };

  // Codes the authorize page handed out (a browser's sign-in): code → the
  // user and the PKCE challenge it was asked with. Each is good once.
  const issued = new Map();

  /** AuthKit's authorize page: signs in `e2e_user` and sends the browser
   * back to `redirect_uri` with a code and the `state` it was given. */
  const authorize = (url) => {
    const who = url.searchParams.get("e2e_user");
    const redirect = url.searchParams.get("redirect_uri");
    const state = url.searchParams.get("state");
    if (!who || !redirect || !state) return new Response("authorize needs e2e_user, redirect_uri and state", { status: 400 });
    const code = `web_${randomUUID()}`;
    issued.set(code, { user: who, challenge: url.searchParams.get("code_challenge"), method: url.searchParams.get("code_challenge_method") });
    const back = new URL(redirect);
    back.searchParams.set("code", code);
    back.searchParams.set("state", state);
    return new Response(null, { status: 302, headers: { location: back.toString() } });
  };

  /** A code's user: an authorize page's (its PKCE verifier checked against
   * its challenge), or a device's test code `code:<user>`. */
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

  /** The API, as the edge reaches it (its outbound service). */
  const api = async (request) => {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === `/sso/jwks/${clientId}`) return reply(200, { keys: [jwk] });
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
      await userCreated(id);
      return reply(200, { user: user(id), organization_id: `org_${id}`, access_token: accessToken(id), refresh_token: `rt:${id}` });
    }
    if (request.method === "GET" && url.pathname === "/user_management/organization_memberships") {
      const id = url.searchParams.get("user_id");
      return reply(200, { data: [{ id: `om_${id}`, organization_id: `org_${id}`, organization_name: `${id}'s`, status: "active", role: { slug: "admin" } }], list_metadata: {} });
    }
    const userPath = url.pathname.match(/^\/user_management\/users\/([^/]+)$/);
    if (request.method === "GET" && userPath) return reply(200, user(decodeURIComponent(userPath[1])));
    console.error(`edge: the WorkOS fake has no ${request.method} ${url.pathname}`);
    return reply(404, { code: "not_found", message: "The fake has no such route." });
  };

  /** The browser's side: AuthKit's page. */
  const page = (url) =>
    url.pathname === "/user_management/authorize" ? authorize(url) : new Response("The WorkOS fake serves /user_management/authorize here.", { status: 404 });

  return { issuer, api, page };
};

// ── Node http in front of a fetch handler ───────────────────────────────────

/** Serve `handle(path, init)` on 127.0.0.1:`port`: the request's method,
 * headers (not Host) and body; the answer's redirects and every cookie
 * passed back as sent. */
const serve = async (port, handle) => {
  const server = createServer(async (req, res) => {
    try {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const headers = new Headers();
      for (const [name, value] of Object.entries(req.headers)) {
        if (name !== "host" && value !== undefined) headers.set(name, Array.isArray(value) ? value.join(", ") : value);
      }
      const method = req.method ?? "GET";
      const body = ["GET", "HEAD"].includes(method) ? undefined : Buffer.concat(chunks);
      const answer = await handle(req.url ?? "/", { method, headers, body, redirect: "manual" });
      const out = {};
      answer.headers.forEach((value, name) => {
        if (name !== "set-cookie") out[name] = value;
      });
      const cookies = answer.headers.getSetCookie?.() ?? [];
      if (cookies.length) out["set-cookie"] = cookies;
      res.writeHead(answer.status, out);
      res.end(Buffer.from(await answer.arrayBuffer()));
    } catch (error) {
      console.error(`edge: ${req.method} ${req.url} failed: ${error?.stack ?? error}`);
      res.writeHead(502, { "content-type": "text/plain" }).end("The local edge failed to answer.");
    }
  });
  await new Promise((ok) => server.listen(port, "127.0.0.1", ok));
  return { server, url: `http://127.0.0.1:${server.address().port}` };
};

// ── The edge ────────────────────────────────────────────────────────────────

/** The edge's wrangler.jsonc (its `//` comment lines dropped). */
const readConfig = (edgeDir) =>
  JSON.parse(
    readFileSync(join(edgeDir, "wrangler.jsonc"), "utf8")
      .split("\n")
      .filter((line) => !line.trim().startsWith("//"))
      .join("\n")
  );

export async function startEdge({
  edgeDir,
  bundleDir,
  siteOrigin = DEFAULT_SITE_ORIGIN,
  port = 0,
  sitePort = 0,
  workosPort = 0,
  workers = [],
  siteWorker,
  site: sitePortOn = true,
}) {
  edgeDir = resolve(edgeDir);
  bundleDir = resolve(bundleDir);
  if (new URL(siteOrigin).protocol !== "https:" || new URL(siteOrigin).origin !== siteOrigin) {
    throw new Error(`the site origin is an https origin, like https://zeron.sh (not ${siteOrigin})`);
  }
  const require = createRequire(join(edgeDir, "package.json"));
  const { Miniflare } = require("miniflare");
  const config = readConfig(edgeDir);
  if (config.name !== EDGE_NAME) console.error(`edge: wrangler.jsonc names it ${config.name}; it runs here as ${EDGE_NAME}`);
  if (siteWorker !== undefined && !workers.some((w) => w.name === siteWorker)) throw new Error(`no worker named ${siteWorker} among workers`);
  const siteRoute = `${new URL(siteOrigin).host}/*`;

  const clientId = "client_e2e";
  const webhookSecret = randomUUID();
  const adminToken = randomUUID();
  let mf;
  const fake = workosFake(clientId, webhookSecret, (path, init) => mf.dispatchFetch(`${url}${path}`, init));
  const outbound = async (request) => {
    const host = new URL(request.url).hostname;
    if (host === "api.workos.com") return fake.api(request);
    console.error(`edge: refused an outbound fetch to ${host}`);
    return new Response("This local run reaches nothing outside this machine.", { status: 502 });
  };

  // Its vars as deployed, then what this run sets.
  const bindings = {
    ...config.vars,
    AUTH_MODE: "workos",
    WORKOS_CLIENT_ID: clientId,
    WORKOS_API_KEY: "sk_e2e",
    WORKOS_ISSUER: fake.issuer,
    WORKOS_WEBHOOK_SECRET: webhookSecret,
    CLOUD_EDGE_URL: "http://127.0.0.1",
    ADMIN_TOKEN: adminToken,
    WEB_ORIGIN: siteOrigin,
  };
  const edge = {
    name: EDGE_NAME,
    modules: true,
    scriptPath: join(bundleDir, "index.js"),
    modulesRoot: bundleDir,
    modulesRules: [{ type: "Text", include: ["**/*.sh"] }],
    compatibilityDate: config.compatibility_date,
    compatibilityFlags: config.compatibility_flags,
    bindings,
    durableObjects: Object.fromEntries(config.durable_objects.bindings.map((b) => [b.name, { className: b.class_name, useSQLite: true }])),
    workflows: Object.fromEntries(config.workflows.map((w) => [w.binding, { name: `local-${w.name}`, className: w.class_name }])),
    d1Databases: Object.fromEntries(config.d1_databases.map((d) => [d.binding, `local-${d.database_name}`])),
    r2Buckets: config.r2_buckets.map((b) => b.binding),
    ratelimits: Object.fromEntries(config.ratelimits.map((r) => [r.name, { namespace_id: r.namespace_id, simple: { limit: 100000, period: 60 } }])),
    serviceBindings: { VAULT: async () => new Response("No vault in this run.", { status: 503 }) },
    outboundService: outbound,
  };
  mf = new Miniflare({
    host: "127.0.0.1",
    port,
    // (The edge first: the fallback for every request no route claims.)
    workers: [
      edge,
      ...workers.map((w) => ({ outboundService: outbound, ...w, ...(w.name === siteWorker && !w.routes ? { routes: [siteRoute] } : {}) })),
    ],
  });
  const url = (await mf.ready).toString().replace(/\/$/, "");

  // The store's schema, as the deploy job applies it.
  for (const d of config.d1_databases) {
    const db = await mf.getD1Database(d.binding, EDGE_NAME);
    const migrations = join(edgeDir, d.migrations_dir);
    for (const file of readdirSync(migrations).filter((f) => f.endsWith(".sql")).sort()) {
      // D1's exec takes one statement per line: run them one by one.
      for (const statement of splitStatements(readFileSync(join(migrations, file), "utf8"))) await db.prepare(statement).run();
    }
  }

  const authkit = await serve(workosPort, async (path) => fake.page(new URL(path, "http://workos")));
  // (dispatchFetch, not getWorker(…).fetch: Miniflare's proxy refuses a
  // request whose Origin isn't its own, and a browser on the site sends
  // the site's.)
  const site = sitePortOn ? await serve(sitePort, (path, init) => mf.dispatchFetch(`${siteOrigin}${path}`, init)) : null;

  const grantRole = async (userId, role = "admin") => {
    const res = await mf.dispatchFetch(`${url}/admin/roles/grant`, {
      method: "POST",
      headers: { authorization: `Bearer ${adminToken}`, "content-type": "application/json" },
      body: JSON.stringify({ userId, role, by: "local" }),
    });
    if (!res.ok) throw new Error(`granting ${role} to ${userId} answered ${res.status}: ${await res.text()}`);
    return res.json();
  };

  const stop = async () => {
    authkit.server.close();
    site?.server.close();
    await mf.dispose();
  };
  return { mf, name: EDGE_NAME, url, site: site?.url ?? null, siteOrigin, workos: authkit.url, adminToken, clientId, grantRole, stop };
}

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

// ── The command ─────────────────────────────────────────────────────────────

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const usage = "usage: node edge.mjs <edge dir> <bundle dir> <ready file> [--site-origin https://…] [--port N] [--site-port N|off] [--workos-port N]";
  const args = process.argv.slice(2);
  const positional = [];
  const flags = {};
  for (let i = 0; i < args.length; i++) {
    if (args[i].startsWith("--")) flags[args[i].slice(2)] = args[++i];
    else positional.push(args[i]);
  }
  const number = (name) => {
    if (flags[name] === undefined) return 0;
    const n = Number(flags[name]);
    if (!Number.isInteger(n) || n < 0 || n > 65535) {
      console.error(`--${name} is a port number\n${usage}`);
      process.exit(2);
    }
    return n;
  };
  const known = new Set(["site-origin", "port", "site-port", "workos-port"]);
  const unknown = Object.keys(flags).filter((f) => !known.has(f) || flags[f] === undefined);
  if (positional.length !== 3 || unknown.length) {
    console.error(usage);
    process.exit(2);
  }
  const [edgeDir, bundleDir, readyFile] = positional.map((p) => resolve(p));
  const off = flags["site-port"] === "off";
  let edge;
  try {
    edge = await startEdge({
      edgeDir,
      bundleDir,
      siteOrigin: flags["site-origin"] ?? DEFAULT_SITE_ORIGIN,
      port: number("port"),
      sitePort: off ? 0 : number("site-port"),
      workosPort: number("workos-port"),
      site: !off,
    });
  } catch (error) {
    console.error(`edge: ${error?.message ?? error}`);
    process.exit(1);
  }
  writeFileSync(
    readyFile,
    JSON.stringify({ url: edge.url, site: edge.site, site_origin: edge.siteOrigin, workos: edge.workos, admin_token: edge.adminToken, client_id: edge.clientId })
  );
  console.error(`edge: serving ${edge.url}; the site ${edge.site ?? "port is off"} (as ${edge.siteOrigin}); WorkOS's page at ${edge.workos}`);
  const stop = async () => {
    await edge.stop();
    process.exit(0);
  };
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
}
