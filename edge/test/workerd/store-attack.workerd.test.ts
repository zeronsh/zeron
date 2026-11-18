/**
 * The identity lane's probes of b467e6299, kept as they were written: each
 * failed then. The take-down feed's `after` read two ways; a restore and an
 * unyank listing an older unverified version beside a newer one; a new
 * owner reading the former owner's deploy-key names.
 */
import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { b64urlEncode } from "../../src/cloud/crypto";
import { deployKeyProofMessage, publishSigningString, scopeText, sha256Hex } from "../../src/store/identity/signing";
import { publisherId } from "../../src/store/cards";
import { stepUp } from "./identity-helpers";
import { type Signer, adminCall, author, call, makePackage, publish, published, uniqueId } from "./store-helpers";

const json = async <T = Record<string, any>>(res: Response) => (await res.json()) as T;
const feed = async (query: string) => json(await SELF.fetch(`https://edge.test/store/v1/takedowns${query}`));
const takeDown = (id: string, version: string) => adminCall("POST", `/store/v1/admin/plugins/${id}/versions/${version}/take-down`, { reason: "It sends your files away." });
const restore = (id: string, version: string) => adminCall("POST", `/store/v1/admin/plugins/${id}/versions/${version}/restore`, { reason: "A mistake." });

describe("the take-down feed's cache key and its handler read `after` differently", () => {
  for (const spelled of ["", "1e0", "%201", "0x1"]) {
    it(`?after=${spelled} is refused, and never fills the fresh start's cache entry`, async () => {
      const a = await author();
      const id = uniqueId();
      await published(a, id, "1.0.0");
      await published(a, id, "2.0.0");
      expect((await takeDown(id, "1.0.0")).status).toBe(200);
      // (The take-down purged the fresh start's entry here.) The odd spelling
      // is keyed as "no after", but answered as a number.
      // (As ruled: anything but digits is 400, never cached; the probe first
      // asked for a reset here.)
      const odd = await SELF.fetch(`https://edge.test/store/v1/takedowns?after=${spelled}`);
      expect([odd.status, (await json(odd)).error, odd.headers.get("cache-control")]).toEqual([400, "bad_request", "no-store"]);
      // A device with no cursor must be told to reset (run the full check).
      const fresh = await feed("");
      expect(fresh.reset ?? false).toBe(true);
    });
  }
});

describe("a restore relists an older unverified version beside a newer one", () => {
  it("when the newer one is yanked by its author at the time", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.0.0");
    expect((await takeDown(id, "1.0.0")).status).toBe(200);
    await published(a, id, "1.0.1");
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.1/yank`)).status).toBe(200);
    expect((await restore(id, "1.0.0")).status).toBe(200);
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.1/unyank`)).status).toBe(200);
    const versions = (await json<{ versions: { version: string; status: string; verified: boolean }[] }>(await call(undefined, "GET", `/store/v1/plugins/${id}/versions`))).versions;
    // The rule: only the newest unverified version on a line takes new installs.
    expect(versions.filter((v) => v.status === "listed" && !v.verified).map((v) => v.version)).toEqual(["1.0.1"]);
  });
});

describe("one account reads another's deploy keys", () => {
  it("a plugin's new owner sees the former owner's deploy key names in /me/plugins", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId("ci");
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
    const sign = async (m: Uint8Array) => b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, m)));
    const signer: Signer = { key, sign };
    const scope = { plugins: [id] };
    const proof = await sign(new TextEncoder().encode(deployKeyProofMessage(await publisherId(a.userId), null, scopeText(scope)!, key)));
    const made = await call(a.userId, "POST", "/store/v1/me/keys", { key, name: "acme-prod-release-bot", scope, proof, stepUp: await stepUp(a.userId) });
    expect(made.status).toBe(201);
    const pkg = await makePackage({ id, version: "1.1.0", signer });
    const ts = String(Date.now());
    const hash = await sha256Hex(pkg);
    const sig = await sign(new TextEncoder().encode(publishSigningString("POST", `/store/v1/plugins/${id}/versions`, hash, ts)));
    const res = await SELF.fetch(`https://edge.test/store/v1/plugins/${id}/versions`, {
      method: "POST",
      headers: { authorization: `ZeronKey key=${key}, ts=${ts}, sig=${sig}`, "x-zeron-content-sha256": hash },
      body: pkg
    });
    expect(res.status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username })).status).toBe(200);
    const mine = (await json<{ plugins: { id: string; versions: Record<string, any>[] }[] }>(await call(b.userId, "GET", "/store/v1/me/plugins"))).plugins.find((p) => p.id === id)!;
    const shown = mine.versions.find((v) => v.version === "1.1.0")!;
    expect(shown.deploy_key?.name ?? null).toBeNull();
  });
});

describe("the same shape elsewhere: the list's cache key and its handler read `q` alike", () => {
  it("a search with no searchable word answers nothing, and never for the list without a search", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.0.0");
    const odd = await json(await SELF.fetch(`https://edge.test/store/v1/plugins?q=${encodeURIComponent("!!!")}&sort=popular`));
    expect(odd).toEqual({ items: [], total: 0 });
    const plain = await json(await SELF.fetch("https://edge.test/store/v1/plugins?sort=popular"));
    expect(plain.total).toBeGreaterThan(0);
  });
});
