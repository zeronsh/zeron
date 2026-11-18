import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { ADMIN, ADMIN2, author, call, listed, makePackage, publish, uniqueId } from "./store-helpers";

const json = async <T = Record<string, unknown>>(res: Response) => (await res.json()) as T;
const pending = async (a: Awaited<ReturnType<typeof author>>, id: string, version: string, options = {}) => {
  const res = await publish(a.userId, id, await makePackage({ id, version, signer: a.signer, ...options }));
  expect(res.status, await res.clone().text()).toBe(201);
};
const audit = async (id: string) =>
  (await json<{ entries: { action: string; version: string | null; actor: { user_id: string } }[] }>(await call(ADMIN, "GET", `/store/v1/admin/audit?plugin=${id}`))).entries;

describe("store admin routes", () => {
  it("are for admins only", async () => {
    const a = await author();
    const id = uniqueId();
    await pending(a, id, "1.0.0");
    const routes: [string, string, unknown?][] = [
      ["GET", "/store/v1/admin/queue"],
      ["GET", `/store/v1/admin/submissions/${id}/1.0.0`],
      ["POST", `/store/v1/admin/submissions/${id}/1.0.0/approve`],
      ["POST", `/store/v1/admin/submissions/${id}/1.0.0/reject`, { reason: "x", message: "y" }],
      ["GET", `/store/v1/admin/plugins/${id}`],
      ["POST", `/store/v1/admin/plugins/${id}/suspend`, { reason: "x" }],
      ["POST", `/store/v1/admin/plugins/${id}/restore`],
      ["POST", `/store/v1/admin/plugins/${id}/feature`, { featured: true }],
      ["POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: a.username }],
      ["POST", `/store/v1/admin/plugins/${id}/versions/1.0.0/remove`, { block: true, reason: "x" }],
      ["GET", "/store/v1/admin/reports"],
      ["POST", "/store/v1/admin/reports/1/resolve", { resolution: "dismissed" }],
      ["GET", "/store/v1/admin/audit"],
      ["GET", "/store/v1/admin/reservations"],
      ["POST", "/store/v1/admin/reservations", { id: "x-y" }],
      ["DELETE", "/store/v1/admin/reservations/x-y"]
    ];
    for (const [method, path, body] of routes) {
      expect((await call(undefined, method, path, body)).status, `${method} ${path} anonymous`).toBe(401);
      expect((await call(a.userId, method, path, body)).status, `${method} ${path} as the author`).toBe(403);
      expect((await call(`runner:${ADMIN}@org_1:dev1`, method, path, body)).status, `${method} ${path} as a runner`).toBe(403);
    }
    // Nothing changed.
    expect((await env.STORE_DB.prepare("SELECT status FROM versions WHERE plugin_id = ?").bind(id).first())?.status).toBe("pending");
  });

  it("shows the queue oldest first, and a submission with its diff base, checks and publisher", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await pending(a, id, "1.1.0", { manifest: { permissions: ["ui.add", "network"], network: { hosts: ["localhost:8080"] } }, files: { "lib.wasm": new Uint8Array([0, 97, 115, 109]) } });
    const queue = await json<{ items: { plugin: { id: string }; version: string; publisher: { username: string } }[] }>(await call(ADMIN, "GET", "/store/v1/admin/queue"));
    const mine = queue.items.filter((i) => i.plugin.id === id);
    expect(mine).toEqual([expect.objectContaining({ version: "1.1.0", status: "pending", publisher: { username: a.username } })]);
    const sub = await json<Record<string, any>>(await call(ADMIN, "GET", `/store/v1/admin/submissions/${id}/1.1.0`));
    expect(sub.base.version).toBe("1.0.0");
    expect(sub.publisher).toMatchObject({ username: a.username, created_at: "2026-10-31T10:00:00.000Z", decisions: { approved: 1, rejected: 0 } });
    expect(sub.files.find((f: { path: string }) => f.path === "lib.wasm")).toMatchObject({ binary: true });
    expect(sub.base_files.map((f: { path: string }) => f.path)).not.toContain("lib.wasm");
    expect(sub.permissions_diff).toMatchObject({ added: ["network"], network_added: ["localhost:8080"] });
    expect(sub.permissions).toMatchObject({ network_local: ["localhost:8080"] });
    const checks = Object.fromEntries((sub.checks as { id: string; level: string }[]).map((c) => [c.id, c.level]));
    expect(checks).toMatchObject({ binary: "warn", hosts: "warn", since_approved: "warn", asks: "warn" });
    expect(sub.version.readme).toContain("It does a thing");
    expect(sub.manifest.id).toBe(id);
  });

  it("approves, rejects with what the author reads, and refuses every illegal transition", async () => {
    const a = await author();
    const id = uniqueId();
    await pending(a, id, "1.0.0");
    await pending(a, id, "1.1.0");
    const approve = (v: string) => call(ADMIN, "POST", `/store/v1/admin/submissions/${id}/${v}/approve`);
    const reject = (v: string) => call(ADMIN, "POST", `/store/v1/admin/submissions/${id}/${v}/reject`, { reason: "broken", message: "It doesn't start." });
    expect((await call(ADMIN, "POST", `/store/v1/admin/submissions/${id}/1.1.0/reject`, { reason: "Not A Word", message: "x" })).status).toBe(400);
    expect((await call(ADMIN, "POST", `/store/v1/admin/submissions/${id}/1.1.0/reject`, { reason: "broken" })).status).toBe(400);
    expect((await approve("1.0.0")).status).toBe(200);
    expect((await reject("1.1.0")).status).toBe(200);
    const illegal: [string, () => Promise<Response>][] = [
      ["approve approved", () => approve("1.0.0")],
      ["reject approved", () => reject("1.0.0")],
      ["approve rejected", () => approve("1.1.0")],
      ["reject rejected", () => reject("1.1.0")],
      ["withdraw approved", () => call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.0/withdraw`)],
      ["unyank approved", () => call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.0/unyank`)],
      ["yank rejected", () => call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/yank`)],
      ["remove rejected", () => call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/versions/1.1.0/remove`, { block: false, reason: "x" })],
      ["restore active", () => call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/restore`)]
    ];
    for (const [what, go] of illegal) {
      const res = await go();
      expect([res.status, (await json(res)).error], what).toEqual([409, "invalid_state"]);
    }
    const mine = await json<{ plugins: { id: string; versions: { version: string; status: string; review: Record<string, unknown> }[] }[] }>(
      await call(a.userId, "GET", "/store/v1/me/plugins")
    );
    const versions = mine.plugins.find((p) => p.id === id)!.versions;
    expect(versions.map((v) => [v.version, v.status])).toEqual([
      ["1.1.0", "rejected"],
      ["1.0.0", "approved"]
    ]);
    expect(versions[0]!.review).toMatchObject({ state: "rejected", reason: "broken", message: "It doesn't start." });
    expect((await audit(id)).map((e) => [e.action, e.version])).toEqual([
      ["version.reject", "1.1.0"],
      ["version.approve", "1.0.0"]
    ]);
  });

  it("lets two admins race on one submission: one decision lands, the other is told", async () => {
    const a = await author();
    const id = uniqueId();
    await pending(a, id, "1.0.0");
    const [one, two] = await Promise.all([
      call(ADMIN, "POST", `/store/v1/admin/submissions/${id}/1.0.0/approve`),
      call(ADMIN2, "POST", `/store/v1/admin/submissions/${id}/1.0.0/reject`, { reason: "broken", message: "No." })
    ]);
    expect([one.status, two.status].sort()).toEqual([200, 409]);
    expect(await audit(id)).toHaveLength(1);
  });

  it("removes a version with a block, and suspends, restores and transfers a plugin, each in the audit log", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await listed(a, id, "1.1.0");
    const remove = await call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/versions/1.1.0/remove`, { block: true, reason: "It sends your files away." });
    expect(remove.status).toBe(200);
    expect((await json<{ version: Record<string, unknown> }>(remove)).version).toMatchObject({ status: "removed", blocked: true });
    expect((await json<{ version: string }>(await call(undefined, "GET", `/store/v1/plugins/${id}`))).version).toBe("1.0.0");
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}/versions/1.1.0/package`)).status).toBe(404);
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/versions/1.1.0/package`)).status).toBe(200);
    expect((await call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/suspend`, { reason: "Under review." })).status).toBe(200);
    expect((await call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/restore`)).status).toBe(200);
    const moved = await call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username });
    expect(moved.status).toBe(200);
    expect(await json(moved)).toMatchObject({ owner: { username: b.username, user_id: b.userId }, status: "active" });
    expect((await json<{ author: { username: string } }>(await call(undefined, "GET", `/store/v1/plugins/${id}`))).author.username).toBe(b.username);
    // The new owner publishes; the old one can't.
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.2.0", signer: a.signer }))).status).toBe(403);
    expect((await publish(b.userId, id, await makePackage({ id, version: "1.2.0", signer: b.signer }))).status).toBe(201);
    expect((await call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: "no-such-member" })).status).toBe(400);
    expect((await audit(id)).map((e) => e.action)).toEqual([
      "plugin.transfer",
      "plugin.restore",
      "plugin.suspend",
      "version.remove",
      "version.approve",
      "version.approve"
    ]);
    const detail = await json<Record<string, any>>(await call(ADMIN, "GET", `/store/v1/admin/plugins/${id}`));
    expect(detail.versions.map((v: { version: string; status: string }) => [v.version, v.status])).toEqual([
      ["1.2.0", "pending"],
      ["1.1.0", "removed"],
      ["1.0.0", "approved"]
    ]);
  });

  it("takes reports from signed-in people on listed plugins, and resolves them", async () => {
    const a = await author();
    const reader = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    expect((await call(undefined, "POST", `/store/v1/plugins/${id}/reports`, { reason: "malware", text: "x" })).status).toBe(401);
    expect((await call(reader.userId, "POST", `/store/v1/plugins/${id}/reports`, { reason: "boring", text: "x" })).status).toBe(400);
    const made = await call(reader.userId, "POST", `/store/v1/plugins/${id}/reports`, { reason: "misleading", text: "It says it's official." });
    expect(made.status).toBe(201);
    const { id: reportId } = await json<{ id: number }>(made);
    // Once per person while it's open.
    expect(await json(await call(reader.userId, "POST", `/store/v1/plugins/${id}/reports`, { reason: "malware", text: "again" }))).toEqual({ id: reportId, status: "open" });
    const open = await json<{ reports: { id: number; reporter: { username: string }; plugin: { id: string } }[] }>(await call(ADMIN, "GET", "/store/v1/admin/reports"));
    expect(open.reports.find((r) => r.id === reportId)).toMatchObject({ reporter: { username: reader.username }, plugin: { id }, reason: "misleading" });
    expect((await call(ADMIN, "POST", `/store/v1/admin/reports/${reportId}/resolve`, { resolution: "closed" })).status).toBe(400);
    expect((await call(ADMIN, "POST", `/store/v1/admin/reports/${reportId}/resolve`, { resolution: "dismissed", note: "It is official." })).status).toBe(200);
    expect((await call(ADMIN, "POST", `/store/v1/admin/reports/${reportId}/resolve`, { resolution: "actioned" })).status).toBe(409);
    expect((await audit(id)).map((e) => e.action)).toContain("report.dismissed");
    // Not listed: nothing to report.
    const quiet = uniqueId();
    await pending(a, quiet, "1.0.0");
    expect((await call(reader.userId, "POST", `/store/v1/plugins/${quiet}/reports`, { reason: "malware", text: "" })).status).toBe(404);
  });

  it("pages the audit log newest first", async () => {
    const a = await author();
    const id = uniqueId();
    await pending(a, id, "1.0.0");
    for (let i = 0; i < 3; i++) {
      expect((await call(ADMIN, "POST", `/store/v1/admin/plugins/${id}/feature`, { featured: i % 2 === 0 })).status).toBe(200);
    }
    const entries = await audit(id);
    expect(entries.map((e) => e.action)).toEqual(["plugin.feature", "plugin.unfeature", "plugin.feature"]);
    expect(entries.every((e) => e.actor.user_id === ADMIN)).toBe(true);
  });
});

describe("store author routes", () => {
  it("withdraws, yanks and unyanks only one's own versions, from a session", async () => {
    const a = await author();
    const other = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await pending(a, id, "1.1.0");
    expect((await call(other.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/withdraw`)).status).toBe(404);
    expect((await call(undefined, "POST", `/store/v1/plugins/${id}/versions/1.1.0/withdraw`)).status).toBe(401);
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/withdraw`)).status).toBe(200);
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.0/yank`)).status).toBe(200);
    // Every version yanked: unlisted.
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(404);
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.0/unyank`)).status).toBe(200);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(200);
    expect((await audit(id)).map((e) => e.action)).toEqual(["version.unyank", "version.yank", "version.withdraw", "version.approve"]);
  });

  it("shows an author their plugins with every version, and no one else's", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId();
    await pending(a, id, "0.1.0-beta.1");
    const mine = await json<{ plugins: Record<string, any>[] }>(await call(a.userId, "GET", "/store/v1/me/plugins"));
    expect(mine.plugins).toHaveLength(1);
    expect(mine.plugins[0]).toMatchObject({
      id,
      listing: "unlisted",
      name: id.replace(/-/g, " "),
      author: { username: a.username },
      versions: [{ version: "0.1.0-beta.1", status: "pending", prerelease: true, review: { state: "pending" } }]
    });
    expect(mine.plugins[0]!.versions[0].checks.length).toBeGreaterThan(0);
    expect((await json<{ plugins: unknown[] }>(await call(b.userId, "GET", "/store/v1/me/plugins"))).plugins).toEqual([]);
    expect((await call(undefined, "GET", "/store/v1/me/plugins")).status).toBe(401);
  });

  it("never lets an owner read another's pending version", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId();
    await pending(a, id, "1.0.0");
    expect((await call(b.userId, "GET", `/store/v1/plugins/${id}/versions/1.0.0/package`)).status).toBe(404);
    expect((await call(b.userId, "GET", `/store/v1/plugins/${id}/files/1.0.0/init.luau`)).status).toBe(404);
    expect((await call(b.userId, "GET", `/store/v1/plugins/${id}/files/1.0.0/shots/one.png`)).status).toBe(404);
  });
});
