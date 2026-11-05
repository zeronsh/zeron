#!/usr/bin/env node
// The three-engine staging rig: `node rig.mjs up | down | status`.
// up.sh / down.sh wrap it; lib.mjs holds the engines, ports and state dir.
//
//   A1  27891  test user U1, one device
//   A2  27892  test user U1, a second device (own data dir, own device id)
//   B   27893  test user U2
//
// Each engine has its own data dir and HOME (HOME, CLAUDE_CONFIG_DIR,
// CODEX_HOME) under $ZERON_RIG_STATE, and signs in against staging as a
// throwaway WorkOS test-env user, headlessly (playwright-core + system Chrome,
// a fresh profile: never the user's Chrome, which signs in as the real user).
// After sign-in the engine is restarted once, as zgx does
// (apps/zgx/src/engine/manager.rs `needs_restart`), so it opens the synced
// workspace instead of the local one it opened signed out.

import { spawn } from "node:child_process";
import { openSync, readFileSync, existsSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  STATE, BIN, STAGING, USERS, ENGINES, guard, ensureDir, writePrivate, sleep,
  ourPid, alive, commandOf, listeners, forgetPid, readPid, Ipc, ipcCall, ipcFirst,
  loadPlaywright, CHROME,
} from "./lib.mjs";

const log = (...a) => console.error("rig:", ...a);

// -- engines ------------------------------------------------------------------

function cleanEnv() {
  // Never inherit the engine this runs under (ZERON_IPC_PORT=27654,
  // ZERON_DATA_DIR, ZERON_APP, …).
  const env = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (!/^(ZERON|COMET)_/.test(k)) env[k] = v;
  }
  return env;
}

function start(e) {
  guard(e.port, e.data);
  if (!existsSync(BIN)) throw new Error(`${BIN} isn't built (cargo build -p zeron --bin zeron-engine)`);
  const busy = listeners(e.port);
  if (busy.length) throw new Error(`port ${e.port} is held by pid ${busy.join(",")} (${commandOf(busy[0])}), not the rig's ${e.name}`);
  for (const dir of [e.root, e.data, join(e.home, ".claude"), join(e.home, ".codex")]) ensureDir(dir);
  const out = openSync(join(e.root, "engine.out"), "a");
  const child = spawn(BIN, [], {
    cwd: e.root,
    detached: true,
    stdio: ["ignore", out, out],
    env: {
      ...cleanEnv(),
      HOME: e.home,
      CLAUDE_CONFIG_DIR: join(e.home, ".claude"),
      CODEX_HOME: join(e.home, ".codex"),
      ZERON_DATA_DIR: e.data,
      ZERON_IPC_PORT: String(e.port),
      ZERON_EDGE_URL: STAGING.edge,
      ZERON_WORKOS_CLIENT_ID: STAGING.client,
      ZERON_CALLBACK_PORT: String(STAGING.callbackPort),
      ZERON_AUTO_UPDATE: "0",
    },
  });
  writePrivate(e.pidFile, String(child.pid));
  writePrivate(join(e.root, "port"), String(e.port));
  child.unref();
  log(`${e.name}: started pid ${child.pid} on ${e.port}`);
  return child.pid;
}

function tail(path, n = 15) {
  try {
    return readFileSync(path, "utf8").trimEnd().split("\n").slice(-n).join("\n");
  } catch {
    return "(none)";
  }
}

/** Until the engine answers EngineInfo (a fresh token each try). */
async function waitReady(e, timeoutMs = 90000) {
  const until = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < until) {
    const pid = readPid(e);
    if (pid && !alive(pid)) {
      throw new Error(`${e.name}: engine pid ${pid} exited.\n--- engine.out ---\n${tail(join(e.root, "engine.out"))}\n--- logs/zeron-engine.log ---\n${tail(join(e.data, "logs/zeron-engine.log"))}`);
    }
    try {
      const info = await ipcCall(e, "EngineInfo", {}, 5000);
      if (info?.deviceId) return info;
    } catch (err) {
      last = err.message;
    }
    await sleep(500);
  }
  throw new Error(`${e.name}: no EngineInfo on ${e.port} in ${timeoutMs} ms (${last})`);
}

/** Running and answering: the rig's tracked engine, else started. */
async function ensureRunning(e) {
  const pid = ourPid(e);
  const held = listeners(e.port);
  if (pid && held.includes(pid)) return waitReady(e);
  if (pid && !held.length) return waitReady(e); // still starting
  if (held.length) {
    throw new Error(`port ${e.port} is held by pid ${held.join(",")} (${commandOf(held[0])}), which the rig didn't start; leaving it alone (ZERON_RIG_PORT_${e.name}=<port> ./up.sh moves ${e.name})`);
  }
  if (!pid) forgetPid(e);
  start(e);
  return waitReady(e);
}

async function stop(e) {
  const pid = ourPid(e);
  if (!pid) {
    forgetPid(e);
    return false;
  }
  try {
    await ipcCall(e, "StopEngine", {}, 5000);
  } catch {}
  for (let i = 0; i < 60 && alive(pid); i++) await sleep(500);
  if (alive(pid) && commandOf(pid).includes("zeron-engine")) {
    log(`${e.name}: pid ${pid} didn't stop on StopEngine; SIGTERM`);
    process.kill(pid, "SIGTERM");
    for (let i = 0; i < 30 && alive(pid); i++) await sleep(500);
  }
  if (alive(pid) && commandOf(pid).includes("zeron-engine")) {
    log(`${e.name}: pid ${pid} still up; SIGKILL`);
    process.kill(pid, "SIGKILL");
    for (let i = 0; i < 20 && alive(pid); i++) await sleep(250);
  }
  forgetPid(e);
  log(`${e.name}: stopped pid ${pid}`);
  return true;
}

async function restart(e) {
  await stop(e);
  // The port and the data dir's instance lock let go with the process.
  for (let i = 0; i < 40 && listeners(e.port).length; i++) await sleep(250);
  start(e);
  return waitReady(e);
}

// -- test users (WorkOS test env) ----------------------------------------------

const WORKOS_KEY_FILE = join(homedir(), ".config/zeron-staging/workos-api-key");

async function workos(method, path, body) {
  const key = readFileSync(WORKOS_KEY_FILE, "utf8").trim();
  if (!key.startsWith("sk_test_")) throw new Error(`${WORKOS_KEY_FILE} isn't a test-env key (sk_test_…); refusing`);
  const res = await fetch(`https://api.workos.com${path}`, {
    method,
    headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok) {
    const what = [json.code, json.message, ...(json.errors ?? []).map((x) => x.message ?? x.code)].filter(Boolean).join(": ");
    throw new Error(`WorkOS ${method} ${path.split("?")[0]} → ${res.status} ${what}`);
  }
  return json;
}

const passwordFile = (id) => join(STATE, "users", `${id}.password`);
const usersFile = join(STATE, "users", "users.json");

function readUsers() {
  try {
    return JSON.parse(readFileSync(usersFile, "utf8"));
  } catch {
    return {};
  }
}

/** The user exists, verified, with the password in its file (set each run). */
async function ensureUser(id) {
  const u = USERS[id];
  ensureDir(join(STATE, "users"));
  let password = existsSync(passwordFile(id)) ? readFileSync(passwordFile(id), "utf8").trim() : "";
  if (!password) {
    password = `${randomBytes(18).toString("base64url")}-Rg9`;
    writePrivate(passwordFile(id), password);
  }
  const found = (await workos("GET", `/user_management/users?email=${encodeURIComponent(u.email)}`)).data?.[0];
  let user;
  if (found) {
    user = await workos("PUT", `/user_management/users/${found.id}`, { password, email_verified: true });
    log(`${id}: reusing WorkOS user ${user.id} (${u.email})`);
  } else {
    user = await workos("POST", "/user_management/users", {
      email: u.email,
      password,
      first_name: u.first,
      last_name: u.last,
      email_verified: true,
    });
    log(`${id}: created WorkOS user ${user.id} (${u.email})`);
  }
  const all = readUsers();
  all[id] = { email: u.email, id: user.id };
  writePrivate(usersFile, JSON.stringify(all, null, 2) + "\n");
  return { ...u, id: user.id, password };
}

const ensured = {};
const userFor = (id) => (ensured[id] ??= ensureUser(id));

// -- sign-in ---------------------------------------------------------------------

/** The callback port is free, or held by `e` itself (its loopback stays bound). */
async function freeCallbackPort(e) {
  for (const pid of listeners(STAGING.callbackPort)) {
    const owner = ENGINES.find((x) => ourPid(x) === pid);
    if (owner === e) continue;
    if (!owner) {
      throw new Error(`callback port ${STAGING.callbackPort} is held by pid ${pid} (${commandOf(pid)}), not a rig engine; only one sign-in can run at a time`);
    }
    // A rig engine that started a sign-in keeps the loopback bound for its
    // life: restarting it lets go.
    log(`${owner.name} holds callback port ${STAGING.callbackPort}; restarting it to let go`);
    await restart(owner);
  }
}

async function driveBrowser(url, email, password, shots) {
  const { chromium } = await loadPlaywright();
  const browser = await chromium.launch({ executablePath: CHROME, headless: true });
  // A fresh, throwaway profile: nothing of the user's Chrome.
  const context = await browser.newContext();
  const page = await context.newPage();
  const clean = (u) => u.replace(/(state|code)=[^&]*/g, "$1=…");
  try {
    await page.goto(url);
    await page.waitForLoadState("networkidle");
    const toSignIn = page.getByRole("link", { name: /^sign in$/i });
    if (await toSignIn.count()) {
      await toSignIn.first().click();
      await page.waitForLoadState("networkidle");
    }
    await page.locator('input[type="email"], input[name="email"]').first().fill(email);
    await page.getByRole("button", { name: /continue/i }).first().click();
    const pw = page.locator('input[type="password"]').first();
    await pw.waitFor({ timeout: 20000 });
    await pw.fill(password);
    await page.getByRole("button", { name: /^(sign in|continue)$/i }).first().click();
    await page.waitForURL(/127\.0\.0\.1:\d+\/callback/, { timeout: 30000 });
    await page.waitForLoadState("load");
    const said = (await page.locator("body").innerText().catch(() => "")).replace(/\s+/g, " ").slice(0, 160);
    log(`browser landed on ${clean(page.url()).split("?")[0]}: "${said}"`);
  } catch (err) {
    ensureDir(shots);
    const shot = join(shots, `signin-failed-${Date.now()}.png`);
    await page.screenshot({ path: shot }).catch(() => {});
    const said = (await page.locator("body").innerText().catch(() => "")).replace(/\s+/g, " ").slice(0, 300);
    throw new Error(`headless sign-in stopped at ${clean(page.url())}: ${String(err).split("\n")[0]}\npage says: "${said}"\nscreenshot: ${shot}`);
  } finally {
    await browser.close();
  }
}

const signedOut = (s) => !s || s.state === "signedOut";

async function signIn(e) {
  const user = await userFor(e.user);
  await freeCallbackPort(e);
  const ipc = await Ipc.open(e.port, e.data);
  let state = null;
  const cancel = ipc.stream("AuthStatus", {}, (item) => (state = item));
  try {
    const answer = await ipc.call("SignIn", { screenHint: "sign-in", fresh: true });
    const url = answer?.url ?? "";
    if (!url.startsWith("https://")) throw new Error(`${e.name}: SignIn answered no sign-in page: ${JSON.stringify(answer)}`);
    log(`${e.name}: signing in as ${user.email} (headless Chrome)`);
    await driveBrowser(url, user.email, user.password, join(e.root, "shots"));
    for (let i = 0; i < 120 && signedOut(state); i++) await sleep(500);
    if (signedOut(state)) throw new Error(`${e.name}: the browser finished but AuthStatus still reads ${JSON.stringify(state)}`);
    log(`${e.name}: AuthStatus ${state.state} as ${state.user?.email}`);
  } finally {
    cancel();
    ipc.close();
  }
}

/** An organization chosen: the one named for the user, created if none. */
async function ensureOrg(e) {
  let s = await ipcFirst(e, "AuthStatus");
  if (s.state === "signedIn" && s.orgId) return s;
  if (s.state !== "needsOrganization") throw new Error(`${e.name}: AuthStatus ${JSON.stringify(s)}`);
  const want = USERS[e.user].org;
  const { orgs = [] } = await ipcCall(e, "ListOrgs");
  const match = orgs.find((o) => o.name === want) ?? orgs[0];
  if (match) {
    log(`${e.name}: selecting organization "${match.name}" (${match.organizationId})`);
    await ipcCall(e, "SelectOrg", { organizationId: match.organizationId });
  } else {
    log(`${e.name}: creating organization "${want}"`);
    await ipcCall(e, "CreateOrg", { name: want });
  }
  for (let i = 0; i < 60; i++) {
    s = await ipcFirst(e, "AuthStatus");
    if (s.state === "signedIn" && s.orgId) return s;
    await sleep(500);
  }
  throw new Error(`${e.name}: no organization after choosing one: ${JSON.stringify(s)}`);
}

/** Signed in, in an org, on its synced workspace. */
async function bringUp(e) {
  let info = await ensureRunning(e);
  let s = await ipcFirst(e, "AuthStatus");
  if (signedOut(s)) {
    if (info.workspaceScope !== "local") {
      // A synced engine that lost its session exits by itself; one that
      // didn't yet: start it over signed out.
      info = await restart(e);
    }
    await signIn(e);
  }
  s = await ensureOrg(e);
  info = await waitReady(e);
  if (info.workspaceScope === "local") {
    // Engine race (Oct 31): on a synced start with no key file yet,
    // Plugins::start's register_key and the auth watch's first pass both
    // run load_or_create_key, make two keys, and the registration record
    // can end up naming the key that lost the file write (registered=false
    // until the next restart). Making the key now, before that start,
    // sidesteps it. ZERON_RIG_KEY_RACE=1 skips this to reproduce it.
    if (process.env.ZERON_RIG_KEY_RACE !== "1") await ipcCall(e, "PluginSigningKey");
    log(`${e.name}: signed in on a local workspace; starting it again to sync`);
    info = await restart(e);
    s = await ipcFirst(e, "AuthStatus");
  }
  if (info.workspaceScope !== "synced") throw new Error(`${e.name}: workspace scope ${info.workspaceScope} after restart`);
  if (s.state !== "signedIn") throw new Error(`${e.name}: AuthStatus ${JSON.stringify(s)} after restart`);
  return { info, auth: s };
}

// -- report -------------------------------------------------------------------------

function devicesOf(item) {
  const list = Array.isArray(item) ? item : item?.devices ?? [];
  return list.map((d) => ({ id: d.deviceId ?? d.id, name: d.name, online: d.online ?? d.connected }));
}

async function signingKey(e, waitMs) {
  const until = Date.now() + waitMs;
  let k;
  do {
    k = await ipcCall(e, "PluginSigningKey");
    if (k?.registered) return k;
    await sleep(2000);
  } while (Date.now() < until);
  return k;
}

async function nameDevice(e, deviceId) {
  const name = `rig-${e.name}`;
  const devices = devicesOf(await ipcFirst(e, "WatchDevices"));
  if (devices.find((d) => d.id === deviceId)?.name === name) return;
  await ipcCall(e, "Mutate", { op: "renameDevice", deviceId, name });
}

async function describe(e, { keyWaitMs }) {
  const info = await ipcCall(e, "EngineInfo");
  const local = await ipcCall(e, "LocalDevice").catch((err) => ({ error: err.message }));
  const auth = await ipcFirst(e, "AuthStatus");
  const key = await signingKey(e, keyWaitMs);
  const devices = devicesOf(await ipcFirst(e, "WatchDevices"));
  return {
    name: e.name,
    port: e.port,
    pid: ourPid(e),
    data: e.data,
    home: e.home,
    deviceId: info.deviceId,
    localDevice: local.deviceId ?? local,
    scope: info.workspaceScope,
    auth: auth.state,
    email: auth.user?.email,
    userId: auth.user?.id,
    orgId: auth.orgId,
    signingKey: key?.key,
    keyRegistered: key?.registered,
    devices,
  };
}

function print(rows) {
  for (const r of rows) {
    console.log(`\n${r.name}  port ${r.port}  pid ${r.pid ?? "-"}`);
    console.log(`  data      ${r.data}`);
    console.log(`  home      ${r.home}`);
    console.log(`  device    ${r.deviceId}${r.localDevice !== r.deviceId ? `  (LocalDevice: ${JSON.stringify(r.localDevice)})` : ""}`);
    console.log(`  account   ${r.email ?? "-"}  user ${r.userId ?? "-"}  org ${r.orgId ?? "-"}  (${r.auth}, ${r.scope})`);
    console.log(`  signing   ${r.signingKey ?? "-"}  registered=${r.keyRegistered}`);
    console.log(`  devices   ${r.devices.map((d) => `${d.name ?? "?"}=${d.id}${d.online === undefined ? "" : d.online ? " (online)" : " (offline)"}`).join(", ") || "-"}`);
  }
}

function checks(rows) {
  const by = Object.fromEntries(rows.map((r) => [r.name, r]));
  const { A1, A2, B } = by;
  const results = [];
  const check = (ok, what) => results.push({ ok: !!ok, what });
  if (A1 && A2 && B) {
    for (const r of rows) check(r.auth === "signedIn" && r.scope === "synced", `${r.name} signed in on a synced workspace`);
    check(A1.userId && A1.userId === A2.userId, "A1 and A2 are the same account");
    check(A1.deviceId !== A2.deviceId, "A1 and A2 have different device ids");
    check(A1.devices.some((d) => d.id === A2.deviceId), "A1 lists A2's device");
    check(A2.devices.some((d) => d.id === A1.deviceId), "A2 lists A1's device");
    check(B.userId && B.userId !== A1.userId, "B is a different account");
    check(!B.devices.some((d) => d.id === A1.deviceId || d.id === A2.deviceId), "B doesn't list U1's devices");
    for (const r of rows) check(r.keyRegistered, `${r.name}'s plugin signing key is registered`);
  }
  console.log("");
  for (const c of results) console.log(`${c.ok ? "PASS" : "FAIL"}  ${c.what}`);
  return results.every((c) => c.ok);
}

// -- commands -------------------------------------------------------------------------

async function up() {
  ensureDir(STATE);
  for (const e of ENGINES) {
    const { info } = await bringUp(e);
    await nameDevice(e, info.deviceId).catch((err) => log(`${e.name}: renaming its device failed: ${err.message}`));
  }
  // Peers show up once each has synced the other's device row.
  let rows;
  for (let i = 0; i < 30; i++) {
    rows = await Promise.all(ENGINES.map((e) => describe(e, { keyWaitMs: i === 0 ? 60000 : 0 })));
    const [A1, A2] = rows;
    if (A1.devices.some((d) => d.id === A2.deviceId) && A2.devices.some((d) => d.id === A1.deviceId)) break;
    await sleep(2000);
  }
  // A key still unregistered after a minute: the engine registers on start,
  // so one restart (see the race note in bringUp).
  const stuck = ENGINES.filter((e, i) => !rows[i].keyRegistered);
  if (stuck.length) {
    for (const e of stuck) {
      log(`${e.name}: signing key ${rows.find((r) => r.name === e.name).signingKey} not registered after 60 s; restarting it once`);
      await restart(e);
    }
    rows = await Promise.all(ENGINES.map((e) => describe(e, { keyWaitMs: stuck.includes(e) ? 60000 : 0 })));
  }
  print(rows);
  writeFileSync(join(STATE, "engines.json"), JSON.stringify(rows, null, 2) + "\n");
  return checks(rows);
}

async function status() {
  const rows = [];
  for (const e of ENGINES) {
    if (!ourPid(e)) {
      console.log(`\n${e.name}  port ${e.port}  not running`);
      continue;
    }
    rows.push(await describe(e, { keyWaitMs: 0 }));
  }
  print(rows);
  return rows.length === ENGINES.length ? checks(rows) : true;
}

async function restartCmd() {
  const names = process.argv.slice(3).map((n) => n.toUpperCase());
  for (const e of ENGINES.filter((x) => !names.length || names.includes(x.name))) {
    if (!ourPid(e)) {
      log(`${e.name}: not running; use up.sh`);
      continue;
    }
    await restart(e);
  }
  return true;
}

async function down() {
  for (const e of ENGINES) {
    if (!(await stop(e))) {
      const held = listeners(e.port);
      log(`${e.name}: not running${held.length ? ` (port ${e.port} is held by pid ${held.join(",")}, not the rig's: left alone)` : ""}`);
    }
  }
  return true;
}

const commands = { up, down, status, restart: restartCmd };
const cmd = process.argv[2];
if (!commands[cmd]) {
  console.error("usage: node rig.mjs up | down | status | restart [A1 A2 B]");
  process.exit(2);
}
try {
  const ok = await commands[cmd]();
  process.exit(ok ? 0 : 1);
} catch (err) {
  console.error(`rig: ${err.message}`);
  process.exit(1);
}
