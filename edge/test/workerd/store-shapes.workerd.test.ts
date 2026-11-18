/**
 * Every store answer's times are ms since the epoch (contract amendment 35),
 * account ages included: one walk over an answer of every shape the store
 * gives, public, owner and admin, failing on any time that isn't a number.
 */
import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { adminCall, author, call, listed, makePackage, publish, uniqueId } from "./store-helpers";

/** A time's key: `at`, or anything `…_at`. */
const isTimeKey = (key: string): boolean => key === "at" || key.endsWith("_at");
const ISO = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/;

/** Every time in `value` that isn't a number (or null: not happened), and
 * every string anywhere that reads as an ISO time, by path; and how many
 * times it saw. */
const walk = (value: unknown, path: string, bad: string[], seen: { n: number }): void => {
  if (Array.isArray(value)) value.forEach((v, i) => walk(v, `${path}[${i}]`, bad, seen));
  else if (value !== null && typeof value === "object") {
    for (const [k, v] of Object.entries(value)) {
      if (isTimeKey(k)) {
        seen.n++;
        if (!(v === null || (typeof v === "number" && Number.isSafeInteger(v) && v > 0))) bad.push(`${path}.${k} = ${JSON.stringify(v)}`);
      }
      walk(v, `${path}.${k}`, bad, seen);
    }
  } else if (typeof value === "string" && ISO.test(value)) bad.push(`${path} = ${JSON.stringify(value)}`);
};

describe("what a device reads (amendment 43)", () => {
  it("names every package's bytes, and never answers with a redirect", async () => {
    const a = await author();
    const other = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    await listed(a, id, "1.1.0", { manifest: { permissions: ["ui.add", "links.open"] } });
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.2.0", signer: a.signer }))).status).toBe(201);
    const served = async (version: string) =>
      (await (await call(a.userId, "GET", `/store/v1/plugins/${id}/versions/${version}/package`)).arrayBuffer()).byteLength;
    const sizes = Object.fromEntries(await Promise.all(["1.0.0", "1.1.0", "1.2.0"].map(async (v) => [v, await served(v)] as const)));
    const detail = (await (await call(undefined, "GET", `/store/v1/plugins/${id}`)).json()) as any;
    const versions = ((await (await call(undefined, "GET", `/store/v1/plugins/${id}/versions`)).json()) as any).versions;
    const mine = ((await (await call(a.userId, "GET", "/store/v1/me/plugins")).json()) as any).plugins.find((p: { id: string }) => p.id === id).versions;
    const hash = (await env.STORE_DB.prepare("SELECT content_hash FROM versions WHERE plugin_id = ? AND version = '1.0.0'").bind(id).first<string>("content_hash"))!;
    const offers = ((await (await call(a.userId, "POST", "/store/v1/updates", { zeron: "0.4.0", plugins: [{ id, version: "1.0.0", hash }] })).json()) as any).updates;
    // Every record a device installs or updates by: its package's exact size.
    expect(detail.package.bytes).toBe(sizes[detail.version]);
    for (const v of [...versions, ...mine]) expect(v.bytes, v.version).toBe(sizes[v.version]);
    expect(offers.length).toBeGreaterThan(0);
    for (const o of offers) expect(o.bytes, o.version).toBe(sizes[o.version]);
    // No route a device calls answers with a redirect, refusals included.
    const asks: [string | undefined, string, string, unknown?][] = [
      [undefined, "GET", "/store/v1/plugins"],
      [undefined, "GET", `/store/v1/plugins/${id}`],
      [undefined, "GET", `/store/v1/plugins/${id}/versions`],
      [undefined, "GET", `/store/v1/plugins/${id}/versions/1.1.0/package`],
      [undefined, "HEAD", `/store/v1/plugins/${id}/versions/1.1.0/package`],
      [undefined, "GET", `/store/v1/plugins/${id}/files/1.1.0/icon.svg`],
      [undefined, "GET", `/store/v1/plugins/${id}/files/1.1.0/init.luau`],
      [other.userId, "GET", `/store/v1/plugins/${id}/versions/1.2.0/package`],
      [undefined, "GET", `/store/v1/plugins/${id}/versions/9.9.9/package`],
      [undefined, "GET", `/store/v1/plugins/${id}/versions/1.1.0/package/`],
      [undefined, "GET", `/store/v1/plugins/${id}//versions`],
      [undefined, "GET", "/store/v1/plugins/no-such-plugin"],
      [undefined, "GET", "/store/v1/categories"],
      [a.userId, "GET", "/store/v1/me/plugins"],
      [a.userId, "POST", "/store/v1/updates", { zeron: "0.4.0", plugins: [] }],
      [a.userId, "POST", `/store/v1/plugins/${id}/installed`]
    ];
    for (const [who, method, path, body] of asks) {
      const res = await call(who, method, path, body);
      expect([res.status >= 300 && res.status < 400, res.headers.get("location")], `${method} ${path}`).toEqual([false, null]);
      await res.arrayBuffer();
    }
  });
});

describe("store answers", () => {
  it("give every time as ms since the epoch", async () => {
    const a = await author();
    const reader = await author();
    const newOwner = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const answers: [string, Response][] = [];
    const take = async (what: string, res: Response | Promise<Response>) => {
      const r = await res;
      expect(r.status, `${what}: ${await r.clone().text()}`).toBeLessThan(300);
      answers.push([what, r]);
    };

    // Owner: a publish, the owner's list, an author action.
    await take("publish", publish(a.userId, id, await makePackage({ id, version: "1.1.0", signer: a.signer, manifest: { permissions: ["ui.add", "links.open"] } })));
    await take("publish again", publish(a.userId, id, await makePackage({ id, version: "1.2.0", signer: a.signer })));
    await take("withdraw", call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.2.0/withdraw`));
    await take("me/plugins", call(a.userId, "GET", "/store/v1/me/plugins"));
    // Admin.
    await take("queue", adminCall("GET", "/store/v1/admin/queue?status=all"));
    await take("submission", adminCall("GET", `/store/v1/admin/submissions/${id}/1.1.0`));
    await take("approve", adminCall("POST", `/store/v1/admin/submissions/${id}/1.1.0/approve`));
    await take("yank", call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/yank`));
    await take("unyank", call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/unyank`));
    // Public.
    await take("list", call(undefined, "GET", "/store/v1/plugins?sort=new"));
    await take("search", call(undefined, "GET", `/store/v1/plugins?q=${encodeURIComponent(id)}`));
    await take("detail", call(undefined, "GET", `/store/v1/plugins/${id}`));
    await take("versions", call(undefined, "GET", `/store/v1/plugins/${id}/versions`));
    await take("categories", call(undefined, "GET", "/store/v1/categories"));
    const hash = (await env.STORE_DB.prepare("SELECT content_hash FROM versions WHERE plugin_id = ? AND version = '1.0.0'").bind(id).first<string>("content_hash"))!;
    await take("updates", call(reader.userId, "POST", "/store/v1/updates", { zeron: "0.4.0", plugins: [{ id, version: "1.0.0", hash }] }));
    // A signed-in person.
    await take("installed", call(reader.userId, "POST", `/store/v1/plugins/${id}/installed`));
    const report = await call(reader.userId, "POST", `/store/v1/plugins/${id}/reports`, { reason: "broken", text: "It crashes." });
    const reportId = ((await report.clone().json()) as { id: number }).id;
    await take("report", report);
    // Admin again: the views with people and reports, and each change's answer.
    await take("reports", adminCall("GET", "/store/v1/admin/reports?status=all"));
    await take("admin plugin", adminCall("GET", `/store/v1/admin/plugins/${id}`));
    await take("resolve", adminCall("POST", `/store/v1/admin/reports/${reportId}/resolve`, { resolution: "actioned", note: "Fixed." }));
    await take("feature", adminCall("POST", `/store/v1/admin/plugins/${id}/feature`, { featured: true }));
    await take("suspend", adminCall("POST", `/store/v1/admin/plugins/${id}/suspend`, { reason: "Looking." }));
    await take("restore", adminCall("POST", `/store/v1/admin/plugins/${id}/restore`));
    await take("remove", adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.0.0/remove`, { block: true, reason: "It sends your files away." }));
    await take("transfer", adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: newOwner.username }));
    await take("audit", adminCall("GET", `/store/v1/admin/audit?plugin=${id}`));
    const reserved = uniqueId();
    await take("reserve", adminCall("POST", "/store/v1/admin/reservations", { id: reserved, to_username: reader.username, note: "Theirs." }));
    await take("reservations", adminCall("GET", "/store/v1/admin/reservations"));
    await take("unreserve", adminCall("DELETE", `/store/v1/admin/reservations/${reserved}`));

    const bad: string[] = [];
    const seen = { n: 0 };
    const bodies: [string, any][] = [];
    for (const [what, res] of answers) bodies.push([what, await res.json()]);
    for (const [what, body] of bodies) walk(body, what, bad, seen);
    // A listed plugin's card says when it was first listed (amendment 39).
    const detail = bodies.find(([what]) => what === "detail")![1];
    const listedAt = await env.STORE_DB.prepare("SELECT listed_at FROM plugins WHERE id = ?").bind(id).first<number>("listed_at");
    expect(detail.listed_at).toBe(listedAt);
    for (const what of ["list", "search"]) {
      const card = bodies.find(([w]) => w === what)![1].items.find((c: { id: string }) => c.id === id);
      expect(card, what).toMatchObject({ listed_at: listedAt, updated_at: expect.any(Number) });
    }
    expect(bad).toEqual([]);
    // (The walk saw them: account ages, decisions, submissions, reports, the log.)
    expect(seen.n).toBeGreaterThan(60);
  });
});
