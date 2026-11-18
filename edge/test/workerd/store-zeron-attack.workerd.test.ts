/**
 * Zeron in a listing is not policed (Wing's ruling, contract fa75e5a40):
 * what the identity lane's probes refused once now publishes, a transfer of
 * such a plugin goes through, and taking Zeron's badge away only takes the
 * badge away. The badge, its grant and revoke stay.
 */
import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { adminCall, author, call, makePackage, publish, uniqueId } from "./store-helpers";

const json = async <T = Record<string, any>>(res: Response) => (await res.json()) as T;
const operator = (path: string, body: unknown) =>
  SELF.fetch(`https://edge.test${path}`, { method: "POST", headers: { authorization: "Bearer test-admin-token", "content-type": "application/json" }, body: JSON.stringify(body) });

describe("names the identity lane's probes refused once", () => {
  for (const name of ["Git by Zer0n", "Git by Zеron", "Git by ᴢᴇʀᴏɴ", "Git bу Zeron", "z e r o n approved", "Z.E.R.O.N. approved", "Git by Z e r o n"]) {
    it(`publish ${JSON.stringify(name)} from an account without Zeron's badge`, async () => {
      const a = await author();
      const id = uniqueId();
      expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer, manifest: { name } }))).status).toBe(201);
    });
  }
});

describe("Zeron's badge, given and taken", () => {
  it("lets a plugin named for Zeron move to any account, and a revoke suspends nothing", async () => {
    await operator("/admin/badges", { id: "maintainer", name: "Maintainer" });
    const zeron = await author();
    const b = await author();
    expect((await operator("/admin/badges/grant", { badgeId: "maintainer", userIds: [zeron.userId] })).status).toBe(200);
    const id = uniqueId();
    expect((await publish(zeron.userId, id, await makePackage({ id, version: "1.0.0", signer: zeron.signer, manifest: { name: "Git by Zeron" } }))).status).toBe(201);
    // Revoked: the badge goes, and only the badge.
    const revoke = await operator("/admin/badges/revoke", { badgeId: "maintainer", userIds: [zeron.userId] });
    expect([revoke.status, await json(revoke)]).toEqual([200, { revoked: 1 }]);
    expect(await json(await call(undefined, "GET", `/store/v1/plugins/${id}`))).toMatchObject({ id, name: "Git by Zeron" });
    const audit = (await json<{ entries: { action: string }[] }>(await adminCall("GET", `/store/v1/admin/audit?plugin=${id}`))).entries;
    expect(audit.map((e) => e.action)).not.toContain("plugin.suspend");
    // A transfer to an account without the badge goes through.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username })).status).toBe(200);
    expect(await json(await call(undefined, "GET", `/store/v1/plugins/${id}`))).toMatchObject({ author: { username: b.username } });
  });
});
