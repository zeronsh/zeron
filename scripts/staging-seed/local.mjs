#!/usr/bin/env node
// The seed, run against a local edge and checked: the real edge worker in
// Miniflare with its WorkOS fake (scripts/plugin-store-e2e/edge.mjs), six
// real `zeron headless` engines, one per author, each signed in through
// the fake as a device signs in, its username claimed. That is what the
// sessions from outside are on staging. seed.mjs then runs exactly as it
// would there: its first pass publishes the first versions and stops for
// deploy tokens, which each author makes with the CLI (`zeron plugin token
// create --all`, the fresh sign-in it asks for played here), and its
// second run publishes the rest. The catalog is then checked through the
// store's public routes and each author's own.
//
//   node scripts/staging-seed/local.mjs --zeron <path to a zeron build> [--edge-dir edge] [--keep]
//
// Prints `PASS …` / `FAIL …`; exits non-zero on a failure. Leaves nothing
// running, keeps nothing (a scratch folder for everything; --keep keeps it).

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { PLUGINS } from "./catalog.mjs";

/** This run's authors, by slot (a real one's are the logins file's). */
const USERNAMES = Object.fromEntries([...new Set(PLUGINS.map((p) => p.slot))].map((slot) => [slot, `seeds-${slot}`]));

const VERSIONS = PLUGINS.reduce((n, p) => n + p.versions.length, 0);
const YANKS = PLUGINS.reduce((n, p) => n + p.versions.filter((v) => v.yank).length, 0);

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i < 0 ? undefined : args[i + 1];
};
const zeron = resolve(flag("--zeron") ?? (console.error("local: give --zeron <path to a zeron build>"), process.exit(2)));
const edgeDir = resolve(flag("--edge-dir") ?? join(root, "edge"));
const keep = args.includes("--keep");
const work = mkdtempSync(join(tmpdir(), "zeron-seed-local-"));

const failures = [];
const check = (what, ok, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}${detail ? ` [${typeof detail === "string" ? detail : JSON.stringify(detail)}]` : ""}`);
  if (!ok) failures.push(what);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const freePort = () =>
  new Promise((res) => {
    const s = createServer().listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => res(port));
    });
  });
const run = (cmd, argv, opts = {}) =>
  new Promise((res) => {
    const child = spawn(cmd, argv, { stdio: ["ignore", "pipe", "pipe"], ...opts });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    opts.onErr && child.stderr.on("data", (d) => opts.onErr(String(d)));
    child.on("close", (code) => res({ code, out, err }));
  });

const engines = [];
let edge;
const stop = async () => {
  for (const e of engines) e.child.kill();
  await edge?.stop?.();
  if (keep) console.log(`scratch kept: ${work}`);
  else rmSync(work, { recursive: true, force: true });
};

try {
  // ── The edge ──
  mkdirSync(join(edgeDir, "public"), { recursive: true });
  const bundled = await run("npx", ["wrangler", "deploy", "--dry-run", "--outdir", join(work, "bundle")], { cwd: edgeDir });
  if (bundled.code !== 0) throw new Error(`the edge didn't bundle:\n${bundled.err}`);
  const { startEdge } = await import(join(root, "scripts/plugin-store-e2e/edge.mjs"));
  edge = await startEdge({ edgeDir, bundleDir: join(work, "bundle"), site: false });
  console.log(`== the edge serves ${edge.url}`);
  const WebSocket = createRequire(join(edgeDir, "package.json"))("ws");

  /** One author's device: a `zeron headless` of its own, and its IPC. */
  const device = async (slot) => {
    const home = join(work, `author-${slot}`, "home");
    const data = join(work, `author-${slot}`, "data");
    mkdirSync(home, { recursive: true });
    mkdirSync(data, { recursive: true });
    const port = await freePort();
    const env = { PATH: process.env.PATH, HOME: home, ZERON_DATA_DIR: data, ZERON_EDGE_URL: edge.url, ZERON_WORKOS_CLIENT_ID: edge.clientId, ZERON_IPC_PORT: String(port) };
    const child = spawn(zeron, ["headless"], { env: { ...env, RUST_LOG: "warn", ZERON_CALLBACK_PORT: String(await freePort()) }, stdio: "ignore" });
    engines.push({ child });
    let ws;
    for (let t = 0; t < 240 && !ws; t++) {
      await sleep(250);
      if (!existsSync(join(data, "ipc-token"))) continue;
      const token = readFileSync(join(data, "ipc-token"), "utf8").trim();
      ws = await new Promise((res) => {
        const s = new WebSocket(`ws://127.0.0.1:${port}`, { headers: { authorization: `Bearer ${token}` } });
        s.once("open", () => res(s));
        s.once("error", () => res(undefined));
      });
    }
    if (!ws) throw new Error(`author ${slot}'s Zeron never answered`);
    let next = 1;
    const waiting = new Map();
    ws.on("message", (raw) => {
      const frame = JSON.parse(String(raw));
      const done = waiting.get(frame.id);
      if (!done) return;
      waiting.delete(frame.id);
      frame.err !== undefined ? done.reject(new Error(frame.err)) : done.resolve(frame.ok);
    });
    const call = (method, params = {}) =>
      new Promise((res, rej) => {
        const id = next++;
        waiting.set(id, { resolve: res, reject: rej });
        ws.send(JSON.stringify({ id, method, params }));
      });
    return { slot, user: `user_seed${slot}`, home, data, port, env, call, ws };
  };

  // ── The authors: signed in, a username each, a deploy token each ──
  const authors = [];
  for (const slot of [1, 2, 3, 4, 5, 6]) {
    const d = await device(slot);
    const { url } = await d.call("SignInHeadless");
    const state = new URL(url).searchParams.get("state");
    await d.call("CompleteSignIn", { code: `${state}.code:${d.user}` });
    for (let t = 0; t < 120; t++) {
      if ((await d.call("PluginSigningKey"))?.registered === true) break;
      await sleep(250);
    }
    const bearer = (await d.call("AccessToken")).token;
    const named = await fetch(`${edge.url}/username`, { method: "POST", headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json" }, body: JSON.stringify({ username: USERNAMES[slot] }) });
    if (!named.ok) throw new Error(`author ${slot} couldn't claim ${USERNAMES[slot]}: ${named.status} ${await named.text()}`);
    d.bearer = bearer;
    authors.push(d);
    console.log(`== author ${slot}: ${USERNAMES[slot]}, signed in`);
  }

  /** `zeron plugin token create` on `d`'s Zeron, the fresh sign-in it asks
   * for played as the person would (their browser, back to its callback). */
  async function makeToken(d, scope, name) {
    let played = false;
    const made = await run(zeron, ["plugin", "token", "create", "--name", name, ...scope], {
      env: { ...d.env, RUST_LOG: "warn" },
      onErr: async (text) => {
        const url = text.match(/https?:\/\/\S+/)?.[0];
        if (!url || played) return;
        played = true;
        const at = new URL(url);
        const back = new URL(at.searchParams.get("redirect_uri"));
        back.searchParams.set("code", `code:${d.user}`);
        back.searchParams.set("state", at.searchParams.get("state"));
        await fetch(back, { redirect: "manual" });
      }
    });
    const token = made.out.match(/zdt_[0-9A-Za-z]{49}/)?.[0];
    if (made.code !== 0 || !token) throw new Error(`author ${d.slot}'s token: ${made.code} ${made.err}`);
    return token;
  }

  // ── The seed, as it runs on staging ──
  const accounts = {
    edge: edge.url,
    workos_client_id: edge.clientId,
    zeron,
    authors: authors.map((d) => ({ slot: d.slot, home: d.home, data_dir: d.data, ipc_port: d.port, token_env: `SEED_TOKEN_${d.slot}` }))
  };
  writeFileSync(join(work, "accounts.json"), JSON.stringify(accounts, null, 2));
  const seed = (env) => run(process.execPath, [join(here, "seed.mjs"), "--accounts", join(work, "accounts.json"), "--edge", edge.url], { env: { ...process.env, SEED_ALLOWED_EDGES: edge.url, ...env }, onErr: (t) => process.stderr.write(t) });
  // A Zeron signed in elsewhere (as a wrong accounts file would name): refused
  // before it publishes anything.
  writeFileSync(join(work, "elsewhere.json"), JSON.stringify({ ...JSON.parse(readFileSync(join(work, "accounts.json"), "utf8")), workos_client_id: "client_elsewhere" }));
  const elsewhere = await run(process.execPath, [join(here, "seed.mjs"), "--accounts", join(work, "elsewhere.json"), "--edge", edge.url], { env: { ...process.env, SEED_ALLOWED_EDGES: edge.url } });
  check("a Zeron signed in to another WorkOS publishes nothing (exit 2)", elsewhere.code === 2 && elsewhere.err.includes("another WorkOS") && !elsewhere.out.includes("published"), elsewhere.err.trim().split("\n")[0]);
  const firstPass = await seed({});
  process.stdout.write(firstPass.out);
  check("the first pass publishes the 30 first versions and stops for tokens (exit 3)", firstPass.code === 3 && (firstPass.out.match(/^published .* \(device\)$/gm) ?? []).length === 30, `exit ${firstPass.code}`);
  for (const d of authors) d.token = await makeToken(d, ["--all"], "Seed CI");
  const tokens = Object.fromEntries(authors.map((d) => [`SEED_TOKEN_${d.slot}`, d.token]));
  const seeded = await seed(tokens);
  process.stdout.write(seeded.out);
  check("the second run publishes the rest", seeded.code === 0 && seeded.out.includes(`done: ${VERSIONS - PLUGINS.length + YANKS} written, ${PLUGINS.length} already there`), seeded.out.trim().split("\n").pop());

  // ── What the store shows ──
  const get = async (path, bearer) => {
    const res = await fetch(`${edge.url}${path}`, bearer ? { headers: { authorization: `Bearer ${bearer}` } } : {});
    return { status: res.status, type: res.headers.get("content-type"), body: res.headers.get("content-type")?.includes("json") ? await res.json() : await res.arrayBuffer() };
  };
  const stats = (await get("/store/v1/stats")).body;
  check("30 plugins listed", stats.plugins === 30, stats);
  const first = (await get("/store/v1/plugins")).body;
  check("the first page holds 24, with a next page", first.items?.length === 24 && Boolean(first.next_cursor) && first.total === 30, { items: first.items?.length, total: first.total });
  const second = (await get(`/store/v1/plugins?cursor=${encodeURIComponent(first.next_cursor)}`)).body;
  check("the second page the other 6, and no third", second.items?.length === 6 && !second.next_cursor, { items: second.items?.length });
  const ids = new Set([...first.items, ...second.items].map((p) => p.id));
  check("the two pages are the 30, each once", ids.size === 30 && PLUGINS.every((p) => ids.has(p.id)));
  for (const [slot, username] of Object.entries(USERNAMES)) {
    const mine = (await get(`/store/v1/plugins?author=${username}`)).body;
    check(`@${username} has 5`, mine.total === 5, mine.total);
  }
  const categories = (await get("/store/v1/categories")).body.categories;
  check("every category has plugins", categories.length === 8 && categories.every((c) => c.count > 0), categories.map((c) => `${c.id}:${c.count}`).join(" "));
  const found = (await get("/store/v1/plugins?q=git")).body.items.map((p) => p.id);
  check("a search finds by keyword", ["branch-radar", "commit-lens", "diff-glance", "tag-keeper"].every((id) => found.includes(id)), found.join(","));
  const kw = (await get("/store/v1/plugins?keyword=focus")).body.items.map((p) => p.id).sort();
  check("a keyword filters", JSON.stringify(kw) === JSON.stringify(["focus-timer", "meeting-clock", "quiet-hours"]), kw.join(","));

  // Every version as the catalog says: from a device signed, from a token not.
  let versionsRight = true;
  const versionsOf = {};
  for (const p of PLUGINS) {
    const listed = (await get(`/store/v1/plugins/${p.id}/versions`)).body.versions ?? [];
    versionsOf[p.id] = new Map(listed.map((v) => [v.version, v]));
    for (const v of p.versions) {
      const got = versionsOf[p.id].get(v.v);
      const ok = got && got.via === (v.by === "device" ? "device" : "deploy_token") && (v.by === "device" ? typeof got.key === "string" : got.key === null);
      if (!ok) {
        versionsRight = false;
        console.log(`  ${p.id} ${v.v}: ${JSON.stringify(got && { via: got.via, key: got.key, status: got.status })}`);
      }
    }
  }
  check(`all ${VERSIONS} versions, devices' signed and tokens' unsigned`, versionsRight, VERSIONS);
  const br = versionsOf["branch-radar"];
  check("a chain published unverified: each older one superseded", br.get("1.0.0")?.yanked_by === "superseded" && br.get("1.0.1")?.yanked_by === "superseded" && br.get("1.0.2")?.status === "listed", [...br.values()].map((v) => `${v.version}:${v.status}/${v.yanked_by}`).join(" "));
  const rc = versionsOf["quiet-hours"].get("1.2.0-rc.1");
  const release = versionsOf["quiet-hours"].get("1.1.0");
  const card = (await get("/store/v1/plugins/quiet-hours")).body;
  check(
    "its author yanked quiet-hours 1.2.0-rc.1, and 1.1.0 is the one listed",
    rc?.status === "yanked" && rc?.yanked_by === "author" && release?.status === "listed" && card.version === "1.1.0",
    `${rc?.status}/${rc?.yanked_by}, ${release?.status}, listed ${card.version}`
  );
  const beta = versionsOf["palette-swap"].get("2.0.0-beta.1");
  check("palette-swap 2.0.0-beta.1 is a pre-release", beta?.prerelease === true, beta && `${beta.status}, prerelease ${beta.prerelease}`);

  // Pictures: screenshots served as PNGs, icons as SVGs, a picture the store can't show warned of.
  const detail = (await get("/store/v1/plugins/standup-notes")).body;
  const shot = detail.screenshots?.[0];
  const pic = shot && (await get(`/store/v1/plugins/standup-notes/files/1.3.0/${shot}`));
  check("a screenshot is served as a PNG", pic?.status === 200 && pic.type === "image/png" && pic.body.byteLength > 1000, pic && `${pic.status} ${pic.type} ${pic.body.byteLength}`);
  const icon = await get(`/store/v1/plugins/standup-notes/files/1.3.0/icon.svg`);
  check("an icon is served as an SVG", icon.status === 200 && icon.type?.startsWith("image/svg+xml"), `${icon.status} ${icon.type}`);
  check("meeting-clock lists no screenshots", ((await get("/store/v1/plugins/meeting-clock")).body.screenshots ?? []).length === 0);
  const own3 = (await get("/store/v1/me/plugins", authors[2].bearer)).body.plugins.find((p) => p.id === "compact-sidebar");
  const warned = own3?.versions?.[0]?.checks?.some((c) => c.id === "picture_unshown");
  check("compact-sidebar's GIF is warned of (picture_unshown)", warned === true);

  // An update that asks for more.
  const lp = versionsOf["link-preview"].get("1.0.0");
  const offered = await (await fetch(`${edge.url}/store/v1/updates`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ zeron: "0.4.0", plugins: [{ id: "link-preview", version: "1.0.0", hash: lp?.hash }] }) })).json();
  const offer = offered.updates?.[0];
  check("link-preview 1.1.0 is offered, adding network", offer?.version === "1.1.0" && offer?.added?.includes("network") && offer?.key === null, offer && `${offer.version} +${offer.added}`);

  // The notices: the owner sees which token published which version.
  const own1 = (await get("/store/v1/me/plugins", authors[0].bearer)).body.plugins.find((p) => p.id === "standup-notes");
  const byToken = own1?.versions?.find((v) => v.version === "1.3.0");
  check("the owner's view names the token on its versions", byToken?.deploy_token?.name === "Seed CI" && byToken?.deploy_token?.short === authors[0].token.slice(4, 12), byToken?.deploy_token);

  // A token for two plugins refuses a third; then a second run changes nothing.
  const two = await makeToken(authors[1], ["--plugins", "branch-radar,commit-lens"], "Two plugins");
  const dir = join(authors[1].data, "plugins", "diff-glance");
  const ci = join(work, "ci-diff-glance");
  cpSync(dir, ci, { recursive: true });
  const m = JSON.parse(readFileSync(join(ci, "zeron-plugin.json"), "utf8"));
  writeFileSync(join(ci, "zeron-plugin.json"), JSON.stringify({ ...m, version: "1.0.1" }));
  writeFileSync(join(ci, "CHANGELOG.md"), "# Changes\n\n## 1.0.1\n\n- A fix.\n");
  const refused = await run(zeron, ["plugin", "publish", ci], { env: { PATH: process.env.PATH, HOME: join(work, "ci-home"), ZERON_DATA_DIR: join(work, "ci-home", "data"), ZERON_IPC_PORT: "1", ZERON_EDGE_URL: edge.url, ZERON_DEPLOY_TOKEN: two } });
  check("a token for two plugins is refused a third (exit 3, token_not_allowed)", refused.code === 3 && refused.err.includes("token_not_allowed"), refused.err.trim().split("\n").pop());
  const again = await run(process.execPath, [join(here, "seed.mjs"), "--accounts", join(work, "accounts.json"), "--edge", edge.url], { env: { ...process.env, SEED_ALLOWED_EDGES: edge.url, ...tokens, SEED_PACE_MS: "0" } });
  check("a second run writes nothing", again.code === 0 && again.out.includes(`done: 0 written, ${VERSIONS} already there`), again.out.trim().split("\n").pop());
} catch (error) {
  check("the run", false, error.message);
} finally {
  await stop();
}
console.log(failures.length ? `\n${failures.length} failed` : "\nall passed");
process.exit(failures.length ? 1 : 0);
