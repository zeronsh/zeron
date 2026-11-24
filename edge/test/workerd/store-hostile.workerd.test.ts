import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { author, makePackage, publish, uniqueId } from "./store-helpers";

const decoder = new TextDecoder();
const encoder = new TextEncoder();
const json = async (res: Response) => (await res.json()) as { error?: string; reason?: string; status?: string };

/** A package's JSON text with `member` added at its top level. */
const withMember = (pkg: Uint8Array, member: string): Uint8Array => {
  const text = decoder.decode(pkg);
  return encoder.encode(`${text.slice(0, -1)},${member}}`);
};

/** Publish and say how long the answer took (real time: it spans I/O). */
const timed = async (bearer: string, id: string, body: Uint8Array) => {
  const started = Date.now();
  const res = await publish(bearer, id, body);
  return { res, ms: Date.now() - started };
};

describe("hostile publish bodies cost time and memory in proportion to their size", () => {
  it("skips a huge unknown member (8 MB of zeros, a million empty objects) without building it", async () => {
    const a = await author();
    for (const [i, bulk] of [`[0${",0".repeat(4 * 1024 * 1024)}]`, `[{}${",{}".repeat(2 * 1024 * 1024)}]`].entries()) {
      const id = uniqueId(`bulk${i}`);
      const body = withMember(await makePackage({ id, version: "1.0.0", signer: a.signer }), `"x":${bulk}`);
      expect(body.length).toBeGreaterThan(4 * 1024 * 1024);
      const { res, ms } = await timed(a.userId, id, body);
      // An unknown member is the engine's to ignore too: the package stands.
      expect(res.status, await res.clone().text()).toBe(201);
      expect(ms).toBeLessThan(5_000);
    }
  });

  it("reads a 4 MB number once: in range it stands, out of range it isn't JSON serde_json takes", async () => {
    const a = await author();
    const small = uniqueId("tiny");
    const ok = withMember(await makePackage({ id: small, version: "1.0.0", signer: a.signer }), `"x":0.${"0".repeat(4 * 1024 * 1024)}1`);
    expect((await publish(a.userId, small, ok)).status).toBe(201);
    const huge = uniqueId("huge");
    const bad = withMember(await makePackage({ id: huge, version: "1.0.0", signer: a.signer }), `"x":1${"0".repeat(4 * 1024 * 1024)}`);
    const { res, ms } = await timed(a.userId, huge, bad);
    expect([res.status, (await json(res)).reason]).toEqual([400, "not_a_package"]);
    expect(ms).toBeLessThan(5_000);
  });

  it("decodes file data written with escapes straight from the body", async () => {
    const a = await author();
    const id = uniqueId("esc");
    // Every byte 0xff: base64 "////…", each "/" written as the escape "\/".
    const data = new Uint8Array(3 * 1024 * 1024).fill(0xff);
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer, files: { "big.bin": data } });
    const text = decoder.decode(pkg);
    const escaped = text.replace(/"big\.bin":"(\/+)"/, (_, slashes: string) => `"big.bin":"${slashes.replaceAll("/", "\\/")}"`);
    expect(escaped.length).toBeGreaterThan(text.length + 1024 * 1024);
    const { res, ms } = await timed(a.userId, id, encoder.encode(escaped));
    expect(res.status, await res.clone().text()).toBe(201);
    expect(ms).toBeLessThan(5_000);
    const stored = await env.STORE_DB.prepare("SELECT bytes FROM version_files WHERE plugin_id = ? AND path = 'big.bin'").bind(id).first<{ bytes: number }>();
    expect(stored?.bytes).toBe(data.length);
  });

  it("refuses a manifest past what the store builds, before building it", async () => {
    const a = await author();
    const id = uniqueId("fat");
    const pkg = decoder.decode(await makePackage({ id, version: "1.0.0", signer: a.signer }));
    const fat = encoder.encode(pkg.replace('"manifest":{', `"manifest":{"x":[0${",0".repeat(1024 * 1024)}],`));
    const { res, ms } = await timed(a.userId, id, fat);
    expect([res.status, (await json(res)).error]).toEqual([413, "too_large"]);
    expect(ms).toBeLessThan(5_000);
  });

  it("refuses a canonical manifest over 256 KB (an unknown field the engine keeps) and leaves nothing in R2", async () => {
    const a = await author();
    const id = uniqueId("big");
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer, manifest: { notes: "n".repeat(300 * 1024) } });
    const res = await publish(a.userId, id, pkg);
    expect([res.status, (await json(res)).error]).toEqual([413, "too_large"]);
    expect((await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` })).objects).toEqual([]);
    expect((await env.PLUGIN_STORE.list({ prefix: `files/${id}/` })).objects).toEqual([]);
  });

  it("reads a CHANGELOG.md line of many #s in linear time, and refuses one over 512 KB", async () => {
    const a = await author();
    const id = uniqueId("log");
    const line = `# ${"#".repeat(200_000)}x\n## 1.0.0\n- Notes.\n`;
    const { res, ms } = await timed(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer, files: { "CHANGELOG.md": line } }));
    expect(res.status, await res.clone().text()).toBe(201);
    expect(ms).toBeLessThan(3_000);
    const big = uniqueId("log");
    const over = await publish(a.userId, big, await makePackage({ id: big, version: "1.0.0", signer: a.signer, files: { "CHANGELOG.md": `## 1.0.0\n${"x".repeat(513 * 1024)}` } }));
    expect([over.status, (await json(over)).error]).toEqual([413, "too_large"]);
  });

  it("refuses a stranger's key before any listing work", async () => {
    const a = await author();
    const stranger = await author();
    const id = uniqueId("key");
    // A listing that's wrong in every way: the key is what's said.
    const pkg = await makePackage({ id, version: "1.0", signer: stranger.signer, manifest: { categories: ["games"], description: undefined } });
    const res = await publish(a.userId, id, pkg);
    expect([res.status, (await json(res)).error]).toEqual([403, "key_not_allowed"]);
  });
});
