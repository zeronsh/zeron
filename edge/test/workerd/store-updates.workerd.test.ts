import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { author, call, listed, published, uniqueId, adminCall } from "./store-helpers";

type Updates = { updates: Record<string, unknown>[]; blocked: Record<string, unknown>[] };
const json = async <T>(res: Response) => (await res.json()) as T;
const hashOf = async (id: string, version: string) =>
  (await env.STORE_DB.prepare("SELECT content_hash FROM versions WHERE plugin_id = ? AND version = ?").bind(id, version).first<{ content_hash: string }>())!.content_hash;
const check = async (body: unknown, bearer?: string) => {
  const res = await call(bearer, "POST", "/store/v1/updates", body);
  expect(res.status, await res.clone().text()).toBe(200);
  return json<Updates>(res);
};

describe("POST /store/v1/updates", () => {
  it("offers the highest listed version the device's Zeron takes, with what it adds", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await listed(a, id, "1.1.0", { manifest: { permissions: ["ui.add", "links.open"] } });
    await listed(a, id, "1.2.0", { manifest: { engines: { zeron: ">=0.5.0" } } });
    await listed(a, id, "2.0.0-beta.1");
    const installed = { id, version: "1.0.0", hash: await hashOf(id, "1.0.0") };
    const old = await check({ zeron: "0.4.2", plugins: [installed] });
    expect(old.updates).toEqual([
      expect.objectContaining({ id, version: "1.1.0", added: ["links.open"], removed: [], prerelease: false, hash: await hashOf(id, "1.1.0"), key: a.signer.key, publisher: a.username, notes: "- What changed in 1.1.0.", verified: true, verified_at: expect.any(Number) })
    ]);
    expect(old.updates[0]).not.toHaveProperty("in_review");
    expect(old.updates[0]).not.toHaveProperty("verified_update");
    expect((await check({ zeron: "0.5.0", plugins: [installed] })).updates[0]).toMatchObject({ version: "1.2.0" });
    expect((await check({ zeron: "0.5.0", plugins: [{ ...installed, prerelease: true }] })).updates[0]).toMatchObject({ version: "2.0.0-beta.1", prerelease: true });
    // Up to date: nothing.
    expect((await check({ zeron: "0.5.0", plugins: [{ id, version: "1.2.0", hash: await hashOf(id, "1.2.0") }] })).updates).toEqual([]);
    // Yanked: not offered.
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.2.0/yank`)).status).toBe(200);
    expect((await check({ zeron: "0.5.0", plugins: [installed] })).updates[0]).toMatchObject({ version: "1.1.0" });
  });

  it("offers every listed version at once, to everyone, saying whether it is verified, with the newest verified beside it", async () => {
    const a = await author();
    const someone = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await listed(a, id, "1.1.0");
    await published(a, id, "1.2.0");
    const body = { zeron: "0.4.0", plugins: [{ id, version: "1.0.0", hash: await hashOf(id, "1.0.0") }] };
    // The same offer to anyone (no version is only its author's any more).
    const offers = [await check(body), await check(body, someone.userId), await check(body, a.userId)].map((r) => r.updates);
    expect(offers[1]).toEqual(offers[0]);
    expect(offers[2]).toEqual(offers[0]);
    const [offer] = offers[0]!;
    expect(offer).toMatchObject({ version: "1.2.0", verified: false, verified_at: null });
    expect(offer).not.toHaveProperty("in_review");
    expect(offer!.verified_update).toMatchObject({ id, version: "1.1.0", verified: true, verified_at: expect.any(Number), hash: await hashOf(id, "1.1.0") });
    // Asking for verified versions only: the newest verified is the offer.
    const vetted = await check({ ...body, plugins: [{ ...body.plugins[0], verified_only: true }] });
    expect(vetted.updates).toEqual([expect.objectContaining({ version: "1.1.0", verified: true })]);
    expect(vetted.updates[0]).not.toHaveProperty("verified_update");
    // Installed at the newest verified: the unverified one is offered alone.
    const fromVetted = await check({ zeron: "0.4.0", plugins: [{ id, version: "1.1.0", hash: await hashOf(id, "1.1.0") }] });
    expect(fromVetted.updates).toEqual([expect.objectContaining({ version: "1.2.0", verified: false })]);
    expect(fromVetted.updates[0]).not.toHaveProperty("verified_update");
    expect((await check({ zeron: "0.4.0", plugins: [{ id, version: "1.1.0", hash: await hashOf(id, "1.1.0"), verified_only: true }] })).updates).toEqual([]);
    expect(await check({ zeron: "0.4.0", plugins: [{ ...body.plugins[0], verified_only: "yes" }] })).toEqual({ updates: [], template_updates: [], blocked: [] });
  });

  it("names installed versions taken down with a block, for those contents only", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await listed(a, id, "1.1.0");
    const bad = await hashOf(id, "1.1.0");
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.1.0/take-down`, { reason: "It sends your files away." })).status).toBe(200);
    const res = await check({ zeron: "0.4.0", plugins: [{ id, version: "1.1.0", hash: bad }] });
    expect(res.blocked).toEqual([{ id, version: "1.1.0", reason: "It sends your files away.", item_kind: "plugin" }]);
    expect(res.updates).toEqual([]);
    expect((await check({ zeron: "0.4.0", plugins: [{ id, version: "1.1.0", hash: "0".repeat(64) }] })).blocked).toEqual([]);
    // Suspended: no updates, blocks still said.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/suspend`, { reason: "x" })).status).toBe(200);
    const suspended = await check({ zeron: "0.4.0", plugins: [{ id, version: "1.1.0", hash: bad }, ] });
    expect(suspended.blocked).toHaveLength(1);
    expect((await check({ zeron: "0.4.0", plugins: [{ id, version: "0.9.0", hash: "0".repeat(64) }] })).updates).toEqual([]);
  });

  it("never blocks a version staff removed without a take-down", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await listed(a, id, "1.1.0");
    const hash = await hashOf(id, "1.1.0");
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.1.0/remove`, { reason: "A copyright claim." })).status).toBe(200);
    const res = await check({ zeron: "0.4.0", plugins: [{ id, version: "1.1.0", hash }] });
    expect(res).toEqual({ updates: [], template_updates: [], blocked: [] });
  });

  it("answers an item whose id, version or hash isn't one as unknown, and the rest", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const down = uniqueId();
    await listed(a, down, "1.0.0");
    const taken = { id: down, version: "1.0.0", hash: await hashOf(down, "1.0.0") };
    expect((await adminCall("POST", `/store/v1/admin/plugins/${down}/versions/1.0.0/take-down`, { reason: "Harmful." })).status).toBe(200);
    const one = { id, version: "1.0.0", hash: await hashOf(id, "1.0.0") };
    await listed(a, id, "1.1.0");
    const odd = [
      { ...one, id: "Not An Id" }, { ...one, id: "Not An Id" }, { ...one, id: 7 }, { version: "1.0.0", hash: one.hash }, {},
      { ...one, id: "v33", version: `1.0.0-${"a".repeat(27)}` }, { ...one, id: "v-empty", version: "" }, { ...one, id: "v-one", version: "1" }, { ...one, id: "v-null", version: null }, { id: "v-none", hash: one.hash },
      { ...one, id: "h-short", hash: "short" }, { ...one, id: "h-upper", hash: "A".repeat(64) }, { ...one, id: "h-seven", hash: 7 }, { ...one, id: "p-text", prerelease: "yes" }
    ];
    const res = await check({ zeron: "0.4.0", plugins: [...odd, one, taken] });
    expect(res.updates).toEqual([expect.objectContaining({ id, version: "1.1.0" })]);
    expect(res.blocked).toEqual([{ id: down, version: "1.0.0", reason: "Harmful.", item_kind: "plugin" }]);
    // The same for the item itself: never offered, never blocked.
    expect(await check({ zeron: "0.4.0", plugins: [{ ...one, version: `1.0.0-${"a".repeat(27)}` }, { ...taken, hash: taken.hash.toUpperCase() }] })).toEqual({ updates: [], template_updates: [], blocked: [] });
    // They count towards the most one request holds.
    const many = Array.from({ length: 200 }, () => ({}));
    expect((await call(undefined, "POST", "/store/v1/updates", { zeron: "0.4.0", plugins: many })).status).toBe(200);
    expect((await call(undefined, "POST", "/store/v1/updates", { zeron: "0.4.0", plugins: [...many, {}] })).status).toBe(400);
  });

  it("refuses a malformed request", async () => {
    const bad = [
      undefined,
      {},
      { zeron: "0.4", plugins: [] },
      { zeron: "0.4.0", plugins: "x" },
      { zeron: "0.4.0", plugins: [null] },
      { zeron: "0.4.0", plugins: ["ab"] },
      { zeron: "0.4.0", plugins: [["ab"]] },
      { zeron: "0.4.0", plugins: [{ id: "ab", version: "1.0.0", hash: "0".repeat(64) }, { id: "ab", version: "1.0.0", hash: "0".repeat(64) }] },
      // (A repeat, whatever the versions and hashes are.)
      { zeron: "0.4.0", plugins: [{ id: "ab", version: "1.0.0", hash: "0".repeat(64) }, { id: "ab", version: "not one" }] },
      { zeron: "0.4.0", plugins: Array.from({ length: 201 }, (_, i) => ({ id: `p${i}x`, version: "1.0.0", hash: "0".repeat(64) })) }
    ];
    for (const body of bad) expect((await call(undefined, "POST", "/store/v1/updates", body)).status, JSON.stringify(body)?.slice(0, 80)).toBe(400);
    expect(await check({ zeron: "0.4.0", plugins: [] })).toEqual({ updates: [], template_updates: [], blocked: [] });
  });
});
