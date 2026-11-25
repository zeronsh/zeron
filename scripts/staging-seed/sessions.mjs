#!/usr/bin/env node
// The seed's authors' sessions, made through the real paths: one `zeron
// headless` per author (its own HOME, data folder and IPC port), signed in
// with the engine's headless sign-in (`SignInHeadless`: the hosted AuthKit
// page, then the edge's `/auth/cli/callback` code, pasted back with
// `CompleteSignIn`), in a workspace of its own (`CreateOrg`, as the app's
// first run makes one; or `SelectOrg`, the one it has: the engine counts a
// session as signed in only in a workspace), its username claimed, and its
// deploy token made with
// the CLI (`zeron plugin token create --all`, whose fresh sign-in is the
// hosted page again). A headless browser (Playwright) fills the hosted page
// with each account's email and password, as the person would; nothing is
// bypassed. seed.mjs then runs with what this writes.
//
//   node sessions.mjs up     --zeron <bin> --edge <url> --client-id <id> --logins <file> --dir <folder> [--playwright <dir>]
//   node sessions.mjs tokens …the same… [--templates]
//   node sessions.mjs down   --dir <folder>
//
// `--edge` must be one SEED_ALLOWED_EDGES names, never production's
// (where.mjs). `--only 7,8` is those authors alone (every author else). `--templates` makes
// their tokens for their templates too (`--all --templates`).
//
// `--logins` is a JSON list of {username, email, user_id, password}, slot n
// being the n-th; passwords are read, never printed or written anywhere.
// `--dir` keeps each author's folders (author-<n>/home, data, engine.log,
// engine.pid), `accounts.json` for seed.mjs, and `tokens.env` (0600, the
// tokens as SEED_TOKEN_<n>=…, made once: a token is shown only when made).
// `up` starts what isn't running and signs in who isn't; `tokens` makes
// the tokens not yet made; `down` stops the engines by their own pids.

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { chmodSync, existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { PLUGINS } from "./catalog.mjs";
import { TEMPLATES } from "./templates.mjs";
import { quiet, quietLibraries, secretsOf } from "./quiet.mjs";
import { whyNotItsWorkos, whyNotSeeded } from "./where.mjs";


const [command, ...rest] = process.argv.slice(2);
const flag = (name) => {
  const i = rest.indexOf(name);
  return i < 0 ? undefined : rest[i + 1];
};
const need = (name) => flag(name) ?? (console.error(`sessions: give ${name}`), process.exit(2));
const dir = resolve(need("--dir"));
const PLUGIN_SLOTS = new Set(PLUGINS.map((p) => p.slot));
const SLOTS = [...new Set([...PLUGIN_SLOTS, ...TEMPLATES.map((t) => t.slot)])].sort((a, b) => a - b);
const only = flag("--only")?.split(",").map(Number) ?? SLOTS;
if (!only.every((slot) => SLOTS.includes(slot))) {
  console.error(`sessions: --only names authors ${SLOTS[0]} to ${SLOTS.at(-1)}`);
  process.exit(2);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const slotDir = (slot) => join(dir, `author-${slot}`);
const pidFile = (slot) => join(slotDir(slot), "engine.pid");
const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** Stop author `slot`'s engine by its own pid. */
const stop = async (slot, quiet = false) => {
  if (!existsSync(pidFile(slot))) return;
  const pid = Number(readFileSync(pidFile(slot), "utf8"));
  if (alive(pid)) process.kill(pid, "SIGTERM");
  for (let t = 0; t < 40 && alive(pid); t++) await sleep(250);
  if (!quiet) console.log(`author ${slot}: engine ${pid} ${alive(pid) ? "still running" : "stopped"}`);
  if (!alive(pid)) rmSync(pidFile(slot));
};

if (command === "down") {
  for (const slot of only) await stop(slot);
  process.exit(0);
}
if (command !== "up" && command !== "tokens") {
  console.error("sessions: up | tokens | down");
  process.exit(2);
}

const zeron = resolve(need("--zeron"));
const edge = need("--edge").replace(/\/$/, "");
const refusedEdge = whyNotSeeded(edge, process.env.SEED_ALLOWED_EDGES);
if (refusedEdge) {
  console.error(`sessions: ${refusedEdge}`);
  process.exit(2);
}
const clientId = need("--client-id");
const refusedWorkos = whyNotItsWorkos(clientId);
if (refusedWorkos) {
  console.error(`sessions: ${refusedWorkos}`);
  process.exit(2);
}
const loginsFile = need("--logins");
/** The logins file's accounts (a JSON error would quote the file). */
const readLogins = () => {
  try {
    const list = JSON.parse(readFileSync(loginsFile, "utf8"));
    return Array.isArray(list) ? list : undefined;
  } catch {
    return undefined;
  }
};
const logins = readLogins();
if (!logins) {
  console.error("sessions: the logins file isn't a JSON list of accounts");
  process.exit(2);
}
/** `text` as it may be printed: no account named, no secret shown. */
const said = (text) => quiet(text, logins.flatMap(secretsOf));
for (const slot of only) {
  const l = logins[slot - 1] ?? {};
  for (const key of ["username", "email", "user_id", "password"]) if (typeof l[key] !== "string" || !l[key]) throw new Error(`sessions: account ${slot} of the logins file has no ${key}`);
}
quietLibraries();
const require = createRequire(join(resolve(flag("--playwright") ?? "."), "package.json"));
const { chromium } = require("playwright");
const WebSocket = require("ws");

const ipcPort = (slot) => 27710 + slot;
const engineEnv = (slot) => ({
  PATH: process.env.PATH,
  HOME: join(slotDir(slot), "home"),
  ZERON_DATA_DIR: join(slotDir(slot), "data"),
  ZERON_IPC_PORT: String(ipcPort(slot)),
  ZERON_EDGE_URL: edge,
  ZERON_WORKOS_CLIENT_ID: clientId
});

/** Author `slot`'s engine, started unless it's running: its IPC. */
const engine = async (slot) => {
  const env = engineEnv(slot);
  mkdirSync(env.HOME, { recursive: true });
  mkdirSync(env.ZERON_DATA_DIR, { recursive: true });
  const running = existsSync(pidFile(slot)) && alive(Number(readFileSync(pidFile(slot), "utf8")));
  if (!running) {
    const log = openSync(join(slotDir(slot), "engine.log"), "a");
    const child = spawn(zeron, ["headless"], { env: { ...env, RUST_LOG: "warn" }, detached: true, stdio: ["ignore", log, log] });
    child.unref();
    writeFileSync(pidFile(slot), String(child.pid));
  }
  let ws;
  for (let t = 0; t < 240 && !ws; t++) {
    if (t) await sleep(250);
    const tokenFile = join(env.ZERON_DATA_DIR, "ipc-token");
    if (!existsSync(tokenFile)) continue;
    const token = readFileSync(tokenFile, "utf8").trim();
    ws = await new Promise((res) => {
      const s = new WebSocket(`ws://127.0.0.1:${ipcPort(slot)}`, { headers: { authorization: `Bearer ${token}` } });
      s.once("open", () => res(s));
      s.once("error", () => res(undefined));
    });
  }
  if (!ws) throw new Error(`author ${slot}'s engine never answered (see ${join(slotDir(slot), "engine.log")})`);
  let next = 1;
  const waiting = new Map();
  ws.on("message", (raw) => {
    // (A frame that isn't JSON is no answer, and an error would quote it.)
    let frame;
    try {
      frame = JSON.parse(String(raw));
    } catch {
      return;
    }
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
  return { slot, env, call, close: () => ws.close() };
};

/** The account a bearer is (its JWT's `sub`), without verifying it: the
 * edge does that; this only checks the right person signed in. */
const subject = (bearer) => {
  try {
    return JSON.parse(Buffer.from(String(bearer).split(".")[1], "base64url").toString()).sub;
  } catch {
    return undefined;
  }
};

/** Complete AuthKit's hosted page at `url` as `login`, until the browser
 * reaches `until` (a URL prefix). Answers the page it lands on. */
const hostedSignIn = async (browser, url, login, until) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    await page.goto(url);
    const started = Date.now();
    while (!page.url().startsWith(until)) {
      if (Date.now() - started > 90_000) throw Object.assign(new Error(), { name: "Timeout" });
      const email = page.locator('input[type="email"], input[name="email"]').first();
      const password = page.locator('input[type="password"]').first();
      if (await password.isVisible().catch(() => false)) {
        await password.fill(login.password);
        await password.press("Enter");
        await page.waitForTimeout(1500);
      } else if (await email.isVisible().catch(() => false)) {
        await email.fill(login.email);
        await email.press("Enter");
        await page.waitForTimeout(1500);
      } else {
        await page.waitForTimeout(500);
      }
    }
    await page.waitForLoadState("domcontentloaded");
    return { page, context };
  } catch (error) {
    // (Its own sentence: the browser's errors quote what it typed.)
    const at = URL.parse(page.url());
    await context.close().catch(() => {});
    throw new Error(`the sign-in page didn't finish (${error.name}, at ${at ? at.origin + at.pathname : "no page"})`);
  }
};

const browser = await chromium.launch();
try {
  const accounts = [];
  for (const slot of only) {
    const login = logins[slot - 1];
    // A fresh sign-in comes back to the engine's loopback callback, on the
    // one port the sign-in's redirect list names (27641, every engine's):
    // so for a token, only this author's engine runs (`up` starts the rest).
    if (command === "tokens") for (const other of SLOTS) if (other !== slot) await stop(other, true);
    const d = await engine(slot);
    if (command === "up") {
      let bearer = await d.call("AccessToken").then((r) => r?.token).catch(() => undefined);
      if (!bearer) {
        const { url } = await d.call("SignInHeadless");
        const { page, context } = await hostedSignIn(browser, url, login, `${edge}/auth/cli/callback`);
        const code = (await page.locator("#paste").textContent())?.trim();
        await context.close();
        if (!code) throw new Error(`author ${slot}: the callback page showed no code`);
        await d.call("CompleteSignIn", { code });
        bearer = (await d.call("AccessToken")).token;
      }
      if (subject(bearer) !== login.user_id) throw new Error(`author ${slot}'s engine is signed in as someone else`);
      const { orgs } = await d.call("ListOrgs");
      if (orgs.length === 0) await d.call("CreateOrg", { name: login.username });
      else await d.call("SelectOrg", { organizationId: orgs[0].organizationId ?? orgs[0].organization_id });
      bearer = (await d.call("AccessToken")).token;
      for (let t = 0; t < 120; t++) {
        if ((await d.call("PluginSigningKey"))?.registered === true) break;
        await sleep(500);
      }
      const named = await (await fetch(`${edge}/username`, { headers: { authorization: `Bearer ${bearer}` } })).json();
      if (!named.username) {
        const claimed = await fetch(`${edge}/username`, { method: "POST", headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json" }, body: JSON.stringify({ username: login.username }) });
        if (!claimed.ok) throw new Error(`author ${slot} couldn't claim its username (${claimed.status})`);
      } else if (named.username !== login.username) {
        throw new Error(`author ${slot} already has another username than the logins file's`);
      }
      const key = await d.call("PluginSigningKey");
      // The store sees the session as the CLI does.
      const status = await new Promise((res) => {
        const child = spawn(zeron, ["plugin", "status"], { env: { ...d.env, RUST_LOG: "warn" }, stdio: ["ignore", "ignore", "pipe"] });
        let err = "";
        child.stderr.on("data", (x) => (err += x));
        child.on("close", (code) => res({ code, err }));
      });
      if (status.code !== 0) throw new Error(`author ${slot}: \`zeron plugin status\` exits ${status.code}`);
      console.log(`author ${slot}: signed in, in a workspace, signing key ${key?.registered ? "registered" : "NOT registered"}, engine on ${ipcPort(slot)}`);
      accounts.push({ slot, home: d.env.HOME, data_dir: d.env.ZERON_DATA_DIR, ipc_port: ipcPort(slot), token_env: `SEED_TOKEN_${slot}` });
    } else {
      // tokens: one each, made once (shown only when made).
      const envFile = join(dir, "tokens.env");
      const have = existsSync(envFile) ? readFileSync(envFile, "utf8") : "";
      if (have.includes(`SEED_TOKEN_${slot}=`)) {
        console.log(`author ${slot}: has its token`);
        d.close();
        continue;
      }
      let signing;
      const made = await new Promise((res) => {
        const child = spawn(zeron, ["plugin", "token", "create", "--all", ...(rest.includes("--templates") ? ["--templates"] : []), "--name", "Seed CI"], { env: { ...d.env, RUST_LOG: "warn" }, stdio: ["ignore", "pipe", "pipe"] });
        let out = "";
        let err = "";
        child.stdout.on("data", (x) => (out += x));
        child.stderr.on("data", (x) => {
          err += x;
          const url = String(err).match(/https:\/\/\S+authorize\S+/)?.[0];
          if (url && !signing) {
            signing = hostedSignIn(browser, url, login, "http://127.0.0.1:").then(
              ({ context }) => sleep(2000).then(() => context.close()),
              (e) => (err += `\n(sign-in: ${e.message})`)
            );
          }
        });
        child.on("close", (code) => res({ code, out, err }));
      });
      await signing;
      const token = made.out.match(/zdt_[0-9A-Za-z]{49}/)?.[0];
      if (made.code !== 0 || !token) throw new Error(`author ${slot}'s token wasn't made (exit ${made.code}): ${made.err.replace(/zdt_[0-9A-Za-z]{49}/g, "zdt_…").replace(/https:\/\/\S+/g, "<url>").trim()}`);
      writeFileSync(envFile, `${have}SEED_TOKEN_${slot}=${token}\n`, { mode: 0o600 });
      chmodSync(envFile, 0o600);
      console.log(`author ${slot}: token ${token.slice(4, 12)} made`);
    }
    d.close();
  }
  if (command === "up") {
    // (One author set's run leaves another's file: the plugins' is accounts.json.)
    const name = only.some((slot) => PLUGIN_SLOTS.has(slot)) ? "accounts.json" : "accounts-templates.json";
    writeFileSync(join(dir, name), `${JSON.stringify({ edge, workos_client_id: clientId, zeron, authors: accounts }, null, 2)}\n`);
    console.log(`accounts for the seed: ${join(dir, name)}`);
  }
} catch (error) {
  // (The browser's errors quote what it typed, and the engine's and the CLI's
  // may name an account: none of it is printed as it came.)
  console.error(`sessions: ${said(error.message)}`);
  process.exitCode = 1;
} finally {
  await browser.close();
}
