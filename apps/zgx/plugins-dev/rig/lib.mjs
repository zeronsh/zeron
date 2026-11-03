// Shared pieces of the three-engine staging rig (see up.sh): where its state
// lives, the engines it runs, an IPC client, and the guards that keep it off
// the production engine.

import { readFileSync, existsSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { homedir, tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

export const RIG_DIR = dirname(fileURLToPath(import.meta.url));
export const REPO = resolve(RIG_DIR, "../../../..");

/** All runtime state (data dirs, homes, PIDs, users, passwords). */
export const STATE =
  process.env.ZERON_RIG_STATE || join(tmpdir(), "zeron-rig");

export const BIN = process.env.ZERON_RIG_BIN || join(REPO, "target/debug/zeron-engine");

export const STAGING = {
  edge: "https://staging.zeron.sh",
  client: "client_01KWD0E9SYPV2WE4QN9TP9DP0W",
  callbackPort: 27642,
};

export const USERS = {
  u1: { email: "zeron-plugin-rig-u1@example.net", first: "Rig", last: "One", org: "Plugin Rig U1" },
  u2: { email: "zeron-plugin-rig-u2@example.net", first: "Rig", last: "Two", org: "Plugin Rig U2" },
};

export const ENGINES = [
  { name: "A1", port: 27891, user: "u1" },
  { name: "A2", port: 27892, user: "u1" },
  { name: "B", port: 27893, user: "u2" },
].map((e) => {
  const root = join(STATE, e.name.toLowerCase());
  // A port taken by someone else: ZERON_RIG_PORT_<NAME>=<n> moves the engine,
  // and the choice is remembered in <root>/port until changed again.
  let port = e.port;
  const override = Number(process.env[`ZERON_RIG_PORT_${e.name}`]);
  if (override) port = override;
  else {
    try {
      port = Number(readFileSync(join(root, "port"), "utf8").trim()) || port;
    } catch {}
  }
  return { ...e, port, root, data: join(root, "data"), home: join(root, "home"), pidFile: join(root, "engine.pid") };
});

const PRODUCTION_PORT = 27654;
const PRODUCTION_DATA = [join(homedir(), ".comet-native"), join(homedir(), ".zeron")];

/** Refuse anything that could be the production engine. */
export function guard(port, data) {
  if (Number(port) === PRODUCTION_PORT) throw new Error(`port ${PRODUCTION_PORT} is the production engine's`);
  if (data) {
    const d = resolve(data);
    for (const p of PRODUCTION_DATA) {
      if (d === p || d.startsWith(p + "/")) throw new Error(`${p} is the production engine's`);
    }
  }
}

export function engineByPort(port) {
  return ENGINES.find((e) => e.port === Number(port));
}

export function ensureDir(dir, mode = 0o700) {
  mkdirSync(dir, { recursive: true, mode });
}

export function writePrivate(path, text) {
  ensureDir(dirname(path));
  writeFileSync(path, text, { mode: 0o600 });
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// -- processes ---------------------------------------------------------------

export function readPid(engine) {
  try {
    const pid = Number(readFileSync(engine.pidFile, "utf8").trim());
    return Number.isInteger(pid) && pid > 0 ? pid : null;
  } catch {
    return null;
  }
}

export function alive(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** The pid's command line, or "" once it's gone. */
export function commandOf(pid) {
  try {
    return execFileSync("/bin/ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" }).trim();
  } catch {
    return "";
  }
}

/** The tracked pid, if it is still the rig's engine. */
export function ourPid(engine) {
  const pid = readPid(engine);
  if (!pid || !alive(pid)) return null;
  return commandOf(pid).includes("zeron-engine") ? pid : null;
}

/** PIDs listening on a TCP port (127.0.0.1 or any). */
export function listeners(port) {
  try {
    return execFileSync("/usr/sbin/lsof", ["-nP", "-t", `-iTCP:${port}`, "-sTCP:LISTEN"], { encoding: "utf8" })
      .split("\n")
      .filter(Boolean)
      .map(Number);
  } catch {
    return [];
  }
}

export function forgetPid(engine) {
  rmSync(engine.pidFile, { force: true });
}

// -- IPC ----------------------------------------------------------------------

export function readToken(data) {
  try {
    const t = readFileSync(join(data, "ipc-token"), "utf8").trim();
    return t || null;
  } catch {
    return null;
  }
}

/**
 * One IPC connection: `{id, method, params}` out; `{id, ok}` / `{id, err}`
 * for calls, `{id, item}`* then `{id, done}` for streams (crates/rpc).
 */
export class Ipc {
  static open(port, data, timeoutMs = 5000) {
    guard(port, data);
    const token = readToken(data);
    if (!token) return Promise.reject(new Error(`no ipc-token in ${data} yet`));
    return new Promise((resolveOpen, rejectOpen) => {
      const ws = new WebSocket(`ws://127.0.0.1:${port}`, { headers: { Authorization: `Bearer ${token}` } });
      const timer = setTimeout(() => {
        try { ws.close(); } catch {}
        rejectOpen(new Error(`IPC on ${port}: no handshake in ${timeoutMs} ms`));
      }, timeoutMs);
      ws.onopen = () => {
        clearTimeout(timer);
        resolveOpen(new Ipc(ws));
      };
      ws.onerror = (ev) => {
        clearTimeout(timer);
        rejectOpen(new Error(`IPC on ${port}: ${ev?.message || ev?.error?.message || "connection failed"}`));
      };
    });
  }

  constructor(ws) {
    this.ws = ws;
    this.next = 1;
    this.pending = new Map();
    this.closed = false;
    ws.onerror = () => {};
    ws.onmessage = (ev) => {
      for (const line of String(ev.data).split("\n")) {
        if (!line.trim()) continue;
        let frame;
        try { frame = JSON.parse(line); } catch { continue; }
        const p = this.pending.get(frame.id);
        if (!p) continue;
        if (p.kind === "call") {
          this.pending.delete(frame.id);
          if ("err" in frame) p.reject(new Error(errText(frame.err)));
          else p.resolve(frame.ok);
        } else if ("err" in frame) {
          this.pending.delete(frame.id);
          p.end(new Error(errText(frame.err)));
        } else if (frame.done) {
          this.pending.delete(frame.id);
          p.end(null);
        } else if ("item" in frame) {
          p.item(frame.item);
        }
      }
    };
    ws.onclose = () => {
      this.closed = true;
      for (const [, p] of this.pending) {
        if (p.kind === "call") p.reject(new Error("closed"));
        else p.end(new Error("closed"));
      }
      this.pending.clear();
    };
  }

  send(method, params) {
    const id = this.next++;
    this.ws.send(JSON.stringify({ id, method, params: params ?? {} }));
    return id;
  }

  call(method, params = {}, timeoutMs = 30000) {
    if (this.closed) return Promise.reject(new Error("closed"));
    return new Promise((res, rej) => {
      const id = this.send(method, params);
      const timer = setTimeout(() => {
        this.pending.delete(id);
        rej(new Error(`${method}: no reply in ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(id, {
        kind: "call",
        resolve: (v) => { clearTimeout(timer); res(v); },
        reject: (e) => { clearTimeout(timer); rej(e); },
      });
    });
  }

  /** A stream: `onItem` per item; `onEnd(err|null)` once. Returns a cancel. */
  stream(method, params, onItem, onEnd = () => {}) {
    const id = this.send(method, params);
    this.pending.set(id, { kind: "stream", item: onItem, end: onEnd });
    return () => {
      if (this.pending.delete(id) && !this.closed) {
        try { this.ws.send(JSON.stringify({ id, cancel: true })); } catch {}
      }
    };
  }

  /** The first `n` items of a stream. */
  take(method, params = {}, n = 1, timeoutMs = 30000) {
    return new Promise((res, rej) => {
      const items = [];
      let cancel = () => {};
      const timer = setTimeout(() => { cancel(); rej(new Error(`${method}: ${items.length}/${n} items in ${timeoutMs} ms`)); }, timeoutMs);
      cancel = this.stream(method, params, (item) => {
        items.push(item);
        if (items.length >= n) { clearTimeout(timer); cancel(); res(items); }
      }, (err) => { clearTimeout(timer); err ? rej(err) : res(items); });
    });
  }

  close() {
    try { this.ws.close(); } catch {}
  }
}

function errText(err) {
  return typeof err === "string" ? err : JSON.stringify(err);
}

/** One call on a fresh connection. */
export async function ipcCall(engine, method, params = {}, timeoutMs = 30000) {
  const ipc = await Ipc.open(engine.port, engine.data);
  try {
    return await ipc.call(method, params, timeoutMs);
  } finally {
    ipc.close();
  }
}

export async function ipcFirst(engine, method, params = {}, timeoutMs = 15000) {
  const ipc = await Ipc.open(engine.port, engine.data);
  try {
    return (await ipc.take(method, params, 1, timeoutMs))[0];
  } finally {
    ipc.close();
  }
}

// -- playwright ---------------------------------------------------------------

export async function loadPlaywright() {
  const root = join(homedir(), ".npm/_npx");
  let dirs = [];
  try {
    dirs = execFileSync("/bin/ls", [root], { encoding: "utf8" }).split("\n").filter(Boolean);
  } catch {}
  for (const d of dirs) {
    const pkg = join(root, d, "node_modules/playwright-core");
    if (existsSync(join(pkg, "package.json")) && existsSync(join(pkg, "index.mjs"))) {
      return await import(join(pkg, "index.mjs"));
    }
  }
  throw new Error("no playwright-core with a package.json under ~/.npm/_npx/*/node_modules (try `npx -y playwright-core --version`)");
}

export const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
