/** Members, keys and signed packages for the store's workerd tests. */
import { SELF, env } from "cloudflare:test";
import { expect } from "vitest";
import { b64urlEncode } from "../../src/cloud/crypto";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import { MAX_LIVE_STEP_UPS, STEP_UP_TTL_MS, newStepUpToken, stepUpHash } from "../../src/step-up";
import { type Json, type JsonObject, canonicalJson, parseJsonText } from "../../src/store/json";
import { PAYLOAD_PREFIX, sha256Hex } from "../../src/store/package";

export const ADMIN = "user_storeadmin1";
export const ADMIN2 = "user_storeadmin2";

export const newUserId = () => `user_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;
export const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));

const encoder = new TextEncoder();

export const call = (bearer: string | undefined, method: string, path: string, body?: unknown, headers: Record<string, string> = {}) =>
  SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: {
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body === undefined || body instanceof Uint8Array ? {} : { "content-type": "application/json" }),
      ...headers
    },
    body: body === undefined ? undefined : body instanceof Uint8Array ? body : JSON.stringify(body)
  });

/** A directory user, with `username` claimed when given. */
export const member = async (username?: string): Promise<string> => {
  const userId = newUserId();
  const stamp = "2026-10-31T10:00:00.000Z";
  await directory().upsert({ id: userId, email: `${userId}@example.net`, first_name: "Ada", last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp });
  if (username) {
    const res = await call(userId, "POST", "/username", { username });
    expect(res.status).toBe(200);
  }
  return userId;
};

/** Random letters that never spell a reserved word or fold into another
 * name (no vowels, no digits, no l, m, n, o or r). */
const SAFE = "bcdfghjkpqvwxz";
export const randomLetters = (n: number): string =>
  [...crypto.getRandomValues(new Uint8Array(n))].map((b) => SAFE[b % SAFE.length]).join("");

/** A username no other test took. */
export const uniqueName = (prefix = "dv"): string => `${prefix}${randomLetters(10)}`;

export interface Signer {
  readonly key: string;
  sign(message: Uint8Array): Promise<string>;
}

export const newSigner = async (): Promise<Signer> => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  return {
    key,
    sign: async (message) => b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, message)))
  };
};

/** Bind `signer` to `userId` as one of their device keys. */
export const registerKey = async (userId: string, signer: Signer): Promise<void> => {
  const token = newStepUpToken();
  await directory().issueStepUp(userId, (await stepUpHash(token))!, STEP_UP_TTL_MS, MAX_LIVE_STEP_UPS);
  const res = await call(userId, "POST", "/plugin-keys", {
    key: signer.key,
    proof: await signer.sign(encoder.encode(PLUGIN_KEY_PROOF_PREFIX + signer.key)),
    stepUp: token
  });
  expect(res.status).toBe(201);
};

/** A member with a username and a registered key: ready to publish. */
export const author = async (prefix = "dv"): Promise<{ userId: string; username: string; signer: Signer }> => {
  const username = uniqueName(prefix);
  const userId = await member(username);
  const signer = await newSigner();
  await registerKey(userId, signer);
  return { userId, username, signer };
};

export const ICON = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/></svg>';

/** A 1×1 PNG image-check takes. */
export const PNG = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="),
  (c) => c.charCodeAt(0)
);

const b64 = (bytes: Uint8Array): string => {
  let s = "";
  for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return btoa(s);
};

export interface PackageOptions {
  readonly id: string;
  readonly version: string;
  readonly signer: Signer;
  /** Manifest fields over the defaults (undefined deletes one). */
  readonly manifest?: Record<string, unknown>;
  /** Files over the defaults (undefined deletes one). */
  readonly files?: Record<string, string | Uint8Array | undefined>;
}

/** A signed `.zeron-plugin`, as the engine packs one. */
export const makePackage = async (o: PackageOptions): Promise<Uint8Array> => {
  const files: Record<string, string | Uint8Array | undefined> = {
    "init.luau": `return "${o.id} ${o.version}"`,
    "README.md": `# ${o.id}\n\nIt does a thing. ![shot](shots/one.png)\n`,
    "CHANGELOG.md": `# Changes\n\n## ${o.version}\n\n- What changed in ${o.version}.\n\n## 0.0.1\n\n- Old.\n`,
    "icon.svg": ICON,
    "shots/one.png": PNG,
    ...o.files
  };
  const bytes = new Map<string, Uint8Array>();
  for (const [path, data] of Object.entries(files)) {
    if (data !== undefined) bytes.set(path, typeof data === "string" ? encoder.encode(data) : data);
  }
  const manifestFields: Record<string, unknown> = {
    id: o.id,
    name: o.id.replace(/-/g, " "),
    version: o.version,
    entry: "init.luau",
    description: `The ${o.id} plugin.`,
    icon: "icon.svg",
    categories: ["productivity"],
    keywords: ["focus", "timer"],
    permissions: ["ui.add"],
    screenshots: ["shots/one.png"],
    ...o.manifest
  };
  for (const [k, v] of Object.entries(manifestFields)) if (v === undefined) delete manifestFields[k];
  const manifest = parseJsonText(JSON.stringify(manifestFields)) as JsonObject;
  const hashes = new Map<string, string>();
  for (const path of [...bytes.keys()].sort()) hashes.set(path, await sha256Hex(bytes.get(path)!));
  const filesValue: JsonObject = new Map([...hashes].map(([k, v]) => [k, v as Json]));
  const toSign: JsonObject = new Map(manifest);
  toSign.set("signature", new Map<string, Json>([["key", o.signer.key], ["files", filesValue]]));
  const sig = await o.signer.sign(encoder.encode(PAYLOAD_PREFIX + canonicalJson(toSign)));
  const signed = JSON.parse(canonicalJson(manifest)) as Record<string, unknown>;
  signed.signature = { key: o.signer.key, files: Object.fromEntries(hashes), sig };
  return encoder.encode(
    JSON.stringify({ format: "zeron-plugin:v1", manifest: signed, files: Object.fromEntries([...bytes].map(([p, d]) => [p, b64(d)])) })
  );
};

export const publish = (bearer: string, id: string, pkg: Uint8Array) => call(bearer, "POST", `/store/v1/plugins/${id}/versions`, pkg);

/** Publish then approve: a listed version. */
export const listed = async (who: { userId: string; signer: Signer }, id: string, version: string, options: Partial<PackageOptions> = {}) => {
  const res = await publish(who.userId, id, await makePackage({ id, version, signer: who.signer, ...options }));
  expect(res.status, await res.clone().text()).toBe(201);
  const ok = await call(ADMIN, "POST", `/store/v1/admin/submissions/${id}/${version}/approve`);
  expect(ok.status, await ok.clone().text()).toBe(200);
};

/** A plugin id no other test took. */
export const uniqueId = (prefix = "p"): string => `${prefix}-${randomLetters(12)}`;
