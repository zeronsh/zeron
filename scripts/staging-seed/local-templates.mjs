#!/usr/bin/env node
// The templates' seed, run against a local edge and checked, as local.mjs is
// for the plugins': the real edge worker in Miniflare with its WorkOS fake
// (scripts/plugin-store-e2e/edge.mjs), two real `zeron headless` engines
// signed in through the fake, then seed-templates.mjs as it runs on staging,
// each author's token made with the CLI (`--all --templates`).
//
//   node local-templates.mjs --zeron <build> --ui <apps/zgx/ui> --playwright <dir> [--e2e <plugin-store-e2e folder>] [--edge-dir edge] [--keep]

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { pictures } from "./pictures.mjs";
import { quiet, quietLibraries, secretsOf } from "./quiet.mjs";
import { TEMPLATES, ZERON } from "./templates.mjs";
import { whyNotItsSession, whyNotSeeded } from "./where.mjs";

/** This run's authors, by slot (the staging ones are the logins file's). */
const USERNAMES = Object.fromEntries([...new Set(TEMPLATES.map((t) => t.slot))].map((slot) => [slot, `looks-${slot}`]));

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i < 0 ? undefined : args[i + 1];
};
const need = (name) => flag(name) ?? (console.error(`local-templates: give ${name}`), process.exit(2));
const zeron = resolve(need("--zeron"));
const ui = resolve(need("--ui"));
const playwright = resolve(need("--playwright"));
const e2e = resolve(flag("--e2e") ?? join(root, "scripts/plugin-store-e2e"));
const edgeDir = resolve(flag("--edge-dir") ?? join(root, "edge"));
const work = mkdtempSync(join(tmpdir(), "zeron-seed-templates-"));
const WebSocket = createRequire(join(playwright, "package.json"))("ws");

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
// Where a seed may write.
{
  const list = "https://one.example.com, http://127.0.0.1:9";
  const refused = (edge, allowed = list) => typeof whyNotSeeded(edge, allowed) === "string";
  check("a seed writes to an edge it's allowed by name, and to no other",
    !refused("https://one.example.com") && !refused("https://one.example.com/") && !refused("http://127.0.0.1:9") &&
      [undefined, "", "nonsense", "http://one.example.com", "https://one.example.com:8443", "https://one.example.com/x", "https://a:b@one.example.com", "https://one.example.com.evil.example", "https://two.example.com", "http://127.0.0.1:8", "ftp://one.example.com"].every((e) => refused(e)) &&
      ["", undefined, ",", "one.example.com", "https://*.example.com"].every((allowed) => refused("https://one.example.com", allowed === undefined ? null : allowed)));
  check("and never to production's, even allowed", ["https://zeron.sh", "https://edge.zeron.sh", "https://EDGE.zeron.sh", "https://edge.zeron.sh.", "https://www.zeron.sh", "https://comet.zeron.sh", "https://edge.comet.zeron.sh", "https://x.any.workers.dev", "https://notstaging.zeron.sh", "https://stagingx.any.workers.dev"].every((e) => refused(e, e)) &&
      ["https://edge.zeron.sh.example.com", "https://notzeron.sh", "https://x-staging.any.workers.dev"].every((e) => !refused(e, e)));
  const jwt = (iss) => `x.${Buffer.from(JSON.stringify({ iss })).toString("base64url")}.y`;
  check("and a Zeron signed in to another WorkOS", whyNotItsSession(jwt("https://w/client_a"), "client_a") === undefined && [whyNotItsSession(jwt("https://w/client_ab"), "client_a"), whyNotItsSession(jwt("https://w/client_a"), undefined), whyNotItsSession("", "client_a")].every((w) => typeof w === "string"));
}

// What's printed.
{
  const login = { username: "ann-author", email: "ann@mail.example.net", user_id: "user_01ANN", password: 'p"a/ss word+1' };
  const raw = [`fill(${JSON.stringify(login.password)})`, login.password, encodeURIComponent(login.password), "Signed in as ann@mail.example.net (@ann-author, ann-author)", "user_01ANN", `zdt_${"a".repeat(49)}`, "eyJhbGciOi.eyJzdWIi.c2ln", "someone.else@mail.example.org", "by @other-name"].join("\n");
  const out = quiet(raw, secretsOf(login));
  check("nothing printed names an account or shows a secret", !/p.?"?a\/ss|word|ann|user_01|zdt_a|eyJ|someone|other-name|mail\.example/i.test(out) && quiet("publishing focus-timer 1.0.0 (exit 3): bad_listing") === "publishing focus-timer 1.0.0 (exit 3): bad_listing", out);
  const odd = { username: "zed-author", password: `q'w"e\\r&t<y u+i%o` };
  const writings = [odd.password, JSON.stringify(odd.password), `'${odd.password.replace(/[\\']/g, "\\$&")}'`, encodeURIComponent(odd.password), new URLSearchParams({ p: odd.password }).toString(), odd.password.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;"), "ZED-AUTHOR"];
  const missed = writings.filter((w) => /q.{0,6}w.{0,6}e|y.{0,3}u.{0,3}i|zed/i.test(quiet(w, secretsOf(odd)).replace(/^p=/, "")));
  check("however a login is written", missed.length === 0, `${missed.length} of ${writings.length} shown`);
  const kept = { ...process.env };
  Object.assign(process.env, { DEBUG: "pw:api", PWDEBUG: "1", NODE_DEBUG: "net" });
  quietLibraries();
  check("and the browser's own log is off", ["DEBUG", "PWDEBUG", "NODE_DEBUG"].every((name) => process.env[name] === undefined));
  for (const name of ["DEBUG", "PWDEBUG", "NODE_DEBUG"]) if (kept[name] !== undefined) process.env[name] = kept[name];
  const bad = join(work, "logins.json");
  writeFileSync(bad, '[{"username":"ann-author","password":"p4ss');
  const ran = await run(process.execPath, [join(here, "sessions.mjs"), "up", "--zeron", zeron, "--edge", "http://127.0.0.1:9", "--client-id", "client_x", "--logins", bad, "--dir", join(work, "none"), "--playwright", playwright], { env: { ...process.env, SEED_ALLOWED_EDGES: "http://127.0.0.1:9" } });
  check("a broken logins file isn't quoted", ran.code === 2 && !/ann|p4ss/.test(ran.out + ran.err), (ran.err || ran.out).trim().split("\n")[0]);
}

try {
  mkdirSync(join(edgeDir, "public"), { recursive: true });
  const bundled = await run("npx", ["wrangler", "deploy", "--dry-run", "--outdir", join(work, "bundle")], { cwd: edgeDir });
  if (bundled.code !== 0) throw new Error(`the edge didn't bundle:\n${bundled.err}`);
  const { startEdge } = await import(pathToFileURL(join(e2e, "edge.mjs")));
  edge = await startEdge({ edgeDir, bundleDir: join(work, "bundle"), site: false });
  console.log(`== the edge serves ${edge.url}`);

  const authors = [];
  for (const slot of Object.keys(USERNAMES).map(Number)) {
    const home = join(work, `author-${slot}`, "home");
    const data = join(work, `author-${slot}`, "data");
    mkdirSync(home, { recursive: true });
    mkdirSync(data, { recursive: true });
    const port = await freePort();
    const env = { PATH: process.env.PATH, HOME: home, ZERON_DATA_DIR: data, ZERON_EDGE_URL: edge.url, ZERON_WORKOS_CLIENT_ID: edge.clientId, ZERON_IPC_PORT: String(port) };
    engines.push(spawn(zeron, ["headless"], { env: { ...env, RUST_LOG: "warn", ZERON_CALLBACK_PORT: String(await freePort()) }, stdio: "ignore" }));
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
    const user = `user_seed${slot}`;
    const { url } = await call("SignInHeadless");
    await call("CompleteSignIn", { code: `${new URL(url).searchParams.get("state")}.code:${user}` });
    for (let t = 0; t < 120; t++) {
      if ((await call("PluginSigningKey"))?.registered === true) break;
      await sleep(250);
    }
    const bearer = (await call("AccessToken")).token;
    const named = await fetch(`${edge.url}/username`, { method: "POST", headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json" }, body: JSON.stringify({ username: USERNAMES[slot] }) });
    if (!named.ok) throw new Error(`author ${slot} couldn't claim ${USERNAMES[slot]}: ${named.status}`);
    authors.push({ slot, user, home, data, port, env, bearer });
    console.log(`== author ${slot}: ${USERNAMES[slot]}, signed in`);
  }

  const makeToken = async (d) => {
    let played = false;
    const made = await run(zeron, ["plugin", "token", "create", "--name", "Seed CI", "--all", "--templates"], {
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
    if (made.code !== 0 || !token) throw new Error(`author ${d.slot}'s token: ${made.code} ${made.err.replace(/zdt_[0-9A-Za-z]{49}/g, "zdt_…")}`);
    return token;
  };

  writeFileSync(
    join(work, "accounts.json"),
    JSON.stringify({ edge: edge.url, workos_client_id: edge.clientId, zeron, authors: authors.map((d) => ({ slot: d.slot, home: d.home, data_dir: d.data, ipc_port: d.port, token_env: `SEED_TOKEN_${d.slot}` })) })
  );
  const seed = (env, extra = []) => run(process.execPath, [join(here, "seed-templates.mjs"), "--accounts", join(work, "accounts.json"), "--edge", edge.url, "--ui", ui, "--playwright", playwright, ...extra], { env: { ...process.env, SEED_ALLOWED_EDGES: edge.url, SEED_PACE_MS: "0", ...env } });
  // Stand-ins for the app's renderings (other bytes than the drawn ones).
  const shots = join(work, "pictures");
  const browser = await createRequire(join(playwright, "package.json"))("playwright").chromium.launch();
  for (const t of TEMPLATES) {
    const drawn = await pictures(browser, { ...t, name: `${t.name} as it is`, home: ["As it is", "Nothing more."] });
    for (const dir of [t.id, `${t.id}-beta`]) {
      mkdirSync(join(shots, dir), { recursive: true });
      for (const [name, file] of [["preview", "preview.png"], ["preview", "chat.png"], ["home", "new-chat.png"]]) writeFileSync(join(shots, dir, file), drawn[name]);
    }
  }
  await browser.close();
  const versions = TEMPLATES.flatMap((t) => t.versions.map((v) => ({ id: t.id, ...v })));
  const drawnOnly = versions.filter((v) => !v.rendered).length;
  const dry = await seed({}, ["--dry-run", "--pictures", shots]);
  check(`the CLI's check takes every version (${versions.length})`, dry.code === 0 && (dry.out.match(/^ready /gm) ?? []).length === versions.length, (dry.err || dry.out).trim().split("\n").pop());
  // The same seed where this run's WorkOS is production's (a copy beside an
  // edge config that says so): refused, though every other thing is right, and
  // so is an accounts file that doesn't say its edge, or says another.
  {
    const copy = join(work, "as-production");
    cpSync(here, join(copy, "scripts/staging-seed"), { recursive: true });
    const refused = async (script, accounts, flags = []) => {
      writeFileSync(join(work, "refused.json"), JSON.stringify(accounts));
      const ran = await run(process.execPath, [script, "--accounts", join(work, "refused.json"), "--edge", edge.url, "--ui", ui, "--playwright", playwright, ...flags], { env: { ...process.env, SEED_ALLOWED_EDGES: edge.url } });
      return ran.code === 2 && !/published|ready/.test(ran.out) ? ran.err.trim().split("\n")[0] : `exit ${ran.code}: ${ran.out}${ran.err}`;
    };
    const right = JSON.parse(readFileSync(join(work, "accounts.json"), "utf8"));
    const script = join(copy, "scripts/staging-seed/seed-templates.mjs");
    mkdirSync(join(copy, "edge"), { recursive: true });
    writeFileSync(join(copy, "edge/wrangler.jsonc"), `// its config\n${JSON.stringify({ vars: { WORKOS_CLIENT_ID: edge.clientId } })}`);
    // (A hand-made file as a person could write it: production's WorkOS, a
    // Zeron of production's, no edge, and --edge one that's allowed.)
    const { edge: _none, ...handMade } = right;
    const byHand = await refused(script, handMade);
    check("a hand-made file for production's Zeron is refused", byHand.startsWith("seed"), byHand);
    const named = await refused(script, right);
    check("production's WorkOS is refused, though the accounts file names it", named.includes("production's"), named);
    const signedIn = await refused(script, { ...right, workos_client_id: "client_other" }, []);
    check("and a Zeron signed in to it, whatever the file names", signedIn.includes("signed in to production"), signedIn);
    rmSync(join(copy, "edge/wrangler.jsonc"));
    const unread = await refused(script, right);
    check("and any when production's can't be read", unread.includes("can't be read"), unread);
    const { edge: _edge, ...edgeless } = right;
    const none = await refused(join(here, "seed-templates.mjs"), edgeless);
    const other = await refused(join(here, "seed-templates.mjs"), { ...right, edge: "https://other.example.com" });
    check("and an accounts file whose edge isn't --edge, or that has none", none.includes("isn't --edge") && other.includes("SEED_ALLOWED_EDGES"), `${none} / ${other}`);
  }
  // A Zeron signed in elsewhere (as a wrong accounts file would name): refused
  // before it publishes anything.
  writeFileSync(join(work, "elsewhere.json"), JSON.stringify({ ...JSON.parse(readFileSync(join(work, "accounts.json"), "utf8")), workos_client_id: "client_elsewhere" }));
  const elsewhere = await run(process.execPath, [join(here, "seed-templates.mjs"), "--accounts", join(work, "elsewhere.json"), "--edge", edge.url, "--ui", ui, "--playwright", playwright], { env: { ...process.env, SEED_ALLOWED_EDGES: edge.url } });
  check("a Zeron signed in to another WorkOS publishes nothing (exit 2)", elsewhere.code === 2 && elsewhere.err.includes("another WorkOS") && !elsewhere.out.includes("published"), elsewhere.err.trim().split("\n")[0]);
  const first = await seed({});
  process.stdout.write(first.out);
  check("the first pass publishes the 8 first versions and stops for tokens (exit 3)", first.code === 3 && (first.out.match(/^published .* \(device\)$/gm) ?? []).length === TEMPLATES.length, first.err.trim().slice(0, 400));
  for (const d of authors) d.token = await makeToken(d);
  const tokens = Object.fromEntries(authors.map((d) => [`SEED_TOKEN_${d.slot}`, d.token]));
  const second = await seed(tokens);
  process.stdout.write(second.out);
  check("the second run publishes the rest", second.code === 0 && second.out.includes(`done: ${drawnOnly - TEMPLATES.length} written, ${TEMPLATES.length} already there`), (second.err || second.out).trim().split("\n").pop());
  // (The store takes ten versions an account an hour: an hour later, on this
  // local edge alone.)
  const { EDGE_NAME } = await import(pathToFileURL(join(e2e, "edge.mjs")));
  await (await edge.mf.getD1Database("STORE_DB", EDGE_NAME)).prepare("UPDATE versions SET published_at = published_at - 7200000").run();
  const third = await seed(tokens, ["--pictures", shots]);
  process.stdout.write(third.out);
  check("with the app's renderings, the versions that use them", third.code === 0 && third.out.includes(`done: ${versions.length - drawnOnly} written, ${drawnOnly} already there`), (third.err || third.out).trim().split("\n").pop());

  const get = async (path, bearer) => {
    const res = await fetch(`${edge.url}${path}`, bearer ? { headers: { authorization: `Bearer ${bearer}` } } : {});
    const json = res.headers.get("content-type")?.includes("json");
    return { status: res.status, type: res.headers.get("content-type"), body: json ? await res.json() : Buffer.from(await res.arrayBuffer()) };
  };
  const list = (await get("/store/v1/templates")).body;
  check("8 templates listed, none among the plugins", list.total === 8 && (await get("/store/v1/plugins")).body.total === 0, list.total);
  for (const [slot, name] of Object.entries(USERNAMES)) check(`@${name} has 4`, (await get(`/store/v1/templates?author=${name}`)).body.total === TEMPLATES.filter((t) => t.slot === Number(slot)).length);
  for (const t of TEMPLATES) {
    const d = (await get(`/store/v1/templates/${t.id}`)).body;
    const got = new Map((d.versions ?? []).map((v) => [v.version, v]));
    const right = t.versions.every((v) => got.get(v.v)?.via === (v.by === "device" ? "device" : "deploy_token") && (got.get(v.v)?.key === null) === (v.by === "token"));
    const release = t.versions.filter((v) => !v.v.includes("-")).at(-1).v;
    check(`${t.id}: its versions, device's signed and token's not, ${release} shown`, right && d.version === release && d.engines?.zeron === `>=${ZERON}` && d.cover === "preview.png" && d.icon === null && !("permissions" in d), { version: d.version, versions: [...got].map(([k, v]) => `${k}:${v.via}`) });
    // A version that corrects its pictures and words, and the files of its
    // that were wrong (its home page, its tokens): nothing else.
    for (const [i, v] of t.versions.entries()) {
      if (!v.rendered) continue;
      const read = async (version) => {
        const pkg = JSON.parse((await get(`/store/v1/templates/${t.id}/versions/${version}/package`)).body);
        const { signature: _s, version: _v, description: _d, keywords: _k, ...manifest } = pkg.manifest;
        return { manifest: JSON.stringify(manifest), files: pkg.files };
      };
      const [was, now] = [await read(t.versions[v.like].v), await read(v.v)];
      const names = [...new Set([...Object.keys(was.files), ...Object.keys(now.files)])].filter((f) => was.files[f] !== now.files[f]).sort();
      check(`${t.id} ${v.v}: only its pictures, words, notes and wrong files differ from ${t.versions[v.like].v}`, was.manifest === now.manifest && JSON.stringify(names) === JSON.stringify(["CHANGELOG.md", "README.md", ...(t.change ? ["ds/tokens.luau"] : []), "preview.png", ...(t.home ? ["screens/chat/home.luau"] : []), ...(v.width ? ["screens/settings/prefs.luau"] : []), "screenshots/chat.png", "screenshots/new-chat.png"]), [i, names]);
    }
    // Its changed copies of Zeron's files: Zeron's own but for what it means.
    const shown = JSON.parse((await get(`/store/v1/templates/${t.id}/versions/${d.version}/package`)).body).files;
    const lines = (path) => {
      const [own, its] = [readFileSync(join(ui, path), "utf8").split("\n"), Buffer.from(shown[path], "base64").toString().split("\n")];
      return { gone: own.filter((l) => !its.includes(l)).length, more: its.filter((l) => !own.includes(l)).length };
    };
    const now = t.versions.find((v) => v.v === d.version);
    if (now.width) check(`${t.id}: its preferences are Zeron's but for one line`, JSON.stringify(lines("screens/settings/prefs.luau")) === '{"gone":1,"more":1}', lines("screens/settings/prefs.luau"));
    if (now.plain) check(`${t.id}: it has no tokens of its own`, !("ds/tokens.luau" in shown));
    if (t.change && !now.plain) check(`${t.id}: its tokens are Zeron's but for one line`, JSON.stringify(lines("ds/tokens.luau")) === '{"gone":1,"more":1}', lines("ds/tokens.luau"));
    if (t.home) check(`${t.id}: its home page is Zeron's whole, and its words`, lines("screens/chat/home.luau").gone === 0 && lines("screens/chat/home.luau").more <= 16, lines("screens/chat/home.luau"));
    const pics = [];
    for (const path of ["preview.png", "screenshots/chat.png", "screenshots/new-chat.png"]) {
      const p = await get(`/store/v1/templates/${t.id}/files/${d.version}/${path}`);
      pics.push(p.status === 200 && p.type === "image/png" && p.body.readUInt32BE(16) === 1280 && p.body.readUInt32BE(20) === 800);
    }
    check(`${t.id}: its pictures are served, 1280×800`, pics.every(Boolean), pics);
  }
  const beta = (await get("/store/v1/templates/wide-margin/versions")).body.versions.find((v) => v.prerelease);
  check("wide-margin's newest, 1.2.0-beta.1, is a pre-release", beta?.version === "1.2.0-beta.1", beta?.version);
  const focus = (await get("/store/v1/templates?keyword=focus")).body.items.map((x) => x.id).sort();
  check("a keyword filters", JSON.stringify(focus) === JSON.stringify(["nightfall-cards", "wide-margin"]), focus.join(","));
  const palette = (await get("/store/v1/templates?keyword=palette")).body.items.map((x) => x.id).sort();
  const cards = (await get("/store/v1/templates?keyword=cards")).body.total;
  check("the corrected keywords are the ones listed", JSON.stringify(palette) === JSON.stringify(["nightfall-cards", "slate-grid"]) && cards === 0, `palette ${palette.join(",")}; cards ${cards}`);
  const q = (await get("/store/v1/templates?q=slate")).body.items.map((x) => x.id);
  check("a search finds one by its name", q.includes("slate-grid"), q.join(","));
  const again = await seed(tokens, ["--pictures", shots]);
  check("a run again writes nothing", again.code === 0 && again.out.includes(`done: 0 written, ${versions.length} already there`), again.out.trim().split("\n").pop());
  const plug = await get("/store/v1/plugins/paper-desk");
  const temp = await get("/store/v1/templates/no-such-one");
  check("what isn't there is said in its prefix's word", plug.body.message === "There's no such plugin." && temp.body.message === "There's no such template.");
} catch (error) {
  check("the run", false, error.message);
} finally {
  for (const e of engines) e.kill();
  await edge?.stop?.();
  if (args.includes("--keep")) console.log(`scratch kept: ${work}`);
  else rmSync(work, { recursive: true, force: true });
}
console.log(failures.length ? `\n${failures.length} failed` : "\nall passed");
process.exit(failures.length ? 1 : 0);
