import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { CLAIMS_PER_WEEK } from "../../src/store/publish";
import { publisherId } from "../../src/store/cards";
import { releaseRows } from "../../src/store/release";
import { releaseUnusedIds } from "../../src/store/release";
import { ADMIN, ADMIN2, author, call, listed, makePackage, publish, uniqueId, adminCall } from "./store-helpers";

const json = async <T = Record<string, unknown>>(res: Response) => (await res.json()) as T;
const pending = async (a: Awaited<ReturnType<typeof author>>, id: string, version: string, options = {}) => {
  const res = await publish(a.userId, id, await makePackage({ id, version, signer: a.signer, ...options }));
  expect(res.status, await res.clone().text()).toBe(201);
};
const audit = async (id: string) =>
  (await json<{ entries: { action: string; version: string | null; actor: { user_id: string } }[] }>(await adminCall("GET", `/store/v1/admin/audit?plugin=${id}`))).entries;

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
      // Admin is by web session only: an admin's own bearer is refused.
      expect((await call(ADMIN, method, path, body)).status, `${method} ${path} with an admin's bearer`).toBe(403);
    }
    // Nothing changed.
    expect((await env.STORE_DB.prepare("SELECT status FROM versions WHERE plugin_id = ?").bind(id).first())?.status).toBe("pending");
  });

  it("shows the queue oldest first, and a submission with its diff base, checks and publisher", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await pending(a, id, "1.1.0", { manifest: { permissions: ["ui.add", "network"], network: { hosts: ["localhost:8080"] } }, files: { "lib.wasm": new Uint8Array([0, 97, 115, 109]) } });
    const queue = await json<{ items: { plugin: { id: string }; version: string; publisher: { username: string } }[] }>(await adminCall("GET", "/store/v1/admin/queue"));
    const mine = queue.items.filter((i) => i.plugin.id === id);
    expect(mine).toEqual([expect.objectContaining({ version: "1.1.0", status: "pending", publisher: { id: await publisherId(a.userId), username: a.username } })]);
    const sub = await json<Record<string, any>>(await adminCall("GET", `/store/v1/admin/submissions/${id}/1.1.0`));
    expect(sub.base.version).toBe("1.0.0");
    expect(sub.publisher).toMatchObject({ id: await publisherId(a.userId), username: a.username, created_at: "2026-10-31T10:00:00.000Z", decisions: { approved: 1, rejected: 0 } });
    expect(sub.plugin.owner).toEqual({ id: await publisherId(a.userId), username: a.username });
    expect(sub.files.find((f: { path: string }) => f.path === "lib.wasm")).toMatchObject({ binary: true });
    expect(sub.base_files.map((f: { path: string }) => f.path)).not.toContain("lib.wasm");
    expect(sub.permissions_diff).toMatchObject({ added: ["network"], network_added: ["localhost:8080"] });
    expect(sub.permissions).toMatchObject({ network_local: ["localhost:8080"] });
    const checks = Object.fromEntries((sub.checks as { id: string; level: string }[]).map((c) => [c.id, c.level]));
    expect(checks).toMatchObject({ binary_file: "warn", hosts: "warn", since_approved: "warn", asks: "warn" });
    expect(sub.version.readme).toContain("It does a thing");
    expect(sub.manifest.id).toBe(id);
  });

  it("approves, rejects with what the author reads, and refuses every illegal transition", async () => {
    const a = await author();
    const id = uniqueId();
    await pending(a, id, "1.0.0");
    await pending(a, id, "1.1.0");
    const approve = (v: string) => adminCall("POST", `/store/v1/admin/submissions/${id}/${v}/approve`);
    const reject = (v: string) => adminCall("POST", `/store/v1/admin/submissions/${id}/${v}/reject`, { reason: "broken", message: "It doesn't start." });
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/1.1.0/reject`, { reason: "Not A Word", message: "x" })).status).toBe(400);
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/1.1.0/reject`, { reason: "broken" })).status).toBe(400);
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
      ["remove rejected", () => adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.1.0/remove`, { block: false, reason: "x" })],
      ["restore active", () => adminCall("POST", `/store/v1/admin/plugins/${id}/restore`)]
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
      adminCall("POST", `/store/v1/admin/submissions/${id}/1.0.0/approve`),
      adminCall("POST", `/store/v1/admin/submissions/${id}/1.0.0/reject`, { reason: "broken", message: "No." }, ADMIN2)
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
    const remove = await adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.1.0/remove`, { block: true, reason: "It sends your files away." });
    expect(remove.status).toBe(200);
    expect((await json<{ version: Record<string, unknown> }>(remove)).version).toMatchObject({ status: "removed", blocked: true });
    expect((await json<{ version: string }>(await call(undefined, "GET", `/store/v1/plugins/${id}`))).version).toBe("1.0.0");
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}/versions/1.1.0/package`)).status).toBe(404);
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/versions/1.1.0/package`)).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/suspend`, { reason: "Under review." })).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/restore`)).status).toBe(200);
    const moved = await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username });
    expect(moved.status).toBe(200);
    expect(await json(moved)).toMatchObject({ owner: { id: await publisherId(b.userId), username: b.username, user_id: b.userId }, status: "active" });
    expect((await json<{ author: { username: string } }>(await call(undefined, "GET", `/store/v1/plugins/${id}`))).author.username).toBe(b.username);
    // The new owner publishes; the old one can't.
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.2.0", signer: a.signer }))).status).toBe(403);
    expect((await publish(b.userId, id, await makePackage({ id, version: "1.2.0", signer: b.signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: "no-such-member" })).status).toBe(400);
    expect((await audit(id)).map((e) => e.action)).toEqual([
      "plugin.transfer",
      "plugin.restore",
      "plugin.suspend",
      "version.remove",
      "version.approve",
      "version.approve"
    ]);
    const detail = await json<Record<string, any>>(await adminCall("GET", `/store/v1/admin/plugins/${id}`));
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
    const open = await json<{ reports: { id: number; reporter: { username: string }; plugin: { id: string } }[] }>(await adminCall("GET", "/store/v1/admin/reports"));
    expect(open.reports.find((r) => r.id === reportId)).toMatchObject({ reporter: { id: await publisherId(reader.userId), username: reader.username }, plugin: { id }, reason: "misleading" });
    expect((await adminCall("POST", `/store/v1/admin/reports/${reportId}/resolve`, { resolution: "closed" })).status).toBe(400);
    expect((await adminCall("POST", `/store/v1/admin/reports/${reportId}/resolve`, { resolution: "dismissed", note: "It is official." })).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/reports/${reportId}/resolve`, { resolution: "actioned" })).status).toBe(409);
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
      expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/feature`, { featured: i % 2 === 0 })).status).toBe(200);
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

  it("takes one open report per person, even sent at once", async () => {
    const a = await author();
    const reader = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const answers = await Promise.all(
      Array.from({ length: 5 }, () => call(reader.userId, "POST", `/store/v1/plugins/${id}/reports`, { reason: "broken", text: "It crashes." }))
    );
    const ids = new Set(await Promise.all(answers.map(async (r) => ((await r.json()) as { id: number }).id)));
    expect(ids.size).toBe(1);
    const open = await env.STORE_DB.prepare("SELECT COUNT(*) AS n FROM reports WHERE plugin_id = ? AND status = 'open'").bind(id).first<{ n: number }>();
    expect(open?.n).toBe(1);
  });

  it("lets a deploy key's request reach only publishing and its plugin's state", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const key = { authorization: "ZeronKey key=AAAA, ts=1, sig=AAAA" };
    const outside: [string, string, unknown?][] = [
      ["GET", "/store/v1/plugins"],
      ["GET", `/store/v1/plugins/${id}`],
      ["POST", `/store/v1/plugins/${id}/versions/1.0.0/yank`],
      ["POST", `/store/v1/plugins/${id}/installed`],
      ["POST", `/store/v1/plugins/${id}/reports`, { reason: "broken", text: "" }],
      ["POST", "/store/v1/updates", { zeron: "0.4.0", plugins: [] }],
      ["GET", "/store/v1/admin/queue"]
    ];
    for (const [method, path, body] of outside) {
      const res = await call(undefined, method, path, body, key);
      expect([res.status, ((await res.json()) as { error: string }).error], `${method} ${path}`).toEqual([403, "key_not_allowed"]);
    }
    // Publishing and its state are let through to the key's own checks.
    expect((await call(undefined, "POST", `/store/v1/plugins/${id}/versions`, new Uint8Array(4), key)).status).not.toBe(403);
    expect((await call(undefined, "GET", "/store/v1/me/plugins", undefined, key)).status).not.toBe(403);
  });

  it("pages the audit log both ways", async () => {
    const a = await author();
    const id = uniqueId();
    await pending(a, id, "1.0.0");
    for (let i = 0; i < 7; i++) expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/feature`, { featured: i % 2 === 0 })).status).toBe(200);
    type Page = { entries: { seq: number }[]; next_cursor?: string; prev_cursor?: string };
    const get = async (q: string) => (await (await adminCall("GET", `/store/v1/admin/audit?plugin=${id}&limit=3${q}`)).json()) as Page;
    const all = ((await (await adminCall("GET", `/store/v1/admin/audit?plugin=${id}`)).json()) as Page).entries.map((e) => e.seq);
    expect(all).toHaveLength(7);
    const forward: number[] = [];
    let page = await get("");
    forward.push(...page.entries.map((e) => e.seq));
    while (page.next_cursor) {
      page = await get(`&cursor=${page.next_cursor}`);
      forward.push(...page.entries.map((e) => e.seq));
    }
    expect(forward).toEqual(all);
    const back: number[] = [...page.entries.map((e) => e.seq)];
    while (page.prev_cursor) {
      page = await get(`&cursor=${page.prev_cursor}`);
      back.unshift(...page.entries.map((e) => e.seq));
    }
    expect(back).toEqual(all);
    expect((await adminCall("GET", "/store/v1/admin/audit?limit=0")).status).toBe(400);
  });
});

describe("store ids: claiming and release (amendment 30)", () => {
  it("lets an account claim at most 5 new ids in 7 days, and a refused publish claims nothing", async () => {
    const a = await author();
    // A publish refused at the insert (not later than nothing: can't happen) — use a bad
    // package instead: refused before any claim.
    expect((await publish(a.userId, uniqueId(), new TextEncoder().encode("{}"))).status).toBe(400);
    for (let i = 0; i < CLAIMS_PER_WEEK; i++) {
      const id = uniqueId("claim");
      expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    }
    const id = uniqueId("claim");
    const res = await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }));
    expect([res.status, ((await res.json()) as { error: string }).error]).toEqual([429, "rate_limited"]);
    expect(await env.STORE_DB.prepare("SELECT 1 FROM plugins WHERE id = ?").bind(id).first()).toBeNull();
    // New versions of ids already held are not claims.
    const held = (await env.STORE_DB.prepare("SELECT id FROM plugins WHERE owner_id = ? LIMIT 1").bind(a.userId).first<{ id: string }>())!.id;
    expect((await publish(a.userId, held, await makePackage({ id: held, version: "1.1.0", signer: a.signer }))).status).toBe(201);
  });

  it("releases an id never approved and quiet for 90 days, with its versions and objects, and keeps the audit", async () => {
    const a = await author();
    const quiet = uniqueId("quiet");
    const live = uniqueId("live");
    expect((await publish(a.userId, quiet, await makePackage({ id: quiet, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/submissions/${quiet}/1.0.0/reject`, { reason: "broken", message: "No." })).status).toBe(200);
    await listed(a, live, "1.0.0");
    const later = Date.now() + 91 * 86_400_000;
    // Not yet: 90 days haven't passed.
    expect(await releaseUnusedIds(env as never, Date.now())).toBe(0);
    const released = await releaseUnusedIds(env as never, later);
    expect(released).toBeGreaterThanOrEqual(1);
    expect(await env.STORE_DB.prepare("SELECT 1 FROM plugins WHERE id = ?").bind(quiet).first()).toBeNull();
    expect(await env.STORE_DB.prepare("SELECT 1 FROM versions WHERE plugin_id = ?").bind(quiet).first()).toBeNull();
    expect((await env.PLUGIN_STORE.list({ prefix: `pkg/${quiet}/` })).objects).toEqual([]);
    expect((await env.PLUGIN_STORE.list({ prefix: `files/${quiet}/` })).objects).toEqual([]);
    expect(await env.STORE_DB.prepare("SELECT 1 FROM plugins WHERE id = ?").bind(live).first()).not.toBeNull();
    const entries = (await (await adminCall("GET", `/store/v1/admin/audit?plugin=${quiet}`)).json()) as { entries: { action: string; detail: unknown }[] };
    expect(entries.entries[0]).toMatchObject({ action: "plugin.release", detail: { versions: [{ version: "1.0.0", status: "rejected" }] } });
    // Anyone may claim it now.
    const b = await author();
    expect((await publish(b.userId, quiet, await makePackage({ id: quiet, version: "1.0.0", signer: b.signer }))).status).toBe(201);
  });

  it("lets an admin release an unused id at once, or reserve it for someone, but never one in use", async () => {
    const a = await author();
    const b = await author();
    const unused = uniqueId("unused");
    const used = uniqueId("used");
    expect((await publish(a.userId, unused, await makePackage({ id: unused, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await call(a.userId, "POST", `/store/v1/plugins/${unused}/versions/1.0.0/withdraw`)).status).toBe(200);
    await listed(a, used, "1.0.0");
    const inUse = await adminCall("POST", "/store/v1/admin/reservations", { id: used, to_username: b.username });
    expect([inUse.status, ((await inUse.json()) as { error: string }).error]).toEqual([409, "invalid_state"]);
    const given = await adminCall("POST", "/store/v1/admin/reservations", { id: unused, to_username: b.username });
    expect(given.status).toBe(200);
    expect(await given.json()).toMatchObject({ released: true });
    expect((await publish(a.userId, unused, await makePackage({ id: unused, version: "1.0.1", signer: a.signer }))).status).toBe(403);
    expect((await publish(b.userId, unused, await makePackage({ id: unused, version: "1.0.0", signer: b.signer }))).status).toBe(201);
  });

  it("never sweeps away an id reserved for an account", async () => {
    const a = await author();
    const id = uniqueId("kept");
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.0/withdraw`)).status).toBe(200);
    // Reserved for its holder, as an admin may: the sweep leaves it.
    await env.STORE_DB.prepare("INSERT INTO reservations (id, user_id, note, created_by, created_at) VALUES (?, ?, 'kept', ?, ?)").bind(id, a.userId, ADMIN, Date.now()).run();
    await releaseUnusedIds(env as never, Date.now() + 91 * 86_400_000);
    expect(await env.STORE_DB.prepare("SELECT 1 FROM plugins WHERE id = ?").bind(id).first()).not.toBeNull();
  });

  it("finishes a release interrupted after its rows went: the next sweep deletes what it owed", async () => {
    const a = await author();
    const id = uniqueId("half");
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.0/withdraw`)).status).toBe(200);
    // The rows' batch alone, as if the Worker stopped right after it.
    expect(await releaseRows(env.STORE_DB, id, Date.now() + 91 * 86_400_000, "system", true)).toBe(true);
    expect(await env.STORE_DB.prepare("SELECT 1 FROM plugins WHERE id = ?").bind(id).first()).toBeNull();
    expect((await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` })).objects).toHaveLength(1);
    const owed = await env.STORE_DB.prepare("SELECT COUNT(*) AS n FROM object_purges WHERE key LIKE ?").bind(`%/${id}/%`).first<{ n: number }>();
    expect(owed?.n).toBe(2);
    await releaseUnusedIds(env as never, Date.now());
    expect((await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` })).objects).toEqual([]);
    expect((await env.PLUGIN_STORE.list({ prefix: `files/${id}/` })).objects).toEqual([]);
    expect((await env.STORE_DB.prepare("SELECT COUNT(*) AS n FROM object_purges WHERE key LIKE ?").bind(`%/${id}/%`).first<{ n: number }>())?.n).toBe(0);
    expect(await env.STORE_DB.prepare("SELECT 1 FROM key_revocations WHERE plugin_id = ?").bind(id).first()).toBeNull();
  });
});

describe("store publisher ids (amendment 33)", () => {
  it("are stable across a username change, differ per account, and ride on every shape that names an author", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const pid = await publisherId(a.userId);
    expect(pid).toMatch(/^pub_[0-9a-f]{32}$/);
    expect(pid).not.toBe(await publisherId(b.userId));
    expect(pid).not.toContain(a.userId);
    const cardAuthor = async () => ((await (await call(a.userId, "GET", `/store/v1/plugins/${id}`)).json()) as { author: { id: string; username: string } }).author;
    expect(await cardAuthor()).toMatchObject({ id: pid, username: a.username });
    // An operator moves the username: the id stays.
    const renamed = `zz${a.username.slice(2)}`;
    const set = await SELF.fetch("https://edge.test/admin/usernames/set", {
      method: "POST",
      headers: { authorization: "Bearer test-admin-token", "content-type": "application/json" },
      body: JSON.stringify({ userId: a.userId, username: renamed })
    });
    expect(set.status).toBe(200);
    expect(await cardAuthor()).toEqual({ id: pid, username: renamed, badges: expect.any(Array) });
    const list = (await (await call(a.userId, "GET", "/store/v1/me/plugins")).json()) as { plugins: { author: { id: string } }[] };
    expect(list.plugins[0]!.author.id).toBe(pid);
    const hash = (await env.STORE_DB.prepare("SELECT content_hash FROM versions WHERE plugin_id = ?").bind(id).first<{ content_hash: string }>())!.content_hash;
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.1.0", signer: a.signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/1.1.0/approve`)).status).toBe(200);
    const offers = (await (await call(undefined, "POST", "/store/v1/updates", { zeron: "0.4.0", plugins: [{ id, version: "1.0.0", hash }] })).json()) as { updates: Record<string, unknown>[] };
    expect(offers.updates[0]).toMatchObject({ publisher: renamed, publisher_id: pid });
  });
});
