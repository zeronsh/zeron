#!/usr/bin/env node
/**
 * Measures what publishing a package of a given size costs the edge, to set
 * the store's package limit (src/store/publish.ts STORE_MAX_PACKAGE_BYTES)
 * from numbers rather than a guess. Results: docs/plugin-store-limits.md.
 *
 *   node scripts/store-package-limit.mjs [size in MB …] [--no-r2] [--chunks] [--shape=zeros|digits|objects|escapes|manifest|keys]
 *
 * For each size it starts a fresh local workerd (`wrangler dev` on
 * scripts/store-limit/, which runs the publish pipeline's heavy steps:
 * verify, listing, file kinds, checks, both R2 writes), sends a small
 * package to warm it, then the measured one, and reads the workerd
 * process's resident memory before, its peak (VmHWM) after, and the CPU
 * time the request took, from /proc. Linux only.
 *
 * The packages are the worst case for the edge: half the bytes random
 * binary files (incompressible, never valid text), half random text files,
 * 256 KB each, all signed.
 */
import { spawn } from "node:child_process";
import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sizes = process.argv.slice(2).filter((a) => !a.startsWith("--")).map(Number).filter((n) => n > 0);
const SIZES_MB = sizes.length ? sizes : [1, 5, 10, 15, 20, 25, 30, 40, 50, 67];
/** `--no-r2`: measure without the R2 writes (local R2 shares workerd's process). */
const R2 = !process.argv.includes("--no-r2");
/** `--shape=<name>`: a hostile body of that shape at each size instead of
 * a package (zeros, digits, escapes, objects, manifest, keys). */
const SHAPE = process.argv.find((a) => a.startsWith("--shape="))?.slice(8);
/** `--chunks`: read the body as the identity lane's readBody does. */
const CHUNKS = process.argv.includes("--chunks");
const FILE = 256 * 1024;
const TICKS = 100; // USER_HZ

const sorted = (v) =>
  Array.isArray(v) ? v.map(sorted) : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, sorted(v[k])])) : v;
/** Canonical JSON for ASCII keys and no floats (all this script makes). */
const canonical = (v) => JSON.stringify(sorted(v));
const sha256 = (b) => createHash("sha256").update(b).digest("hex");
const b64url = (b) => Buffer.from(b).toString("base64url");

const { publicKey, privateKey } = generateKeyPairSync("ed25519");
const key = b64url(publicKey.export({ format: "der", type: "spki" }).subarray(-32));

/** A hostile body of `bytes` bytes: valid-looking JSON whose bulk is the
 * shape named, beside a small valid package's members where that matters. */
const hostile = (shape, bytes) => {
  const small = JSON.parse(makePackage(64 * 1024).toString());
  const fill = (unit, room) => unit.repeat(Math.max(0, Math.floor(room / unit.length)));
  const wrap = (member) => {
    const head = JSON.stringify(small).slice(0, -1);
    return Buffer.from(`${head},${member}}`);
  };
  const room = bytes - JSON.stringify(small).length - 64;
  switch (shape) {
    case "zeros": return wrap(`"x":[0${fill(",0", room)}]`);
    case "digits": return wrap(`"x":0.${fill("0", room)}1`);
    case "objects": return wrap(`"x":[{}${fill(",{}", room)}]`);
    case "escapes": return Buffer.from(JSON.stringify({ ...small, files: { ...small.files, "big.bin": "@" } }).replace('"@"', `"${fill("\\/", room)}"`));
    case "manifest": return Buffer.from(JSON.stringify(small).replace('"manifest":{', `"manifest":{"x":[0${fill(",0", room)}],`));
    case "keys": return Buffer.from(JSON.stringify(small).replace('"files":{', `"files":{${Array.from({ length: Math.floor(room / 12) }, (_, i) => `"k${i}":"",`).join("")}`));
    default: throw new Error(`no shape ${shape}`);
  }
};

const makePackage = (bytes) => {
  const files = {
    "init.luau": Buffer.from("return 1"),
    "README.md": Buffer.from("# Measure\n\nA package for measuring.\n"),
    "icon.svg": Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>')
  };
  // base64 is 4/3 of the bytes; leave room for JSON and the manifest.
  const target = Math.floor((bytes * 3) / 4) - 4096;
  for (let i = 0, made = 0; made < target; i++) {
    const n = Math.min(FILE, target - made);
    files[`data/${String(i).padStart(4, "0")}.${i % 2 ? "bin" : "txt"}`] =
      i % 2 ? randomBytes(n) : Buffer.from(randomBytes(n).toString("base64").slice(0, n));
    made += n;
  }
  const hashes = Object.fromEntries(Object.entries(files).map(([p, b]) => [p, sha256(b)]));
  const manifest = {
    id: "measure",
    name: "Measure",
    version: "1.0.0",
    entry: "init.luau",
    description: "Measures.",
    icon: "icon.svg",
    categories: ["utilities"]
  };
  const payload = `zeron-plugin:v1\n${canonical({ ...manifest, signature: { key, files: hashes } })}`;
  const sig = b64url(sign(null, Buffer.from(payload), privateKey));
  return Buffer.from(
    JSON.stringify({
      format: "zeron-plugin:v1",
      manifest: { ...manifest, signature: { key, files: hashes, sig } },
      files: Object.fromEntries(Object.entries(files).map(([p, b]) => [p, b.toString("base64")]))
    })
  );
};

const children = (pid) =>
  readdirSync("/proc")
    .filter((d) => /^\d+$/.test(d))
    .filter((d) => {
      try {
        return readFileSync(`/proc/${d}/stat`, "utf8").split(") ")[1].split(" ")[1] === String(pid);
      } catch {
        return false;
      }
    })
    .map(Number);
const descendants = (pid) => children(pid).flatMap((c) => [c, ...descendants(c)]);
const comm = (pid) => readFileSync(`/proc/${pid}/comm`, "utf8").trim();
const status = (pid, field) => Number(/(\d+) kB/.exec(readFileSync(`/proc/${pid}/status`, "utf8").split("\n").find((l) => l.startsWith(field)))[1]) * 1024;
const cpu = (pid) => {
  const fields = readFileSync(`/proc/${pid}/stat`, "utf8").split(") ")[1].split(" ");
  return ((Number(fields[11]) + Number(fields[12])) / TICKS) * 1000;
};

const startWorker = (port, persist) =>
  new Promise((resolve, reject) => {
    const child = spawn("npx", ["wrangler", "dev", "--config", join(here, "store-limit/wrangler.jsonc"), "--port", String(port), "--persist-to", persist, "--log-level", "warn"], {
      cwd: join(here, ".."),
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, WRANGLER_SEND_METRICS: "false" },
      detached: true
    });
    const deadline = setTimeout(() => reject(new Error("workerd didn't start")), 60_000);
    const poll = setInterval(async () => {
      try {
        await fetch(`http://127.0.0.1:${port}/`, { method: "HEAD" });
        clearInterval(poll);
        clearTimeout(deadline);
        // wrangler runs two: its proxy and the worker's. Both are watched;
        // the one that did the work is reported.
        const workerds = descendants(child.pid).filter((p) => comm(p) === "workerd");
        resolve({ child, workerds });
      } catch {}
    }, 250);
  });

const mb = (n) => (n / (1024 * 1024)).toFixed(1);
const warm = makePackage(64 * 1024);
console.log("package MB | files | peak over idle MB | CPU ms | wall ms | answer");
for (const size of SIZES_MB) {
  const persist = mkdtempSync(join(tmpdir(), "store-limit-"));
  const port = 28100 + Math.floor(Math.random() * 800);
  const { child, workerds } = await startWorker(port, persist);
  try {
    await (await fetch(`http://127.0.0.1:${port}/`, { method: "POST", body: warm })).text();
    const pkg = SHAPE ? hostile(SHAPE, size * 1024 * 1024) : makePackage(size * 1024 * 1024);
    const idle = workerds.map((w) => status(w, "VmRSS"));
    const before = workerds.map(cpu);
    const res = await fetch(`http://127.0.0.1:${port}/?${R2 ? "" : "r2=0&"}${CHUNKS ? "read=chunks" : ""}`, { method: "POST", body: pkg });
    const text = await res.text();
    const cpus = workerds.map((w, i) => cpu(w) - before[i]);
    const worker = cpus.indexOf(Math.max(...cpus));
    const peak = status(workerds[worker], "VmHWM");
    const used = cpus[worker];
    const idleRss = idle[worker];
    const answer = res.ok ? JSON.parse(text) : { status: res.status, text: text.slice(0, 60) };
    console.log(`${mb(pkg.length)} | ${answer.files ?? "-"} | ${mb(peak - idleRss)} | ${used.toFixed(0)} | ${answer.wallMs ?? "-"} | ${res.status}${answer.refused ? ` ${answer.refused}` : ""}`);
  } finally {
    process.kill(-child.pid, "SIGTERM");
    await new Promise((r) => setTimeout(r, 500));
    rmSync(persist, { recursive: true, force: true });
  }
}
process.exit(0);
