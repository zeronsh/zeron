#!/usr/bin/env node
// Seeds a store's catalog through its real paths only: each author's own
// signed-in Zeron publishes their plugins' first versions (signed with
// that device's key, `zeron plugin publish <id>`), a CI-style checkout
// publishes the later ones with the author's deploy token
// (`ZERON_DEPLOY_TOKEN=… zeron plugin publish <folder>`, unsigned), and
// the author's Zeron yanks what the catalog says to (`zeron plugin
// yank`). Nothing but the CLI talks to the store, and the store is only
// read (its public routes) to skip what's already published, so a run
// that stopped part way picks up where it was.
//
// The sessions come from outside: the seed never signs anyone in. Each
// author is a running Zeron, signed in as that account, with a username;
// and, for the second pass, a deploy token made by it (`zeron plugin token
// create --all`), which Zeron makes only once the account has a plugin in
// the store. So it runs in two passes: every plugin's first version from
// its author's Zeron, then the rest. Without the tokens it stops between
// the two and says which to make; run it again once they're set. See
// README.md for the accounts file.
//
//   node scripts/staging-seed/seed.mjs --accounts accounts.json --edge <url> [--only 1,3] [--dry-run]
//
// Writes only to an edge SEED_ALLOWED_EDGES names, never production's, from
// Zerons signed in to the accounts file's WorkOS (where.mjs).
//
// Exits 0 once every version of every plugin of the authors named is
// published (or already was), 1 at the first refusal (with what the CLI
// said), 2 for a usage or set-up problem, 3 when the first versions are
// published and deploy tokens are needed for the rest.

import { spawn } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { PLUGINS, files, manifestFields } from "./catalog.mjs";
import { whyNotTheirs } from "./ipc.mjs";
import { quiet } from "./quiet.mjs";
import { whyNotItsWorkos, whyNotSeeded } from "./where.mjs";

const MANIFEST = "zeron-plugin.json";
/** Between two writes of one account: the store takes 30 a minute. */
const PACE_MS = Number(process.env.SEED_PACE_MS ?? 2500);

const usage = (why) => {
  console.error(`seed: ${why}\nusage: seed.mjs --accounts <file> [--only <slots>] [--dry-run]`);
  process.exit(2);
};

const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i < 0 ? undefined : (args[i + 1] ?? usage(`${name} needs a value`));
};
const accountsFile = flag("--accounts") ?? usage("give --accounts");
const only = flag("--only")?.split(",").map(Number);
const dryRun = args.includes("--dry-run");

const accounts = JSON.parse(readFileSync(accountsFile, "utf8"));
const edge = flag("--edge")?.replace(/\/$/, "");
const refusedEdge = whyNotSeeded(edge, process.env.SEED_ALLOWED_EDGES);
// (What an author's Zeron publishes goes to its own edge, whatever this is
// told: the file says which that is.)
const refusedFileEdge = whyNotSeeded(accounts.edge, process.env.SEED_ALLOWED_EDGES);
if (!refusedEdge && typeof accounts.edge === "string" && refusedFileEdge) usage(`the accounts file's edge: ${refusedFileEdge}`);
if (!refusedEdge && (typeof accounts.edge !== "string" || accounts.edge.replace(/\/$/, "") !== edge)) usage("the accounts file's edge isn't --edge");
const refusedWorkos = whyNotItsWorkos(accounts.workos_client_id);
if (!refusedEdge && refusedWorkos) usage(refusedWorkos);
if (refusedEdge) usage(refusedEdge);
const zeron = accounts.zeron ?? "zeron";
const authors = (accounts.authors ?? []).filter((a) => !only || only.includes(a.slot));
if (authors.length === 0) usage("no authors to seed");
for (const a of authors) {
  for (const key of ["slot", "home", "data_dir", "ipc_port", "token_env"]) if (a[key] === undefined) usage(`author ${a.slot ?? "?"} has no ${key}`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The CLI, as a person or a CI job runs it: `env` is all it gets (beside
 * PATH), so nothing of this machine's own Zeron leaks in. */
const cli = (argv, env) =>
  new Promise((resolve) => {
    const child = spawn(zeron, ["plugin", ...argv], { env: { PATH: process.env.PATH, RUST_LOG: "warn", ...env }, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    child.on("close", (code) => resolve({ code, out, err }));
  });

/** What an author's own Zeron is reached with. */
const asAuthor = (a) => ({
  HOME: a.home,
  ZERON_DATA_DIR: a.data_dir,
  ZERON_IPC_PORT: String(a.ipc_port),
  ZERON_EDGE_URL: edge,
  ...(accounts.workos_client_id ? { ZERON_WORKOS_CLIENT_ID: accounts.workos_client_id } : {})
});

/** The versions the store lists of `id`, by version: their status. */
const listed = async (id) => {
  const res = await fetch(`${edge}/store/v1/plugins/${id}/versions?limit=50`);
  if (res.status === 404) return new Map();
  if (!res.ok) throw new Error(`the store answered ${res.status} for ${id}'s versions`);
  const body = await res.json();
  return new Map((body.versions ?? []).map((v) => [v.version, v]));
};

const failed = (what, ran) => {
  const said = quiet((ran.err || ran.out).trim());
  console.error(`FAIL ${what} (exit ${ran.code})\n${said}`);
  // (An account's versions an hour and a day are the store's limit, not a fault.)
  if (said.includes("rate_limited")) console.error("The store takes 10 versions an account an hour, 50 a day: run this again later; it picks up where it stopped.");
  process.exit(1);
};

/** Write `plugin` at version index `i` into `dir` (the author's own copy):
 * its files, and its manifest over what `zeron plugin new` made, its old
 * signature dropped (the author's Zeron signs it again as it's saved). */
const write = (dir, plugin, i) => {
  for (const [path, bytes] of files(plugin, i)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), bytes);
  }
  const path = join(dir, MANIFEST);
  const manifest = JSON.parse(readFileSync(path, "utf8"));
  delete manifest.signature;
  delete manifest.network;
  delete manifest.commands;
  writeFileSync(path, `${JSON.stringify({ ...manifest, ...manifestFields(plugin, i) }, null, 2)}\n`);
};

/** A CI job's environment: the edge and what it's given, and nothing else (no
 * Zeron running, no sign-in; a port no Zeron listens on). */
const inCi = async (argv, extra = {}) => {
  const scratch = mkdtempSync(join(tmpdir(), "seed-ci-"));
  try {
    return await cli(argv, { HOME: scratch, ZERON_DATA_DIR: join(scratch, "data"), ZERON_IPC_PORT: "1", ZERON_EDGE_URL: edge, ...extra });
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
};

/** Check version `i` of `plugin` and nothing more: built in a folder of this
 * run's own (over a copy of `dir` where there is one, else over the least
 * `zeron plugin new` makes), never in a Zeron's. */
const checkVersion = async (dir, plugin, i) => {
  const at = existsSync(dir) ? checkout(dir) : mkdtempSync(join(tmpdir(), "seed-checkout-"));
  try {
    if (!existsSync(join(at, MANIFEST))) {
      writeFileSync(join(at, MANIFEST), JSON.stringify({ id: plugin.id, name: plugin.name, entry: "init.luau" }));
      writeFileSync(join(at, "init.luau"), "return {}\n");
    }
    write(at, plugin, i);
    return await inCi(["check", at]);
  } finally {
    rmSync(at, { recursive: true, force: true });
  }
};

/** A CI job's checkout of `dir`: its files, without dot files. */
const checkout = (dir) => {
  const to = mkdtempSync(join(tmpdir(), "seed-checkout-"));
  for (const name of readdirSync(dir)) if (!name.startsWith(".")) cpSync(join(dir, name), join(to, name), { recursive: true });
  return to;
};

let wrote = 0;
let skipped = 0;

/** Version `i` of `plugin`, by author `a`: published unless the store has
 * it, then yanked if the catalog says so. */
const publishVersion = async (a, plugin, i, have) => {
  const own = asAuthor(a);
  const dir = join(a.data_dir, "plugins", plugin.id);
  const version = plugin.versions[i];
  const what = `${plugin.id} ${version.v}`;
  const there = have.get(version.v);
  if (there) {
    skipped++;
    console.log(`have ${what} (${there.status})`);
  } else if (dryRun) {
    const ran = await checkVersion(dir, plugin, i);
    if (ran.code !== 0) failed(`checking ${what}`, ran);
    wrote++;
    console.log(`ready ${what} (${version.by})`);
  } else {
    if (!existsSync(dir)) {
      if (version.by !== "device") usage(`${plugin.id}'s first version must be the device's`);
      // (Its permissions are the manifest's, written next: `new` checks them
      // alone, before the programs or hosts they go with are named.)
      const made = await cli(["new", plugin.id, "--name", plugin.name], own);
      if (made.code !== 0) failed(`making ${plugin.id} on author ${a.slot}'s Zeron`, made);
    }
    write(dir, plugin, i);
    if (version.by === "device") {
      // (The author's Zeron signs a plugin it made again as it's saved: let it settle.)
      await sleep(1000);
      const ran = await cli(["publish", plugin.id], own);
      if (ran.code !== 0) failed(`publishing ${what} from author ${a.slot}'s Zeron`, ran);
    } else {
      const ci = checkout(dir);
      const ran = await inCi(["publish", ci], { ZERON_DEPLOY_TOKEN: process.env[a.token_env] });
      rmSync(ci, { recursive: true, force: true });
      if (ran.code !== 0) failed(`publishing ${what} with author ${a.slot}'s deploy token`, ran);
    }
    wrote++;
    console.log(`published ${what} (${version.by})`);
    await sleep(PACE_MS);
  }
  if (version.yank && !dryRun && (there?.status ?? "listed") !== "yanked") {
    const ran = await cli(["yank", plugin.id, version.v], own);
    if (ran.code !== 0) failed(`yanking ${what}`, ran);
    wrote++;
    console.log(`yanked ${what}`);
    await sleep(PACE_MS);
  }
};

// Each author's Zeron is this edge's, before anything else (a dry run's too,
// though it writes in none of them).
for (const a of authors) {
  const why = await whyNotTheirs(a, accounts.workos_client_id);
  if (why) usage(`author ${a.slot}'s Zeron: ${why}`);
}

// Pass 1: every plugin's first version, from its author's Zeron.
const haves = new Map();
for (const a of authors) {
  for (const plugin of PLUGINS.filter((p) => p.slot === a.slot)) {
    haves.set(plugin.id, await listed(plugin.id));
    await publishVersion(a, plugin, 0, haves.get(plugin.id));
  }
}

// Pass 2: the rest, in order: later versions from a token (or the device).
const later = (a) => PLUGINS.filter((p) => p.slot === a.slot).some((p) => p.versions.slice(1).some((v) => v.by === "token" && !haves.get(p.id).has(v.v)));
const tokenless = authors.filter((a) => !process.env[a.token_env] && later(a));
if (tokenless.length && !dryRun) {
  console.log(
    `\nThe first versions are published. Now each author makes a deploy token on their Zeron:\n  zeron plugin token create --all --name "Seed CI"\nand it's set as ${tokenless.map((a) => `$${a.token_env} (author ${a.slot})`).join(", ")}. Then run this again.`
  );
  process.exit(3);
}
for (const a of authors) {
  for (const plugin of PLUGINS.filter((p) => p.slot === a.slot)) {
    for (let i = 1; i < plugin.versions.length; i++) await publishVersion(a, plugin, i, haves.get(plugin.id));
  }
}

const stats = await (await fetch(`${edge}/store/v1/stats`)).json().catch(() => ({}));
console.log(`done: ${wrote} ${dryRun ? "checked" : "written"}, ${skipped} already there; the store lists ${stats.plugins ?? "?"} plugins (${stats.verified ?? "?"} verified)`);
