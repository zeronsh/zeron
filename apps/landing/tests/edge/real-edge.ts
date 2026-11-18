// The real edge worker for the site's second tier of tests: its bundle as
// `wrangler deploy --dry-run` makes it, run by the store's local runner
// (scripts/plugin-store-e2e/edge.mjs: Miniflare, the store's D1 migrations,
// local R2 and Durable Objects, WorkOS faked but its tokens real), with the
// built site beside it in the same Miniflare, its EDGE service binding
// naming the edge as it does when deployed. Publishers sign in as a device
// does and publish real signed packages; staff sign in on the web through
// the site.
//
// E2E_ROOT names the checkout that holds edge/, docs/ and the runner (the
// repository by default); E2E_BUNDLE an already built bundle.
import { spawnSync } from "node:child_process";
import { createHash, generateKeyPairSync, sign, type KeyObject } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { siteWorker } from "../harness.ts";

const ROOT = resolve(process.env.E2E_ROOT ?? new URL("../../../../", import.meta.url).pathname);
const EDGE = join(ROOT, "edge");

/** The site's origin, the edge's WEB_ORIGIN: a name that is never anyone's real site. */
export const ORIGIN = "https://site.e2e.test";

export interface RealEdge {
  /** The edge worker's address, as devices and the CLI reach it. */
  url: string;
  /** The site's origin (the edge holds web sign-in and cookie writes to it). */
  origin: string;
  /** A request to the site, as from a browser on its origin. */
  site(path: string, init?: RequestInit): Promise<Response>;
  /** AuthKit's page, for a browser sent to `authorize` (api.workos.com/user_management/authorize?…): it signs `userId` in. */
  authorize(authorize: URL, userId: string): Promise<Response>;
  /** An operator grants `userId` (a user the edge has already met) a staff role. */
  grant(userId: string, role: "admin"): Promise<void>;
  stop(): Promise<void>;
}

interface Runner {
  EDGE_NAME: string;
  startEdge(options: Record<string, unknown>): Promise<{
    mf: { dispatchFetch(url: string, init?: RequestInit): Promise<Response> };
    url: string;
    workos: string;
    grantRole(userId: string, role: string): Promise<unknown>;
    stop(): Promise<void>;
  }>;
}

/** Bundle the edge (unless E2E_BUNDLE is given) and start it with the built site beside it. */
export async function startRealEdge(): Promise<RealEdge> {
  const work = mkdtempSync(join(tmpdir(), "zeron-site-edge-"));
  if (!existsSync(join(EDGE, "node_modules"))) run("npm", ["ci", "--no-audit", "--no-fund"], EDGE);
  let bundle = process.env.E2E_BUNDLE;
  if (!bundle) {
    bundle = join(work, "bundle");
    mkdirSync(join(EDGE, "public"), { recursive: true });
    run("npx", ["wrangler", "deploy", "--dry-run", "--outdir", bundle], EDGE);
  }
  const runner = (await import(pathToFileURL(join(ROOT, "scripts/plugin-store-e2e/edge.mjs")).href)) as Runner;
  const edge = await runner.startEdge({
    edgeDir: EDGE,
    bundleDir: bundle,
    siteOrigin: ORIGIN,
    workers: [siteWorker({ EDGE: runner.EDGE_NAME })],
    siteWorker: "site",
    site: false,
  });
  return {
    url: edge.url,
    origin: ORIGIN,
    site: (path, init = {}) => edge.mf.dispatchFetch(`${ORIGIN}${path}`, { redirect: "manual", ...init }),
    authorize: (authorize, userId) => fetch(`${edge.workos}${authorize.pathname}${authorize.search}&e2e_user=${encodeURIComponent(userId)}`, { redirect: "manual" }),
    async grant(userId, role) {
      // (The operator's route, as staff are made: not a way in for the tests.
      // Every admin call after this is the web session's.)
      await edge.grantRole(userId, role);
    },
    async stop() {
      await edge.stop();
      rmSync(work, { recursive: true, force: true });
    },
  };
}

function run(command: string, args: string[], cwd: string) {
  const done = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (done.status !== 0) throw new Error(`${command} ${args.join(" ")} failed:\n${done.stdout}\n${done.stderr}`);
}

// ── A publisher, as a device is one ─────────────────────────────────────

const b64url = (bytes: Uint8Array | Buffer) => Buffer.from(bytes).toString("base64url");

export const ICON = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor"/></svg>';
/** A 1×1 PNG, as the store's picture check takes it. */
export const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==", "base64");

export interface PackageOptions {
  manifest?: Record<string, unknown>;
  files?: Record<string, string | Uint8Array | undefined>;
}

export class Publisher {
  private constructor(
    readonly edge: string,
    readonly userId: string,
    readonly username: string,
    private readonly bearer: string,
    private readonly key: string,
    private readonly privateKey: KeyObject,
  ) {}

  /**
   * `userId` signs in as a device does (a code exchanged at the edge, which
   * earns a step-up ticket), claims `username`, and registers a new signing
   * key with that ticket and a proof of it.
   */
  static async signIn(edge: string, userId: string, username: string): Promise<Publisher> {
    const exchanged = await json(fetch(`${edge}/auth/exchange`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ code: `code:${userId}` }) }));
    const bearer = exchanged.accessToken as string;
    // (A fresh sign-in's ticket: { token, expiresAt }.)
    const stepUp = (exchanged.stepUp as { token: string }).token;
    await json(fetch(`${edge}/username`, { method: "POST", headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json" }, body: JSON.stringify({ username }) }));
    const { publicKey, privateKey } = generateKeyPairSync("ed25519");
    const key = publicKey.export({ format: "jwk" }).x as string;
    const proof = b64url(sign(null, Buffer.from(`zeron-plugin-key:v1:${key}`), privateKey));
    await json(fetch(`${edge}/plugin-keys`, { method: "POST", headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json" }, body: JSON.stringify({ key, proof, stepUp }) }));
    return new Publisher(edge, userId, username, bearer, key, privateKey);
  }

  /** A signed `.zeron-plugin`, as the engine packs one (the edge's own canonical JSON). */
  async package(id: string, version: string, o: PackageOptions = {}): Promise<Uint8Array<ArrayBuffer>> {
    const { canonicalJson, parseJsonText } = (await import(pathToFileURL(join(EDGE, "src/store/json.ts")).href)) as {
      canonicalJson(v: unknown): string;
      parseJsonText(t: string): unknown;
    };
    const prefix = (JSON.parse(readFileSync(join(ROOT, "docs/plugin-store-data.json"), "utf8")) as { payload_prefix: string }).payload_prefix;
    const files: Record<string, string | Uint8Array | undefined> = {
      "init.luau": `return "${id} ${version}"`,
      "README.md": `# ${id}\n\nIt does a thing.\n\n![A screen](shots/one.png)\n`,
      "CHANGELOG.md": `# Changes\n\n## ${version}\n\n- What changed in ${version}.\n`,
      "icon.svg": ICON,
      "shots/one.png": PNG,
      ...o.files,
    };
    const bytes = new Map<string, Buffer>();
    for (const [path, data] of Object.entries(files)) if (data !== undefined) bytes.set(path, Buffer.from(data));
    const fields: Record<string, unknown> = {
      id,
      name: id.replace(/-/g, " "),
      version,
      entry: "init.luau",
      description: `The ${id} plugin.`,
      icon: "icon.svg",
      categories: ["productivity"],
      keywords: ["focus"],
      permissions: ["ui.add"],
      screenshots: ["shots/one.png"],
      ...o.manifest,
    };
    for (const [k, v] of Object.entries(fields)) if (v === undefined) delete fields[k];
    const manifest = parseJsonText(JSON.stringify(fields)) as Map<string, unknown>;
    const hashes = new Map([...bytes.keys()].sort().map((p) => [p, createHash("sha256").update(bytes.get(p)!).digest("hex")]));
    const toSign = new Map(manifest);
    toSign.set("signature", new Map<string, unknown>([["key", this.key], ["files", new Map(hashes)]]));
    const sig = b64url(sign(null, Buffer.from(prefix + canonicalJson(toSign)), this.privateKey));
    const signed = JSON.parse(canonicalJson(manifest)) as Record<string, unknown>;
    signed.signature = { key: this.key, files: Object.fromEntries(hashes), sig };
    return new TextEncoder().encode(
      JSON.stringify({ format: "zeron-plugin:v1", manifest: signed, files: Object.fromEntries([...bytes].map(([p, d]) => [p, d.toString("base64")])) }),
    );
  }

  /** A deploy key for `id`, as `zeron plugin key create` makes one (a fresh sign-in's ticket with it). */
  async deployKey(id: string, name: string): Promise<string> {
    const exchanged = await json(fetch(`${this.edge}/auth/exchange`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ code: `code:${this.userId}` }) }));
    const { publicKey, privateKey } = generateKeyPairSync("ed25519");
    const key = publicKey.export({ format: "jwk" }).x as string;
    const proof = b64url(sign(null, Buffer.from(`zeron-deploy-key:v1:${id}:${key}`), privateKey));
    await json(fetch(`${this.edge}/store/v1/plugins/${id}/keys`, { method: "POST", headers: { authorization: `Bearer ${this.bearer}`, "content-type": "application/json" }, body: JSON.stringify({ key, name, proof, stepUp: (exchanged.stepUp as { token: string }).token }) }));
    return key;
  }

  /** Submit a version (it lands in review). */
  async publish(id: string, version: string, o: PackageOptions = {}): Promise<void> {
    const response = await fetch(`${this.edge}/store/v1/plugins/${id}/versions`, { method: "POST", headers: { authorization: `Bearer ${this.bearer}` }, body: await this.package(id, version, o) });
    if (response.status !== 201) throw new Error(`publishing ${id} ${version}: ${response.status} ${await response.text()}`);
  }
}

async function json(pending: Promise<Response>): Promise<Record<string, unknown>> {
  const response = await pending;
  const text = await response.text();
  if (!response.ok) throw new Error(`${response.url}: ${response.status} ${text}`);
  return JSON.parse(text) as Record<string, unknown>;
}
