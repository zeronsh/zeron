/**
 * Publishing is immediate; review verifies (amendment 45, contract
 * docs/plans/2026-11-19-store-shapes-44-45.md): the superseding rule and
 * its edges, the queue by line, the one-way mark, names too like another's,
 * the catalog's mark, filters and totals, and the take-down feed.
 */
import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { SIMILAR_INSTALLS } from "../../src/store/publish";
import { TAKEDOWN_DAYS } from "../../src/store/catalog";
import { pruneTakedowns } from "../../src/store/release";
import { author, call, listed, makePackage, publish, published, randomLetters, uniqueId, adminCall } from "./store-helpers";

const json = async <T = Record<string, any>>(res: Response) => (await res.json()) as T;
const row = (id: string, version: string) =>
  env.STORE_DB.prepare("SELECT status, yanked_by, verification, superseded_by FROM versions WHERE plugin_id = ? AND version = ?")
    .bind(id, version)
    .first<{ status: string; yanked_by: string | null; verification: string; superseded_by: string | null }>();
const states = async (id: string) =>
  (await env.STORE_DB.prepare("SELECT version, status, yanked_by, superseded_by FROM versions WHERE plugin_id = ? ORDER BY published_at").bind(id).all<Record<string, unknown>>()).results.map(
    (r) => [r.version, r.status, r.yanked_by, r.superseded_by]
  );
const verify = (id: string, version: string) => adminCall("POST", `/store/v1/admin/versions/${id}/${version}/verify`);
const takeDown = (id: string, version: string) => adminCall("POST", `/store/v1/admin/plugins/${id}/versions/${version}/take-down`, { reason: "It sends your files away." });
const restore = (id: string, version: string) => adminCall("POST", `/store/v1/admin/plugins/${id}/versions/${version}/restore`, { reason: "A mistake." });
const queueOf = async (id: string) =>
  (await json<{ items: Record<string, any>[] }>(await adminCall("GET", "/store/v1/admin/queue?limit=50"))).items.filter((i) => i.plugin.id === id);

describe("an unverified version stops taking new installs when a newer one is published", () => {
  it("a release supersedes older unverified releases and pre-releases; verified ones are untouched", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.0.0");
    await listed(a, id, "1.1.0");
    await published(a, id, "1.2.0-beta.1");
    const answer = await published(a, id, "1.2.0");
    // The publish answer says what it superseded.
    expect(answer).toMatchObject({ status: "listed", verified: false, superseded: ["1.2.0-beta.1"] });
    expect(await states(id)).toEqual([
      ["1.0.0", "yanked", "superseded", "1.1.0"],
      ["1.1.0", "listed", null, null],
      ["1.2.0-beta.1", "yanked", "superseded", "1.2.0"],
      ["1.2.0", "listed", null, null]
    ]);
    // A superseded version: never installable anew, nor unyanked by its author.
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}/versions/1.0.0`)).status).toBe(200);
    const unyank = await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.0.0/unyank`);
    expect([unyank.status, (await json(unyank)).error]).toEqual([409, "invalid_state"]);
    // The newest stays installable, verified or not; the verified one too.
    const card = await json(await call(undefined, "GET", `/store/v1/plugins/${id}`));
    expect(card).toMatchObject({ version: "1.2.0", verified: false, verified_version: "1.1.0" });
  });

  it("a pre-release supersedes older pre-releases only, so a release line and a pre-release line each keep one undecided row", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.5.0");
    await published(a, id, "2.0.0-beta.1");
    expect(await published(a, id, "2.0.0-beta.2")).toMatchObject({ superseded: ["2.0.0-beta.1"] });
    // 1.5.0 is untouched: stable users keep their release.
    expect(await row(id, "1.5.0")).toMatchObject({ status: "listed", yanked_by: null });
    // The queue: the newest undecided release and the newer pre-release, each once.
    const rows = await queueOf(id);
    expect(rows.map((r) => [r.version, r.earlier_undecided])).toEqual([
      ["1.5.0", 0],
      ["2.0.0-beta.2", 1]
    ]);
    // A release later than both supersedes both: one row.
    expect(await published(a, id, "2.0.0")).toMatchObject({ superseded: ["2.0.0-beta.2", "1.5.0"] });
    expect((await queueOf(id)).map((r) => [r.version, r.earlier_undecided])).toEqual([["2.0.0", 3]]);
    // Nothing that left the queue unread is installable.
    for (const v of ["1.5.0", "2.0.0-beta.1", "2.0.0-beta.2"]) expect(await row(id, v), v).toMatchObject({ status: "yanked", yanked_by: "superseded" });
  });

  it("verifying a superseded version lists it again (the author's own yank stays theirs)", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "2.0.5");
    await published(a, id, "2.0.6");
    expect(await row(id, "2.0.5")).toMatchObject({ status: "yanked", yanked_by: "superseded" });
    // A reviewer reading 2.0.5 while 2.0.6 landed: their verify makes it installable.
    expect((await verify(id, "2.0.5")).status).toBe(200);
    expect(await row(id, "2.0.5")).toMatchObject({ status: "listed", yanked_by: null, verification: "verified" });
    expect(await json(await call(undefined, "GET", `/store/v1/plugins/${id}`))).toMatchObject({ version: "2.0.6", verified_version: "2.0.5" });
    // An author's yank isn't undone by a verify.
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/2.0.6/yank`)).status).toBe(200);
    expect((await verify(id, "2.0.6")).status).toBe(200);
    expect(await row(id, "2.0.6")).toMatchObject({ status: "yanked", yanked_by: "author", verification: "verified" });
  });

  it("brings nothing back when the newest is yanked by its author: the plugin leaves the lists", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.0.0");
    await published(a, id, "1.1.0");
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/yank`)).status).toBe(200);
    expect(await row(id, "1.0.0")).toMatchObject({ status: "yanked", yanked_by: "superseded" });
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(404);
    // Its own yank undone: back on the lists, with what it had.
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/unyank`)).status).toBe(200);
    expect(await json(await call(undefined, "GET", `/store/v1/plugins/${id}`))).toMatchObject({ version: "1.1.0" });
    expect(await row(id, "1.0.0")).toMatchObject({ status: "yanked", yanked_by: "superseded" });
  });

  it("brings nothing back when the newest is taken down or removed: the author publishes again", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.0.0");
    await published(a, id, "1.1.0");
    expect((await takeDown(id, "1.1.0")).status).toBe(200);
    expect(await row(id, "1.0.0")).toMatchObject({ status: "yanked", yanked_by: "superseded" });
    // Nothing installable: out of the lists, the counts and the totals.
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(404);
    await published(a, id, "1.2.0");
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.2.0/remove`, { reason: "A copyright claim." })).status).toBe(200);
    expect(await row(id, "1.0.0")).toMatchObject({ status: "yanked", yanked_by: "superseded" });
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(404);
  });

  it("a restore comes back unverified, and straight to superseded when anything newer on its line is listed", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    expect((await takeDown(id, "1.0.0")).status).toBe(200);
    await published(a, id, "1.1.0");
    expect((await restore(id, "1.0.0")).status).toBe(200);
    expect(await row(id, "1.0.0")).toMatchObject({ status: "yanked", yanked_by: "superseded", superseded_by: "1.1.0", verification: "unverified" });
    // A pre-release restored under a newer release: superseded too.
    const other = uniqueId();
    await listed(a, other, "2.0.0-beta.1");
    expect((await takeDown(other, "2.0.0-beta.1")).status).toBe(200);
    await listed(a, other, "2.0.0");
    expect((await restore(other, "2.0.0-beta.1")).status).toBe(200);
    expect(await row(other, "2.0.0-beta.1")).toMatchObject({ status: "yanked", superseded_by: "2.0.0" });
    // A release restored under only a newer pre-release: listed (its line is releases).
    const third = uniqueId();
    await listed(a, third, "3.0.0");
    expect((await takeDown(third, "3.0.0")).status).toBe(200);
    await published(a, third, "3.1.0-rc.1");
    expect((await restore(third, "3.0.0")).status).toBe(200);
    expect(await row(third, "3.0.0")).toMatchObject({ status: "listed", yanked_by: null, verification: "unverified" });
  });

  it("is one write with the publish: a refused publish supersedes nothing, and a retry of the same bytes answers that version as it is now", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.0.0");
    const pkg = await makePackage({ id, version: "1.1.0", signer: a.signer });
    // A publish refused (not later) leaves everything as it was.
    const older = await publish(a.userId, id, await makePackage({ id, version: "0.9.0", signer: a.signer }));
    expect([older.status, (await json(older)).error]).toEqual([409, "not_newer"]);
    expect(await row(id, "1.0.0")).toMatchObject({ status: "listed" });
    expect((await publish(a.userId, id, pkg)).status).toBe(201);
    await published(a, id, "1.2.0");
    // 1.1.0 is superseded now; its bytes again answer just that, and nothing moves.
    const before = await states(id);
    const again = await publish(a.userId, id, pkg);
    expect([again.status, await json(again)]).toEqual([201, expect.objectContaining({ version: "1.1.0", status: "yanked", verified: false, superseded: ["1.0.0"] })]);
    expect(await states(id)).toEqual(before);
  });

  it("takes the superseded versions out of the cache at once", async () => {
    const a = await author();
    const id = uniqueId();
    await published(a, id, "1.0.0");
    // Cached as listed.
    expect((await json(await call(undefined, "GET", `/store/v1/plugins/${id}/versions/1.0.0`))).status).toBe("listed");
    await published(a, id, "1.1.0");
    expect((await json(await call(undefined, "GET", `/store/v1/plugins/${id}/versions/1.0.0`))).status).toBe("yanked");
  });
});

describe("the mark is the store's alone", () => {
  it("stays once given; only a take-down ends it", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const decline = await adminCall("POST", `/store/v1/admin/versions/${id}/1.0.0/decline`, { reason: "broken", message: "x" });
    expect([decline.status, (await json(decline)).error]).toEqual([409, "invalid_state"]);
    expect((await takeDown(id, "1.0.0")).status).toBe(200);
    expect(await row(id, "1.0.0")).toMatchObject({ status: "taken_down", verification: "unverified" });
    const v = await json(await adminCall("GET", `/store/v1/admin/versions/${id}/1.0.0`));
    expect(v.version).toMatchObject({ verified: false, review: { decided: false, decision: null } });
  });
});

describe("names too like another's", () => {
  it("refuses a claim one edit or one confusable letter from another account's id with 100 installs, unless staff reserved it", async () => {
    const owner = await author();
    const other = await author();
    const id = `tmr-${randomLetters(8)}`;
    await listed(owner, id, "1.0.0");
    await env.STORE_DB.prepare("UPDATE plugins SET installs = ? WHERE id = ?").bind(SIMILAR_INSTALLS, id).run();
    const near = [`${id}x`, id.replace("tmr", "tnr"), id.slice(0, -1), id.replace("tmr", "tm1")];
    for (const [i, claim] of near.entries()) {
      const res = await publish(other.userId, claim, await makePackage({ id: claim, version: "1.0.0", signer: other.signer }));
      expect([res.status, (await json(res)).error, i], claim).toEqual([403, "id_too_similar", i]);
    }
    // Two edits away: anyone's.
    const far = `${id}xy`;
    expect((await publish(other.userId, far, await makePackage({ id: far, version: "1.0.0", signer: other.signer }))).status).toBe(201);
    // Its owner may claim one near their own; a quiet id (fewer installs) protects nothing.
    expect((await publish(owner.userId, `${id}z`, await makePackage({ id: `${id}z`, version: "1.0.0", signer: owner.signer }))).status).toBe(201);
    // Reserved for the claimer by staff: allowed.
    const given = `${id}q`;
    expect((await adminCall("POST", "/store/v1/admin/reservations", { id: given, to_username: other.username })).status).toBe(200);
    expect((await publish(other.userId, given, await makePackage({ id: given, version: "1.0.0", signer: other.signer }))).status).toBe(201);
  });
});

describe("the catalog's mark, filters and totals", () => {
  it("narrows to verified plugins, features only verified ones, and lists one author's", async () => {
    const a = await author();
    const tag = `t${randomLetters(8)}`;
    const vetted = uniqueId();
    const fresh = uniqueId();
    const hijacked = uniqueId();
    await listed(a, vetted, "1.0.0", { manifest: { keywords: [tag] } });
    await published(a, fresh, "1.0.0", { manifest: { keywords: [tag] } });
    await listed(a, hijacked, "1.0.0", { manifest: { keywords: [tag] } });
    await published(a, hijacked, "1.1.0", { manifest: { keywords: [tag] } });
    const ids = async (q: string) => (await json<{ items: { id: string }[] }>(await call(undefined, "GET", `/store/v1/plugins?keyword=${tag}&${q}`))).items.map((c) => c.id).sort();
    expect(await ids("")).toEqual([vetted, fresh, hijacked].sort());
    // Only a plugin whose newest version is verified.
    expect(await ids("verified=1")).toEqual([vetted]);
    // Featured needs that too, when made and when read.
    const refused = await adminCall("POST", `/store/v1/admin/plugins/${hijacked}/feature`, { featured: true });
    expect([refused.status, (await json(refused)).error]).toEqual([409, "not_verified"]);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${vetted}/feature`, { featured: true })).status).toBe(200);
    expect(await ids("featured=true")).toEqual([vetted]);
    // A newer unverified version takes it off the shelf: on the featured
    // first page at once (purged here; filtered pages within the list TTL).
    const shelf = async () => (await json<{ items: { id: string }[] }>(await call(undefined, "GET", "/store/v1/plugins?featured=true"))).items.map((c) => c.id);
    expect(await shelf()).toContain(vetted);
    await published(a, vetted, "1.1.0", { manifest: { keywords: [tag] } });
    expect(await shelf()).not.toContain(vetted);
    // One publisher's plugins, by username (folded), and nobody's for a name no one holds.
    const mine = await json<{ items: unknown[]; total: number }>(await call(undefined, "GET", `/store/v1/plugins?author=${a.username.toUpperCase()}&keyword=${tag}`));
    expect(mine.total).toBe(3);
    expect(await json(await call(undefined, "GET", "/store/v1/plugins?author=nobody-holds-this"))).toEqual({ items: [], total: 0 });
    expect((await call(undefined, "GET", "/store/v1/plugins?author=Not%20A%20Name")).status).toBe(400);
  });

  it("counts each category's listed plugins, and the store's totals, exactly as the lists show them", async () => {
    const a = await author();
    const count = async () => {
      const cats = (await json<{ categories: { id: string; count: number }[] }>(await call(undefined, "GET", "/store/v1/categories"))).categories;
      return cats.find((c) => c.id === "productivity")!.count;
    };
    const stats = async () => json<{ plugins: number; verified: number }>(await call(undefined, "GET", "/store/v1/stats"));
    const listedCount = async () => (await json<{ total: number }>(await call(undefined, "GET", "/store/v1/plugins?category=productivity"))).total;
    const before = { category: await count(), stats: await stats() };
    expect(before.category).toBe(await listedCount());
    const id = uniqueId();
    await published(a, id, "1.0.0");
    expect(await count()).toBe(before.category + 1);
    expect(await stats()).toEqual({ plugins: before.stats.plugins + 1, verified: before.stats.verified });
    expect((await verify(id, "1.0.0")).status).toBe(200);
    expect(await stats()).toEqual({ plugins: before.stats.plugins + 1, verified: before.stats.verified + 1 });
    expect((await takeDown(id, "1.0.0")).status).toBe(200);
    expect(await count()).toBe(before.category);
    expect(await stats()).toEqual(before.stats);
    expect(await count()).toBe(await listedCount());
  });

  it("pages every listed version, and answers one by name", async () => {
    const a = await author();
    const id = uniqueId();
    for (const v of ["1.0.0", "1.1.0", "1.2.0", "1.3.0", "1.4.0", "1.5.0", "1.6.0"]) await listed(a, id, v);
    const detail = await json(await call(undefined, "GET", `/store/v1/plugins/${id}`));
    expect(detail.versions.map((v: { version: string }) => v.version)).toEqual(["1.6.0", "1.5.0", "1.4.0", "1.3.0", "1.2.0"]);
    expect(typeof detail.versions_next_cursor).toBe("string");
    const page = (cursor?: string) => call(undefined, "GET", `/store/v1/plugins/${id}/versions?limit=3${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`);
    const seen: string[] = [];
    let p = await json(await page());
    expect(p.total).toBe(7);
    seen.push(...p.versions.map((v: { version: string }) => v.version));
    while (p.next_cursor) {
      p = await json(await page(p.next_cursor));
      seen.push(...p.versions.map((v: { version: string }) => v.version));
    }
    expect(seen).toEqual(["1.6.0", "1.5.0", "1.4.0", "1.3.0", "1.2.0", "1.1.0", "1.0.0"]);
    expect(await json(await page(p.prev_cursor))).toMatchObject({ versions: [{ version: "1.3.0" }, { version: "1.2.0" }, { version: "1.1.0" }] });
    expect((await page("not-a-cursor")).status).toBe(400);
    expect(await json(await call(undefined, "GET", `/store/v1/plugins/${id}/versions/1.2.0`))).toMatchObject({ version: "1.2.0", status: "listed", verified: true, via: "device" });
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}/versions/9.9.9`)).status).toBe(404);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}/versions/not-a-version`)).status).toBe(404);
  });
});

describe("the take-down feed", () => {
  it("numbers each take-down and restore in commit order, from any seq a device holds, and resets one too old", async () => {
    const a = await author();
    const feed = async (after?: number) => json<{ events: Record<string, unknown>[]; head: number; reset?: boolean }>(await call(undefined, "GET", `/store/v1/takedowns${after === undefined ? "" : `?after=${after}`}`));
    // A device starting afresh: reset, then go on from the head.
    const start = await feed();
    expect(start).toMatchObject({ events: [], reset: true });
    const head = start.head;
    expect(await feed(head)).toEqual({ events: [], head });
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    expect((await takeDown(id, "1.0.0")).status).toBe(200);
    expect((await restore(id, "1.0.0")).status).toBe(200);
    // (The answer a device up to date held was purged: it sees both at once.)
    const after = await feed(head);
    expect(after.events).toEqual([
      { seq: head + 1, kind: "take_down", id, version: "1.0.0", reason: "It sends your files away." },
      { seq: head + 2, kind: "restore", id, version: "1.0.0", reason: "A mistake." }
    ]);
    expect(after.head).toBe(head + 2);
    expect(await feed(head + 1)).toMatchObject({ events: [{ seq: head + 2, kind: "restore" }] });
    // A seq the feed never gave, or a malformed one.
    expect(await feed(head + 50)).toMatchObject({ reset: true, head: head + 2 });
    expect((await call(undefined, "GET", "/store/v1/takedowns?after=-1")).status).toBe(400);
    // Pruned past 30 days: a device behind them starts afresh. (Read past
    // the cache: this test pruned by hand, where the sweep's purge would.)
    // (The prune takes the oldest first: everything up to head + 1 here.)
    await env.STORE_DB.prepare("DELETE FROM takedown_events WHERE seq <= ?").bind(head + 1).run();
    expect(TAKEDOWN_DAYS).toBe(30);
    expect(await json(await call(a.userId, "GET", `/store/v1/takedowns?after=${head}`))).toMatchObject({ reset: true, head: head + 2 });

    // The sweep prunes past 30 days, oldest first, and always keeps the newest.
    await pruneTakedowns(env.STORE_DB, Date.now() + (TAKEDOWN_DAYS + 1) * 86_400_000);
    const kept = await env.STORE_DB.prepare("SELECT seq FROM takedown_events").all<{ seq: number }>();
    expect(kept.results.map((r) => r.seq)).toEqual([head + 2]);
    expect(await json(await call(a.userId, "GET", `/store/v1/takedowns?after=${head + 2}`))).toEqual({ events: [], head: head + 2 });
  });
});
