import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { packageBytesKey, packageStateKey } from "../../src/store/serve";
import { ADMIN, PNG, author, call, listed, makePackage, publish, randomLetters, uniqueId, adminCall } from "./store-helpers";

type Card = { id: string; name: string; version: string; installs: number; author: { username: string }; icon: string | null; featured: boolean };
const json = async <T = Record<string, unknown>>(res: Response) => (await res.json()) as T;
const list = async (query: string, bearer?: string) => {
  const res = await call(bearer, "GET", `/store/v1/plugins?${query}`);
  expect(res.status, await res.clone().text()).toBe(200);
  return json<{ items: Card[]; next_cursor?: string }>(res);
};

describe("store catalog", () => {
  it("lists approved plugins as cards, with the author's live username", async () => {
    const a = await author();
    const id = uniqueId();
    const tag = `t${randomLetters(8)}`;
    await listed(a, id, "1.0.0", { manifest: { keywords: [tag] } });
    const page = await list(`keyword=${tag}`);
    expect(page.items).toHaveLength(1);
    expect(page.items[0]).toMatchObject({
      id,
      version: "1.0.0",
      summary: `The ${id} plugin.`,
      icon: `/store/v1/plugins/${id}/files/1.0.0/icon.svg`,
      author: { username: a.username, badges: [] },
      categories: ["productivity"],
      keywords: [tag],
      installs: 0,
      featured: false
    });
    const detail = await json<Record<string, unknown>>(await call(undefined, "GET", `/store/v1/plugins/${id}`));
    expect(detail).toMatchObject({
      id,
      readme: expect.stringContaining("It does a thing"),
      screenshots: [`/store/v1/plugins/${id}/files/1.0.0/shots/one.png`],
      permissions: { asks: ["ui.add"], commands: [], network_hosts: [], tools: [] },
      engines: null,
      versions: [{ version: "1.0.0", status: "approved", yanked: false, notes: "- What changed in 1.0.0.", key: a.signer.key }],
      package: { url: `/store/v1/plugins/${id}/versions/1.0.0/package`, key: a.signer.key }
    });
    const categories = await json<{ categories: { id: string }[] }>(await call(undefined, "GET", "/store/v1/categories"));
    expect(categories.categories.map((c) => c.id)).toEqual(["agents", "appearance", "chat", "developer-tools", "git", "integrations", "productivity", "utilities"]);
  });

  it("shows nothing pending, rejected, unknown or suspended to the public", async () => {
    const a = await author();
    const id = uniqueId();
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(404);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}/versions`)).status).toBe(404);
    expect((await call(undefined, "GET", "/store/v1/plugins/nope-not-here")).status).toBe(404);
    expect((await call(undefined, "GET", "/store/v1/plugins/Bad..Id")).status).toBe(404);
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/1.0.0/approve`)).status).toBe(200);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/suspend`, { reason: "Looking into it." })).status).toBe(200);
    const gone = await call(undefined, "GET", `/store/v1/plugins/${id}`);
    expect([gone.status, (await json(gone)).error]).toEqual([404, "not_found"]);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}/versions/1.0.0/package`)).status).toBe(404);
  });

  it("pages with keyset cursors: every item once, in order, whatever the page size", async () => {
    const a = await author();
    const tag = `t${randomLetters(8)}`;
    const ids: string[] = [];
    for (let i = 0; i < 7; i++) {
      const id = uniqueId("page");
      ids.push(id);
      await listed(a, id, "1.0.0", { manifest: { keywords: [tag] } });
    }
    for (const sort of ["popular", "updated", "new"]) {
      const all = (await list(`keyword=${tag}&sort=${sort}&limit=50`)).items.map((c) => c.id);
      expect(new Set(all)).toEqual(new Set(ids));
      for (const size of [1, 3, 6]) {
        const seen: string[] = [];
        let cursor: string | undefined;
        do {
          const page = await list(`keyword=${tag}&sort=${sort}&limit=${size}${cursor ? `&cursor=${cursor}` : ""}`);
          seen.push(...page.items.map((c) => c.id));
          cursor = page.next_cursor;
        } while (cursor);
        expect(seen, `${sort} by ${size}`).toEqual(all);
      }
    }
    // A cursor belongs to its list.
    const first = await list(`keyword=${tag}&limit=2`);
    const wrong = await call(undefined, "GET", `/store/v1/plugins?category=git&limit=2&cursor=${first.next_cursor}`);
    expect([wrong.status, (await json(wrong)).error]).toEqual([400, "bad_cursor"]);
    expect((await call(undefined, "GET", "/store/v1/plugins?cursor=%7B%7D")).status).toBe(400);
    expect((await call(undefined, "GET", "/store/v1/plugins?limit=51")).status).toBe(400);
    expect((await call(undefined, "GET", "/store/v1/plugins?sort=random")).status).toBe(400);
    expect((await call(undefined, "GET", "/store/v1/plugins?category=games")).status).toBe(400);
  });

  it("searches name, summary, keywords, author and README, best match first, by prefix", async () => {
    const a = await author();
    const word = `w${randomLetters(9)}`;
    const inName = uniqueId(word);
    const inReadme = uniqueId("other");
    await listed(a, inReadme, "1.0.0", { files: { "README.md": `# Other\n\nMentions ${word} once, deep in its text.\n` } });
    await listed(a, inName, "1.0.0");
    const hits = await list(`q=${word}`);
    expect(hits.items.map((c) => c.id)).toEqual([inName, inReadme]);
    // Prefix: the first letters find it.
    expect((await list(`q=${word.slice(0, 6)}`)).items.map((c) => c.id)).toEqual(expect.arrayContaining([inName, inReadme]));
    // By author.
    expect((await list(`q=${a.username}`)).items.map((c) => c.id)).toEqual(expect.arrayContaining([inName, inReadme]));
    // Several words must all match.
    expect((await list(`q=${word}+deep`)).items.map((c) => c.id)).toEqual([inReadme]);
  });

  it("never lets a search be FTS syntax", async () => {
    for (const q of ['"', '" OR 1', "NEAR(a b)", "a*", "*", "-x", "col:value", "^start", "a AND OR NOT", "))((", "%22%29", "\u0000", "ünïcödé", "😀"]) {
      const res = await call(undefined, "GET", `/store/v1/plugins?q=${encodeURIComponent(q)}`);
      expect(res.status, q).toBe(200);
    }
    expect((await list("q=%21%21%21")).items).toEqual([]);
  });

  it("filters by category, keyword and featured", async () => {
    const a = await author();
    const tag = `t${randomLetters(8)}`;
    const gitOne = uniqueId("g");
    const chatOne = uniqueId("c");
    await listed(a, gitOne, "1.0.0", { manifest: { categories: ["git"], keywords: [tag] } });
    await listed(a, chatOne, "1.0.0", { manifest: { categories: ["chat", "agents"], keywords: [tag] } });
    expect((await list(`keyword=${tag}&category=git`)).items.map((c) => c.id)).toEqual([gitOne]);
    expect((await list(`keyword=${tag}&category=agents`)).items.map((c) => c.id)).toEqual([chatOne]);
    // (Signed in: past the cache, which keeps a filtered page up to a minute.)
    expect((await list(`keyword=${tag}&featured=true`, a.userId)).items).toEqual([]);
    const before = await list("featured=true&sort=new");
    expect(before.items.map((c) => c.id)).not.toContain(chatOne);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${chatOne}/feature`, { featured: true })).status).toBe(200);
    expect((await list(`keyword=${tag}&featured=true`, a.userId)).items.map((c) => [c.id, c.featured])).toEqual([[chatOne, true]]);
    // The featured row's first page is purged on the change.
    expect((await list("featured=true&sort=new")).items.map((c) => c.id)).toContain(chatOne);
  });

  it("sorts popular by installs, counted once per account", async () => {
    const a = await author();
    const tag = `t${randomLetters(8)}`;
    const quiet = uniqueId("q");
    const loved = uniqueId("l");
    await listed(a, quiet, "1.0.0", { manifest: { keywords: [tag] } });
    await listed(a, loved, "1.0.0", { manifest: { keywords: [tag] } });
    const fans = [await author(), await author()];
    for (const fan of fans) {
      expect(await json(await call(fan.userId, "POST", `/store/v1/plugins/${loved}/installed`))).toEqual({ counted: true });
      expect(await json(await call(fan.userId, "POST", `/store/v1/plugins/${loved}/installed`))).toEqual({ counted: false });
    }
    expect((await call(undefined, "POST", `/store/v1/plugins/${loved}/installed`)).status).toBe(401);
    expect((await call(fans[0]!.userId, "POST", "/store/v1/plugins/nope-not-here/installed")).status).toBe(404);
    const page = await list(`keyword=${tag}&sort=popular`);
    expect(page.items.map((c) => [c.id, c.installs])).toEqual([
      [loved, 2],
      [quiet, 0]
    ]);
  });

  it("purges the cached page and detail when a version is approved or yanked", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    expect((await json<Card>(await call(undefined, "GET", `/store/v1/plugins/${id}`))).version).toBe("1.0.0");
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.1.0", signer: a.signer }))).status).toBe(201);
    // Pending changes nothing public.
    expect((await json<Card>(await call(undefined, "GET", `/store/v1/plugins/${id}`))).version).toBe("1.0.0");
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/1.1.0/approve`)).status).toBe(200);
    expect((await json<Card>(await call(undefined, "GET", `/store/v1/plugins/${id}`))).version).toBe("1.1.0");
    expect((await call(a.userId, "POST", `/store/v1/plugins/${id}/versions/1.1.0/yank`)).status).toBe(200);
    const after = await json<Card & { versions: { version: string; yanked: boolean }[] }>(await call(undefined, "GET", `/store/v1/plugins/${id}`));
    expect(after.version).toBe("1.0.0");
    expect(after.versions.map((v) => [v.version, v.yanked])).toEqual([
      ["1.1.0", true],
      ["1.0.0", false]
    ]);
  });

  it("serves public reads from the cache, and a signed-in caller never through it", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const first = await call(undefined, "GET", `/store/v1/plugins/${id}`);
    expect(first.headers.get("cache-control")).toBe("public, max-age=60");
    const signedIn = await call(a.userId, "GET", `/store/v1/plugins/${id}`);
    expect(signedIn.headers.get("cache-control")).toBe("private, no-store");
  });
});

describe("store packages and files", () => {
  it("serves an approved package to anyone, and a pending one to its owner and admins only", async () => {
    const a = await author();
    const stranger = await author();
    const id = uniqueId();
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer });
    expect((await publish(a.userId, id, pkg)).status).toBe(201);
    const path = `/store/v1/plugins/${id}/versions/1.0.0/package`;
    expect((await call(undefined, "GET", path)).status).toBe(404);
    expect((await call(stranger.userId, "GET", path)).status).toBe(404);
    const own = await call(a.userId, "GET", path);
    expect(own.status).toBe(200);
    expect(own.headers.get("cache-control")).toBe("private, no-store");
    expect(new Uint8Array(await own.arrayBuffer())).toEqual(pkg);
    expect((await adminCall("GET", path)).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/1.0.0/approve`)).status).toBe(200);
    const pub = await call(undefined, "GET", path);
    expect(pub.status).toBe(200);
    expect(pub.headers.get("content-type")).toBe("application/octet-stream");
    expect(pub.headers.get("content-disposition")).toBe(`attachment; filename="${id}-1.0.0.zeron-plugin"`);
    expect(pub.headers.get("x-content-type-options")).toBe("nosniff");
    expect(pub.headers.get("content-security-policy")).toBe("sandbox; default-src 'none'");
    expect(new Uint8Array(await pub.arrayBuffer())).toEqual(pkg);
  });

  it("serves pictures and the icon inline, and every other file only as an attachment to its owner or admins", async () => {
    const a = await author();
    const id = uniqueId();
    const html = "<html><script>alert(1)</script></html>";
    const fakePng = new TextEncoder().encode("\u0089PNG not really");
    await listed(a, id, "1.0.0", { files: { "page.html": html, "fake.png": fakePng, "deep/dir/x.luau": "return 1" } });
    const file = (p: string, bearer?: string) => call(bearer, "GET", `/store/v1/plugins/${id}/files/1.0.0/${p}`);
    const png = await file("shots/one.png");
    expect(png.status).toBe(200);
    expect(png.headers.get("content-type")).toBe("image/png");
    expect(png.headers.get("content-security-policy")).toBe("sandbox; default-src 'none'");
    expect(png.headers.get("x-content-type-options")).toBe("nosniff");
    expect(png.headers.get("content-disposition")).toBeNull();
    expect(new Uint8Array(await png.arrayBuffer())).toEqual(PNG);
    const icon = await file("icon.svg");
    expect([icon.status, icon.headers.get("content-type"), icon.headers.get("content-security-policy")]).toEqual([200, "image/svg+xml", "sandbox; default-src 'none'"]);
    // Not pictures: nobody but the owner and admins, and never inline.
    for (const p of ["page.html", "fake.png", "init.luau", "deep/dir/x.luau"]) expect((await file(p)).status, p).toBe(404);
    const page = await file("page.html", a.userId);
    expect(page.status).toBe(200);
    expect(page.headers.get("content-type")).toBe("application/octet-stream");
    expect(page.headers.get("content-disposition")).toBe('attachment; filename="page.html"');
    expect(await page.text()).toBe(html);
    const asAdmin = await adminCall("GET", `/store/v1/plugins/${id}/files/1.0.0/deep/dir/x.luau`);
    expect([asAdmin.status, await asAdmin.text()]).toEqual([200, "return 1"]);
  });

  it("refuses paths that leave the version, and never reads outside it", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    for (const p of ["..%2F..%2Fpkg", "%2e%2e/icon.svg", "a//b", ".hidden", "shots/%00.png", "C%3Aicon.svg", "%E0%A4%A"]) {
      const res = await call(a.userId, "GET", `/store/v1/plugins/${id}/files/1.0.0/${p}`);
      expect([400, 404], p).toContain(res.status);
    }
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/files/9.9.9/icon.svg`)).status).toBe(404);
  });

  it("checks a version's state before serving it: a takedown lands within the state's TTL, with the bytes still cached", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    const path = `/store/v1/plugins/${id}/versions/1.0.0/package`;
    const first = await call(undefined, "GET", path);
    expect([first.status, first.headers.get("cache-control")]).toEqual([200, "public, max-age=60"]);
    const sha = first.headers.get("etag")!.replaceAll('"', "");
    // Removed where no purge reaches (another data center): the state here is
    // still the cached one, for at most its 60 s.
    await env.STORE_DB.prepare("UPDATE versions SET status = 'removed' WHERE plugin_id = ?").bind(id).run();
    expect((await call(undefined, "GET", path)).status).toBe(200);
    // The state's TTL passes: gone, though the bytes are still cached.
    const cache = await caches.open("store");
    expect(await cache.delete(packageStateKey(id, "1.0.0"))).toBe(true);
    expect((await call(undefined, "GET", path)).status).toBe(404);
    expect(await cache.match(packageBytesKey(sha))).toBeDefined();
  });

  it("never lets a packaged path reach another plugin's cache keys", async () => {
    const victim = await author();
    const evil = await author();
    const victimId = uniqueId("victim");
    const evilId = uniqueId("evil");
    await listed(victim, victimId, "1.0.0");
    const tricks = [`%2e%2e/%2e%2e/%2e%2e/package/${victimId}/1.0.0`, `%2e%2e/%2e%2e/%2e%2e/%2e%2e/plugins/${victimId}`, "%2E%2E/%2E%2E/%2E%2E/%2E%2E/categories"];
    await listed(evil, evilId, "1.0.0", { files: Object.fromEntries(tricks.map((t) => [t, PNG])) });
    for (const t of tricks) {
      const res = await call(undefined, "GET", `/store/v1/plugins/${evilId}/files/1.0.0/${t.split("/").map(encodeURIComponent).join("/")}`);
      expect(res.status, t).toBe(200);
      expect(res.headers.get("content-type")).toBe("image/png");
    }
    const detail = await call(undefined, "GET", `/store/v1/plugins/${victimId}`);
    expect([detail.status, detail.headers.get("content-type")]).toEqual([200, "application/json; charset=utf-8"]);
    const pkg = await call(undefined, "GET", `/store/v1/plugins/${victimId}/versions/1.0.0/package`);
    expect(pkg.headers.get("content-type")).toBe("application/octet-stream");
    expect((await call(undefined, "GET", "/store/v1/categories")).headers.get("content-type")).toBe("application/json; charset=utf-8");
  });

  it("answers a HEAD without ever caching an empty body for a GET", async () => {
    const a = await author();
    const id = uniqueId();
    await listed(a, id, "1.0.0");
    for (const path of [`/store/v1/plugins/${id}/versions/1.0.0/package`, `/store/v1/plugins/${id}/files/1.0.0/shots/one.png`, `/store/v1/plugins/${id}`]) {
      const head = await call(undefined, "HEAD", path);
      expect([head.status, (await head.arrayBuffer()).byteLength], path).toEqual([200, 0]);
      const get = await call(undefined, "GET", path);
      expect((await get.arrayBuffer()).byteLength, path).toBeGreaterThan(0);
    }
  });
});
