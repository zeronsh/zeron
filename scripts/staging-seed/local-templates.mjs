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
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { TEMPLATES, USERNAMES, ZERON } from "./templates.mjs";

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
  const seed = (env, extra = []) => run(process.execPath, [join(here, "seed-templates.mjs"), "--accounts", join(work, "accounts.json"), "--ui", ui, "--playwright", playwright, ...extra], { env: { ...process.env, SEED_PACE_MS: "0", ...env } });
  const versions = TEMPLATES.flatMap((t) => t.versions.map((v) => ({ id: t.id, ...v })));
  const dry = await seed({}, ["--dry-run"]);
  check(`the CLI's check takes every version (${versions.length})`, dry.code === 0 && (dry.out.match(/^ready /gm) ?? []).length === versions.length, (dry.err || dry.out).trim().split("\n").pop());
  const first = await seed({});
  process.stdout.write(first.out);
  check("the first pass publishes the 8 first versions and stops for tokens (exit 3)", first.code === 3 && (first.out.match(/^published .* \(device\)$/gm) ?? []).length === TEMPLATES.length, first.err.trim().slice(0, 400));
  for (const d of authors) d.token = await makeToken(d);
  const tokens = Object.fromEntries(authors.map((d) => [`SEED_TOKEN_${d.slot}`, d.token]));
  const second = await seed(tokens);
  process.stdout.write(second.out);
  check("the second run publishes the rest", second.code === 0 && second.out.includes(`done: ${versions.length - TEMPLATES.length} written, ${TEMPLATES.length} already there`), (second.err || second.out).trim().split("\n").pop());

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
    const pics = [];
    for (const path of ["preview.png", "screenshots/chat.png", "screenshots/new-chat.png"]) {
      const p = await get(`/store/v1/templates/${t.id}/files/${d.version}/${path}`);
      pics.push(p.status === 200 && p.type === "image/png" && p.body.readUInt32BE(16) === 1280 && p.body.readUInt32BE(20) === 800);
    }
    check(`${t.id}: its pictures are served, 1280×800`, pics.every(Boolean), pics);
  }
  const beta = (await get("/store/v1/templates/wide-margin/versions")).body.versions.find((v) => v.prerelease);
  check("wide-margin's 1.1.0-beta.1 is a pre-release", beta?.version === "1.1.0-beta.1", beta?.version);
  const focus = (await get("/store/v1/templates?keyword=focus")).body.items.map((x) => x.id).sort();
  check("a keyword filters", JSON.stringify(focus) === JSON.stringify(["nightfall-cards", "wide-margin"]), focus.join(","));
  const q = (await get("/store/v1/templates?q=slate")).body.items.map((x) => x.id);
  check("a search finds one by its name", q.includes("slate-grid"), q.join(","));
  const again = await seed(tokens);
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
