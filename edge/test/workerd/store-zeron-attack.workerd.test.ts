/**
 * The identity lane's probes of claims_zeron at e3a51b494, kept as written
 * but for two spellings that wait on the shared table's look-alikes for the
 * phrases' other letters (b y z n…): "Git by ᴢᴇʀᴏɴ" and "Git bу Zeron" (`it.todo`).
 */
import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { adminCall, author, call, makePackage, publish, uniqueId } from "./store-helpers";

const json = async <T = Record<string, any>>(res: Response) => (await res.json()) as T;
const operator = (path: string, body: unknown) =>
  SELF.fetch(`https://edge.test${path}`, { method: "POST", headers: { authorization: "Bearer test-admin-token", "content-type": "application/json" }, body: JSON.stringify(body) });

describe("claims_zeron spelled as the mark's words may not be", () => {
  // Each reads as "by Zeron" / "Zeron approved" at a glance.
  for (const name of ["Git by ᴢᴇʀᴏɴ", "Git bу Zeron"]) it.todo(`refuses ${JSON.stringify(name)} (the shared table's z, n, b, y)`);
  for (const name of ["Git by Zer0n", "Git by Zеron", "z e r o n approved", "Z.E.R.O.N. approved", "Git by Z e r o n"]) {
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

describe("Zeron's own badge doesn't leave an account whose plugins still say so", () => {
  it("refuses revoking it while one does, and the transfer the probe tried is refused 409 claims_zeron", async () => {
    await operator("/admin/badges", { id: "maintainer", name: "Maintainer" });
    const zeron = await author();
    const b = await author();
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId] })).status).toBe(200);
    const id = uniqueId();
    expect((await publish(zeron.userId, id, await makePackage({ id, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git by Zeron" } }))).status).toBe(201);
    const moved = await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username });
    expect([moved.status, (await json(moved)).error]).toEqual([409, "claims_zeron"]);
    const revoke = await operator("/admin/badges/revoke", { badgeId: "maintainer", userIds: [zeron.userId] });
    expect([revoke.status, await json(revoke)]).toEqual([409, expect.objectContaining({ error: "claims_zeron", plugins: [id] })]);
    // Taken down, it says nothing: the badge may go, and the plugin may move.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.0.0/take-down`, { reason: "Not Zeron's." })).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username })).status).toBe(200);
    expect((await operator("/admin/badges/revoke", { badgeId: "maintainer", userIds: [zeron.userId] })).status).toBe(200);
  });

  it("lets another badge come and go, and Zeron's own plugin move to another of Zeron's accounts", async () => {
    await operator("/admin/badges", { id: "maintainer", name: "Maintainer" });
    await operator("/admin/badges", { id: "helper", name: "Helper" });
    const zeron = await author();
    const other = await author();
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId, other.userId] })).status).toBe(200);
    expect((await operator("/admin/badges/grant", { badgeId: "helper", userIds: [zeron.userId] })).status).toBe(200);
    const id = uniqueId();
    expect((await publish(zeron.userId, id, await makePackage({ id, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git by Zeron" } }))).status).toBe(201);
    expect((await operator("/admin/badges/revoke", { badgeId: "helper", userIds: [zeron.userId] })).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: other.username })).status).toBe(200);
  });
});
