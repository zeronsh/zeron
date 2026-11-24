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
//   node sessions.mjs tokens …the same…
//   node sessions.mjs down   --dir <folder>
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
import { USERNAMES } from "./catalog.mjs";

const [command, ...rest] = process.argv.slice(2);
const flag = (name) => {
  const i = rest.indexOf(name);
  return i < 0 ? undefined : rest[i + 1];
};
const need = (name) => flag(name) ?? (console.error(`sessions: give ${name}`), process.exit(2));
const dir = resolve(need("--dir"));
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
  for (let slot = 1; slot <= 6; slot++) await stop(slot);
  process.exit(0);
}
if (command !== "up" && command !== "tokens") {
  console.error("sessions: up | tokens | down");
  process.exit(2);
}

const zeron = resolve(need("--zeron"));
const edge = need("--edge").replace(/\/$/, "");
const clientId = need("--client-id");
const logins = JSON.parse(readFileSync(need("--logins"), "utf8"));
if (!Array.isArray(logins) || logins.length !== 6) throw new Error("sessions: the logins file lists six accounts");
for (const [i, l] of logins.entries()) {
  if (l.username !== USERNAMES[i + 1]) throw new Error(`sessions: account ${i + 1} is ${l.username}, the catalog's author ${i + 1} is ${USERNAMES[i + 1]}`);
}
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
  return { slot, env, call, close: () => ws.close() };
};

/** The account a bearer is (its JWT's `sub`), without verifying it: the
 * edge does that; this only checks the right person signed in. */
const subject = (bearer) => JSON.parse(Buffer.from(bearer.split(".")[1], "base64url").toString()).sub;

/** Complete AuthKit's hosted page at `url` as `login`, until the browser
 * reaches `until` (a URL prefix). Answers the page it lands on. */
const hostedSignIn = async (browser, url, login, until) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    await page.goto(url);
    const started = Date.now();
    while (!page.url().startsWith(until)) {
      if (Date.now() - started > 90_000) throw new Error(`the sign-in page never finished (at ${new URL(page.url()).origin}${new URL(page.url()).pathname})`);
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
    await page.screenshot({ path: join(dir, `sign-in-${login.username}.png`) }).catch(() => {});
    await context.close();
    throw error;
  }
};

const browser = await chromium.launch();
try {
  const accounts = [];
  for (let slot = 1; slot <= 6; slot++) {
    const login = logins[slot - 1];
    // A fresh sign-in comes back to the engine's loopback callback, on the
    // one port the sign-in's redirect list names (27641, every engine's):
    // so for a token, only this author's engine runs (`up` starts the rest).
    if (command === "tokens") for (let other = 1; other <= 6; other++) if (other !== slot) await stop(other, true);
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
        if (!claimed.ok) throw new Error(`author ${slot} couldn't claim ${login.username}: ${claimed.status} ${await claimed.text()}`);
      } else if (named.username !== login.username) {
        throw new Error(`author ${slot} already has username ${named.username}, not ${login.username}`);
      }
      const key = await d.call("PluginSigningKey");
      // The store sees the session as the CLI does.
      const status = await new Promise((res) => {
        const child = spawn(zeron, ["plugin", "status"], { env: { ...d.env, RUST_LOG: "warn" }, stdio: ["ignore", "ignore", "pipe"] });
        let err = "";
        child.stderr.on("data", (x) => (err += x));
        child.on("close", (code) => res({ code, err }));
      });
      if (status.code !== 0) throw new Error(`author ${slot}: \`zeron plugin status\` says ${status.err.trim().split("\n").pop()}`);
      console.log(`author ${slot}: @${login.username} signed in, in a workspace, signing key ${key?.registered ? "registered" : "NOT registered"}, engine on ${ipcPort(slot)}`);
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
        const child = spawn(zeron, ["plugin", "token", "create", "--all", "--name", "Seed CI"], { env: { ...d.env, RUST_LOG: "warn" }, stdio: ["ignore", "pipe", "pipe"] });
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
    writeFileSync(join(dir, "accounts.json"), `${JSON.stringify({ edge, workos_client_id: clientId, zeron, authors: accounts }, null, 2)}\n`);
    console.log(`accounts for seed.mjs: ${join(dir, "accounts.json")}`);
  }
} finally {
  await browser.close();
}
