/**
 * The identity lane's probes of claims_zeron at e3a51b494, and what the
 * coordinator ruled after them (58fc12caf): a claim never outlives the badge.
 */
import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { adminCall, author, call, makePackage, publish, uniqueId } from "./store-helpers";

const json = async <T = Record<string, any>>(res: Response) => (await res.json()) as T;
const operator = (path: string, body: unknown) =>
  SELF.fetch(`https://edge.test${path}`, { method: "POST", headers: { authorization: "Bearer test-admin-token", "content-type": "application/json" }, body: JSON.stringify(body) });

describe("claims_zeron spelled as the mark's words may not be", () => {
  // Each reads as "by Zeron" / "Zeron approved" at a glance.
  for (const name of ["Git by Zer0n", "Git by Zеron", "Git by ᴢᴇʀᴏɴ", "Git bу Zeron", "z e r o n approved", "Z.E.R.O.N. approved", "Git by Z e r o n"]) {
    it(`refuses ${JSON.stringify(name)}`, async () => {
      const a = await author();
      const id = uniqueId();
      const res = await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer, manifest: { name } }));
      expect([res.status, (await json(res)).error]).toEqual([400, "claims_zeron"]);
    });
  }
});

describe("a listing that says it's Zeron's own outlives the badge that let it", () => {
  it("a transfer out of a maintainer's account keeps \"Git by Zeron\" listed under someone else", async () => {
    await operator("/admin/badges", { id: "maintainer", name: "Maintainer" });
    const zeron = await author();
    const b = await author();
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId] })).status).toBe(200);
    const id = uniqueId();
    expect((await publish(zeron.userId, id, await makePackage({ id, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git by Zeron" } }))).status).toBe(201);
    const moved = await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username });
    const card = await json(await call(undefined, "GET", `/store/v1/plugins/${id}`));
    // Either the transfer is refused, or the plugin no longer shows the claim under its new owner.
    const shown = moved.status === 200 && card.name === "Git by Zeron" && card.author?.username === b.username;
    expect(shown).toBe(false);
  });
});

describe("a claim doesn't outlive the badge", () => {
  it("a transfer to an account without the badge is refused until the plugin is renamed", async () => {
    await operator("/admin/badges", { id: "maintainer", name: "Maintainer" });
    const zeron = await author();
    const b = await author();
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId] })).status).toBe(200);
    const id = uniqueId();
    expect((await publish(zeron.userId, id, await makePackage({ id, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git by Zeron" } }))).status).toBe(201);
    const moved = await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username });
    expect([moved.status, (await json(moved)).error]).toEqual([409, "claims_zeron"]);
    // Renamed (the claim superseded, so it can't come back but by staff): it moves.
    expect((await publish(zeron.userId, id, await makePackage({ id, version: "1.1.0", signer: zeron.signer, manifest: { name: "Git tools" } }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username })).status).toBe(200);
  });

  it("revoking it suspends each of the account's plugins that would now be refused, named in the answer and the audit; granting it back relists none", async () => {
    await operator("/admin/badges", { id: "maintainer", name: "Maintainer" });
    const zeron = await author();
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId] })).status).toBe(200);
    const claims = uniqueId();
    const plain = uniqueId();
    const yanked = uniqueId();
    expect((await publish(zeron.userId, claims, await makePackage({ id: claims, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git by Zeron" } }))).status).toBe(201);
    expect((await publish(zeron.userId, plain, await makePackage({ id: plain, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git tools" } }))).status).toBe(201);
    // One its author yanked could come back by an unyank: it counts.
    expect((await publish(zeron.userId, yanked, await makePackage({ id: yanked, version: "1.0.0", signer: zeron.signer, manifest: { description: "Notes from Zeron." } }))).status).toBe(201);
    expect((await call(zeron.userId, "POST", `/store/v1/plugins/${yanked}/versions/1.0.0/yank`)).status).toBe(200);
    const revoke = await operator("/admin/badges/revoke", { badgeId: "maintainer", userIds: [zeron.userId] });
    expect([revoke.status, await json(revoke)]).toEqual([200, { revoked: 1, suspended: [claims, yanked].sort(), message: expect.stringContaining("staff restore") }]);
    // Out of the store at once; its installed copies keep running (nothing is blocked).
    expect((await call(undefined, "GET", `/store/v1/plugins/${claims}`)).status).toBe(404);
    expect((await call(undefined, "GET", `/store/v1/plugins/${plain}`)).status).toBe(200);
    const audit = (await json<{ entries: { action: string; detail: Record<string, unknown> }[] }>(await adminCall("GET", `/store/v1/admin/audit?plugin=${claims}`))).entries;
    expect(audit[0]).toMatchObject({ action: "plugin.suspend", detail: { claims_zeron: true } });
    const hash = (await (await call(zeron.userId, "GET", "/store/v1/me/plugins")).json() as any).plugins.find((p: { id: string }) => p.id === claims).versions[0].hash;
    expect((await json(await call(undefined, "POST", "/store/v1/updates", { zeron: "0.4.0", plugins: [{ id: claims, version: "1.0.0", hash }] }))).blocked ?? []).toEqual([]);
    // Its owner can't rename it: a suspended plugin takes no new version.
    const renamed = await publish(zeron.userId, claims, await makePackage({ id: claims, version: "1.1.0", signer: zeron.signer, manifest: { name: "Git tools" } }));
    expect([renamed.status, (await json(renamed)).error]).toEqual([403, "suspended"]);
    // A staff restore is refused while it would still say so…
    const early = await adminCall("POST", `/store/v1/admin/plugins/${claims}/restore`);
    expect([early.status, (await json(early)).error]).toEqual([409, "claims_zeron"]);
    // …and goes through once the versions that say so are taken down.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${yanked}/versions/1.0.0/take-down`, { reason: "Not Zeron's." })).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${yanked}/restore`)).status).toBe(200);
    // The badge back relists nothing by itself; a staff restore does (now allowed: its owner is Zeron's).
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId] })).status).toBe(200);
    expect((await call(undefined, "GET", `/store/v1/plugins/${claims}`)).status).toBe(404);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${claims}/restore`)).status).toBe(200);
    expect((await call(undefined, "GET", `/store/v1/plugins/${claims}`)).status).toBe(200);
  });

  it("revoking another badge suspends nothing, and Zeron's own plugin may move to another of Zeron's accounts", async () => {
    await operator("/admin/badges", { id: "maintainer", name: "Maintainer" });
    await operator("/admin/badges", { id: "helper", name: "Helper" });
    const zeron = await author();
    const other = await author();
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId, other.userId] })).status).toBe(200);
    expect((await operator("/admin/badges/grant", { badgeId: "helper", userIds: [zeron.userId] })).status).toBe(200);
    const id = uniqueId();
    expect((await publish(zeron.userId, id, await makePackage({ id, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git by Zeron" } }))).status).toBe(201);
    expect(await json(await operator("/admin/badges/revoke", { badgeId: "helper", userIds: [zeron.userId] }))).toEqual({ revoked: 1 });
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: other.username })).status).toBe(200);
  });
});
