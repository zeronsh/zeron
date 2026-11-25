/** Members, keys and signed packages for the store's workerd tests. */
import { SELF, env } from "cloudflare:test";
import { expect } from "vitest";
import { b64urlEncode } from "../../src/cloud/crypto";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import { type Json, type JsonObject, canonicalJson, parseJsonText } from "../../src/store/json";
import { PAYLOAD_PREFIX, TEMPLATE_PAYLOAD_PREFIX, sha256Hex } from "../../src/store/package";
import { SITE, grantAdmin, person, sessionHeader, setCookies, stepUp, webSignIn } from "./identity-helpers";

/** Which staff account: two, for the tests where two admins race. */
export const ADMIN = 1;
export const ADMIN2 = 2;

interface Staff {
  readonly userId: string;
  cookie: string;
  readonly csrf: string;
}
const staff = new Map<number, Promise<Staff>>();

/**
 * A staff account (one per `slot`, made once per test file): a WorkOS user
 * signed in on the web site, granted `admin` by an operator
 * (`/admin/roles/grant`, ADMIN_TOKEN). Admin is by web session only
 * (amendment 27).
 */
export const admin = (slot = ADMIN): Promise<Staff> => {
  let made = staff.get(slot);
  if (!made) {
    made = (async () => {
      const userId = await person();
      await grantAdmin(userId);
      const browser = await webSignIn(userId);
      return { userId, cookie: browser.cookie, csrf: browser.csrf };
    })();
    staff.set(slot, made);
  }
  return made;
};

/**
 * An admin's request, as the web site's admin pages make it: the staff
 * session's cookie, the site's Origin and the session's CSRF token. A
 * rotated session cookie (the edge rotates a staff token on privileged use)
 * is kept for the next call.
 */
export const adminCall = async (method: string, path: string, body?: unknown, slot = ADMIN): Promise<Response> => {
  const s = await admin(slot);
  const res = await SELF.fetch(`${SITE}${path}`, {
    method,
    redirect: "manual",
    headers: {
      cookie: s.cookie,
      origin: SITE,
      "x-csrf-token": s.csrf,
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  });
  const rotated = setCookies(res).get("__Host-zeron_session");
  if (rotated) s.cookie = sessionHeader(rotated);
  return res;
};

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
  const res = await call(userId, "POST", "/plugin-keys", {
    key: signer.key,
    proof: await signer.sign(encoder.encode(PLUGIN_KEY_PROOF_PREFIX + signer.key)),
    stepUp: await stepUp(userId)
  });
  expect(res.status).toBe(201);
};

/** A deploy token of `userId`'s, made as the app or the CLI makes one. */
export const deployToken = async (
  userId: string,
  scope: { all: true; all_kinds?: ("plugin" | "template")[] } | { plugins: string[] },
  extra: Record<string, unknown> = {}
): Promise<{ token: string; id: string; short: string; name: string }> => {
  const res = await call(userId, "POST", "/store/v1/me/tokens", { name: "CI", scope, stepUp: await stepUp(userId), ...extra });
  expect(res.status, await res.clone().text()).toBe(201);
  return (await res.json()) as { token: string; id: string; short: string; name: string };
};

/** Publish `pkg` with deploy token `token`, as CI does. */
export const tokenPublish = (token: string, id: string, pkg: Uint8Array) =>
  call(undefined, "POST", `/store/v1/plugins/${id}/versions`, pkg, { authorization: `Bearer ${token}` });

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
  /** Who signs it; null for an unsigned package, as CI sends one with a
   * deploy token. */
  readonly signer: Signer | null;
  /** Manifest fields over the defaults (undefined deletes one). */
  readonly manifest?: Record<string, unknown>;
  /** Files over the defaults (undefined deletes one). */
  readonly files?: Record<string, string | Uint8Array | undefined>;
}

/** A `.zeron-plugin` as the engine packs one: signed by `signer`, or (null)
 * unsigned, as `zeron plugin publish` sends one with a deploy token. */
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
  const signed = JSON.parse(canonicalJson(manifest)) as Record<string, unknown>;
  if (o.signer) {
    const filesValue: JsonObject = new Map([...hashes].map(([k, v]) => [k, v as Json]));
    const toSign: JsonObject = new Map(manifest);
    toSign.set("signature", new Map<string, Json>([["key", o.signer.key], ["files", filesValue]]));
    const sig = await o.signer.sign(encoder.encode(PAYLOAD_PREFIX + canonicalJson(toSign)));
    signed.signature = { key: o.signer.key, files: Object.fromEntries(hashes), sig };
  }
  return encoder.encode(
    JSON.stringify({ format: "zeron-plugin:v1", manifest: signed, files: Object.fromEntries([...bytes].map(([p, d]) => [p, b64(d)])) })
  );
};

export interface TemplateOptions {
  readonly id: string;
  readonly version: string;
  /** Who signs it; null for an unsigned one, as CI sends one with a deploy token. */
  readonly signer: Signer | null;
  /** Manifest fields over the defaults (undefined deletes one). */
  readonly manifest?: Record<string, unknown>;
  /** Files over the defaults (undefined deletes one). */
  readonly files?: Record<string, string | Uint8Array | undefined>;
  /** A package that isn't a template's: its format, its signing domain. */
  readonly format?: string;
  readonly prefix?: string;
}

/** A `.zeron-template` as the engine packs one: signed by `signer` in the
 * template domain, or (null) unsigned. */
export const makeTemplate = async (o: TemplateOptions): Promise<Uint8Array> => {
  const files: Record<string, string | Uint8Array | undefined> = {
    "app.luau": `return "${o.id} ${o.version}"`,
    "ds/tokens.luau": "return {}",
    "README.md": `# ${o.id}\n\nA look for Zeron.\n`,
    "CHANGELOG.md": `# Changes\n\n## ${o.version}\n\n- What changed in ${o.version}.\n`,
    "preview.png": PNG,
    "screenshots/one.png": PNG,
    ...o.files
  };
  const bytes = new Map<string, Uint8Array>();
  for (const [path, data] of Object.entries(files)) {
    if (data !== undefined) bytes.set(path, typeof data === "string" ? encoder.encode(data) : data);
  }
  const fields: Record<string, unknown> = {
    id: o.id,
    name: o.id.replace(/-/g, " "),
    description: `The ${o.id} template.`,
    version: o.version,
    zeron_version: "0.3.0",
    keywords: ["dark", "layout"],
    screenshots: ["screenshots/one.png"],
    ...o.manifest
  };
  for (const [k, v] of Object.entries(fields)) if (v === undefined) delete fields[k];
  const manifest = parseJsonText(JSON.stringify(fields)) as JsonObject;
  const hashes = new Map<string, string>();
  for (const path of [...bytes.keys()].sort()) hashes.set(path, await sha256Hex(bytes.get(path)!));
  const signed = JSON.parse(canonicalJson(manifest)) as Record<string, unknown>;
  if (o.signer) {
    const toSign: JsonObject = new Map(manifest);
    toSign.set("signature", new Map<string, Json>([["key", o.signer.key], ["files", new Map([...hashes].map(([k, v]) => [k, v as Json]))]]));
    const sig = await o.signer.sign(encoder.encode((o.prefix ?? TEMPLATE_PAYLOAD_PREFIX) + canonicalJson(toSign)));
    signed.signature = { key: o.signer.key, files: Object.fromEntries(hashes), sig };
  }
  return encoder.encode(
    JSON.stringify({ format: o.format ?? "zeron-template:v1", manifest: signed, files: Object.fromEntries([...bytes].map(([p, d]) => [p, b64(d)])) })
  );
};

export const publishTemplate = (bearer: string | undefined, id: string, pkg: Uint8Array, headers: Record<string, string> = {}) =>
  call(bearer, "POST", `/store/v1/templates/${id}/versions`, pkg, headers);

/** Publish a template from its author's device: its answer's body. */
export const publishedTemplate = async (who: { userId: string; signer: Signer }, id: string, version: string, options: Partial<TemplateOptions> = {}) => {
  const res = await publishTemplate(who.userId, id, await makeTemplate({ id, version, signer: who.signer, ...options }));
  expect(res.status, await res.clone().text()).toBe(201);
  return (await res.json()) as { id: string; version: string; status: string; verified: boolean; superseded: string[] };
};

/** Publish a template, then staff verify it. */
export const listedTemplate = async (who: { userId: string; signer: Signer }, id: string, version: string, options: Partial<TemplateOptions> = {}) => {
  await publishedTemplate(who, id, version, options);
  const ok = await adminCall("POST", `/store/v1/admin/versions/${id}/${version}/verify`);
  expect(ok.status, await ok.clone().text()).toBe(200);
};

/** Publish a template with a deploy token, as CI does. */
export const tokenPublishTemplate = (token: string, id: string, pkg: Uint8Array) => publishTemplate(undefined, id, pkg, { authorization: `Bearer ${token}` });

export const publish = (bearer: string, id: string, pkg: Uint8Array) => call(bearer, "POST", `/store/v1/plugins/${id}/versions`, pkg);

/** Publish: listed at once, unverified (amendment 45). The answer's body. */
export const published = async (who: { userId: string; signer: Signer }, id: string, version: string, options: Partial<PackageOptions> = {}) => {
  const res = await publish(who.userId, id, await makePackage({ id, version, signer: who.signer, ...options }));
  expect(res.status, await res.clone().text()).toBe(201);
  return (await res.json()) as { id: string; version: string; status: string; verified: boolean; superseded: string[] };
};

/** Publish, then staff verify it: a listed, verified version (what an
 * approved one was before amendment 45; a newer version never supersedes it). */
export const listed = async (who: { userId: string; signer: Signer }, id: string, version: string, options: Partial<PackageOptions> = {}) => {
  await published(who, id, version, options);
  const ok = await adminCall("POST", `/store/v1/admin/versions/${id}/${version}/verify`);
  expect(ok.status, await ok.clone().text()).toBe(200);
};

/** A plugin id no other test took. */
export const uniqueId = (prefix = "p"): string => `${prefix}-${randomLetters(12)}`;
