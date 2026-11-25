#!/usr/bin/env node
// Seeds a store's templates through its real paths only: each author's own
// signed-in Zeron publishes its templates' first versions, as the app does
// (`PrepareTemplatePublish`, then `PublishTemplate` with the ticket it gave,
// over the Zeron's own IPC: signed with that device's key; the CLI publishes
// a template only with a token), and a CI-style checkout publishes the later
// ones with the author's deploy token (`ZERON_DEPLOY_TOKEN=… zeron template
// publish <folder>`, unsigned). The store is otherwise only read, to skip
// what's already there, so a run that stopped part way picks up where it was.
//
//   node seed-templates.mjs --accounts accounts-templates.json --ui <Zeron's apps/zgx/ui at the release> --playwright <dir> --edge <url> [--pictures <folder>] [--only 7,8] [--ids a,b] [--dry-run]
//
// Writes only to an edge SEED_ALLOWED_EDGES names, never production's, from a
// Zeron signed in to the accounts file's WorkOS (where.mjs).
//
// `--pictures` has the app's own renderings, <id>/preview.png, chat.png and
// new-chat.png (a pre-release's under <id>-beta), for the versions that use
// them; without it those versions wait.
//
// The accounts file is seed.mjs's (sessions.mjs writes it). Two passes, as
// seed.mjs: exits 3 when the first versions are published and tokens are
// needed for the rest (`zeron plugin token create --all --templates`), 0 once
// all are, 1 at the first refusal, 2 for a set-up problem. `--dry-run` checks
// each version with the CLI (`zeron template check`) and sends nothing.

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { call, whyNotTheirs } from "./ipc.mjs";
import { pictures } from "./pictures.mjs";
import { TEMPLATES as ALL, files } from "./templates.mjs";
import { whyNotSeeded } from "./where.mjs";

/** Between two writes of one account: the store takes 30 a minute. */
const PACE_MS = Number(process.env.SEED_PACE_MS ?? 2500);

const args = process.argv.slice(2);
const usage = (why) => {
  console.error(`seed-templates: ${why}`);
  process.exit(2);
};
const flag = (name) => {
  const i = args.indexOf(name);
  return i < 0 ? undefined : (args[i + 1] ?? usage(`${name} needs a value`));
};
const accounts = JSON.parse(readFileSync(flag("--accounts") ?? usage("give --accounts"), "utf8"));
const ui = resolve(flag("--ui") ?? usage("give --ui"));
if (!existsSync(join(ui, "ds/tokens.luau"))) usage(`${ui} isn't Zeron's apps/zgx/ui`);
const require = createRequire(join(resolve(flag("--playwright") ?? usage("give --playwright")), "package.json"));
const { chromium } = require("playwright");
const only = flag("--only")?.split(",").map(Number);
const rendered = flag("--pictures");
const ids = flag("--ids")?.split(",");
const unknownId = ids?.find((id) => !ALL.some((t) => t.id === id));
if (unknownId) usage(`there's no template ${unknownId}`);
const TEMPLATES = ALL.filter((t) => !ids || ids.includes(t.id));
const dryRun = args.includes("--dry-run");
const edge = flag("--edge")?.replace(/\/$/, "");
const refusedEdge = whyNotSeeded(edge, process.env.SEED_ALLOWED_EDGES);
if (!refusedEdge && accounts.edge !== undefined && accounts.edge.replace(/\/$/, "") !== edge) usage("the accounts file's sessions are another edge's");
if (refusedEdge) usage(refusedEdge);
const zeron = accounts.zeron ?? "zeron";
const slots = new Set(TEMPLATES.map((t) => t.slot));
const authors = (accounts.authors ?? []).filter((a) => slots.has(a.slot) && (!only || only.includes(a.slot)));
if (authors.length === 0) usage("no authors of templates in the accounts file");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const failed = (what, said) => {
  console.error(`FAIL ${what}\n${said}`);
  process.exit(1);
};

const cli = (argv, env) =>
  new Promise((res) => {
    const child = spawn(zeron, ["template", ...argv], { env: { PATH: process.env.PATH, RUST_LOG: "warn", ...env }, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    child.on("close", (code) => res({ code, out, err }));
  });

/** A CI job's environment: the edge, and no Zeron running nor signed in. */
const ci = async (argv, extra) => {
  const scratch = mkdtempSync(join(tmpdir(), "seed-ci-"));
  try {
    return await cli(argv, { HOME: scratch, ZERON_DATA_DIR: join(scratch, "data"), ZERON_IPC_PORT: "1", ZERON_EDGE_URL: edge, ...extra });
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
};

/** The app's own renderings of `t` at `version`, or undefined without them:
 * three still 16:10 PNGs. */
const renderings = (t, version) => {
  if (!rendered) return undefined;
  const dir = join(resolve(rendered), version.v.includes("-") ? `${t.id}-beta` : t.id);
  const out = {};
  for (const [name, file] of [["preview", "preview.png"], ["chat", "chat.png"], ["home", "new-chat.png"]]) {
    const path = join(dir, file);
    if (!existsSync(path)) usage(`there's no ${path}`);
    const bytes = readFileSync(path);
    const png = bytes.length > 24 && bytes.readUInt32BE(0) === 0x89504e47;
    if (!png || bytes.readUInt32BE(16) * 10 !== bytes.readUInt32BE(20) * 16) usage(`${path} isn't a 16:10 PNG`);
    out[name] = bytes;
  }
  return out;
};

/** The versions the store lists of `id`. */
const listed = async (id) => {
  const res = await fetch(`${edge}/store/v1/templates/${id}/versions?limit=50`);
  if (res.status === 404) return new Map();
  if (!res.ok) throw new Error(`the store answered ${res.status} for ${id}'s versions`);
  return new Map(((await res.json()).versions ?? []).map((v) => [v.version, v]));
};

const write = (dir, t, i, drawn) => {
  rmSync(dir, { recursive: true, force: true });
  for (const [path, bytes] of files(t, i, { ui, pictures: drawn })) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), bytes);
  }
};

let wrote = 0;
let skipped = 0;
const publishVersion = async (a, t, i, have, drawn) => {
  const version = t.versions[i];
  const what = `${t.id} ${version.v}`;
  if (have.has(version.v)) {
    skipped++;
    console.log(`have ${what} (${have.get(version.v).status})`);
    return;
  }
  if (version.rendered) {
    drawn = renderings(t, version);
    if (!drawn) {
      console.log(`waits ${what} (give --pictures)`);
      return;
    }
  }
  // The author's own copy: the template in their Zeron's folder.
  const own = join(a.data_dir, "templates", t.id);
  if (version.by === "device" && !dryRun) {
    write(own, t, i, drawn);
    let prepared;
    let answer;
    try {
      prepared = await call(a, "PrepareTemplatePublish", { id: t.id });
      if (prepared.ok !== true || !prepared.ticket) failed(`preparing ${what}`, JSON.stringify(prepared.problems ?? prepared, null, 2));
      answer = await call(a, "PublishTemplate", { id: t.id, hash: prepared.hash, ticket: prepared.ticket });
    } catch (error) {
      failed(`publishing ${what} from author ${a.slot}'s Zeron`, error.message);
    }
    if (answer.ok !== true) failed(`publishing ${what} from author ${a.slot}'s Zeron`, JSON.stringify(answer));
    for (const w of prepared.warnings ?? []) console.log(`  note ${what}: ${w.rule ?? ""} ${w.message ?? w.words ?? ""}`.trimEnd());
  } else {
    const checkout = mkdtempSync(join(tmpdir(), "seed-checkout-"));
    write(checkout, t, i, drawn);
    const ran = dryRun ? await ci(["check", checkout], {}) : await ci(["publish", checkout], { ZERON_DEPLOY_TOKEN: process.env[a.token_env] });
    rmSync(checkout, { recursive: true, force: true });
    if (ran.code !== 0) failed(`${dryRun ? "checking" : "publishing"} ${what} (exit ${ran.code})`, (ran.err || ran.out).trim());
  }
  wrote++;
  console.log(`${dryRun ? "ready" : "published"} ${what} (${version.by})`);
  if (!dryRun) await sleep(PACE_MS);
};

// Each author's Zeron is this edge's, before any of them publishes.
if (!dryRun) {
  for (const a of authors) {
    const why = await whyNotTheirs(a, accounts.workos_client_id);
    if (why) usage(`author ${a.slot}'s Zeron: ${why}`);
  }
}

const browser = await chromium.launch();
const drawn = new Map();
try {
  for (const t of TEMPLATES) drawn.set(t.id, await pictures(browser, t));
} finally {
  await browser.close();
}

// Pass 1: every first version, from its author's Zeron.
const haves = new Map();
for (const a of authors) {
  for (const t of TEMPLATES.filter((x) => x.slot === a.slot)) {
    haves.set(t.id, await listed(t.id));
    await publishVersion(a, t, 0, haves.get(t.id), drawn.get(t.id));
  }
}
// Pass 2: the rest.
const later = (a) => TEMPLATES.filter((t) => t.slot === a.slot).some((t) => t.versions.slice(1).some((v) => v.by === "token" && (rendered || !v.rendered) && !haves.get(t.id).has(v.v)));
const tokenless = authors.filter((a) => !process.env[a.token_env] && later(a));
if (tokenless.length && !dryRun) {
  console.log(`\nThe first versions are published. Now a deploy token for their templates too:\n  zeron plugin token create --all --templates --name "Seed CI"\nset as ${tokenless.map((a) => `$${a.token_env} (author ${a.slot})`).join(", ")}. Then run this again.`);
  process.exit(3);
}
for (const a of authors) {
  for (const t of TEMPLATES.filter((x) => x.slot === a.slot)) {
    for (let i = 1; i < t.versions.length; i++) await publishVersion(a, t, i, haves.get(t.id), drawn.get(t.id));
  }
}
console.log(`done: ${wrote} written, ${skipped} already there`);
