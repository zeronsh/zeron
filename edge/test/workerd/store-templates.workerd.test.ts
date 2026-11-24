/**
 * Templates in the store (contract a33b7c5b6, "Templates in the store"):
 * published, browsed, reviewed and updated beside plugins, under their own
 * prefix, in one namespace of ids. A template never shows where a plugin is
 * asked for, nor the reverse, whatever the cache holds.
 */
import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { crc32 } from "../../src/image-check";
import { handlePublish } from "../../src/store/publish";
import { openCache } from "../../src/store/cache";
import { fileStateKey, packageStateKey } from "../../src/store/serve";
import { UNUSED_ID_DAYS, releaseRows, releaseUnusedIds } from "../../src/store/release";
import { stockOf } from "../../src/store/stock";
import { MAX_UPDATE_ITEMS } from "../../src/store/updates";
import { PNG, adminCall, author, call, deployToken, listed, listedTemplate, makePackage, makeTemplate, newSigner, publish, publishTemplate, publishedTemplate, randomLetters, tokenPublish, tokenPublishTemplate, uniqueId } from "./store-helpers";

const json = async <T = Record<string, any>>(res: Response) => (await res.json()) as T;
const get = (path: string, bearer?: string) => call(bearer, "GET", `/store/v1${path}`);
const error = async (res: Response) => [res.status, (await json<{ error: string }>(res)).error] as const;

/** A PNG's first bytes and some: what a device's preview check takes, and
 * image-check doesn't. */
const FAKE_PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4]);

/** PNG with an animation control chunk after its header: an APNG, sound in
 * every chunk. */
const animated = (png: Uint8Array): Uint8Array => {
  const chunk = new Uint8Array(20);
  const view = new DataView(chunk.buffer);
  view.setUint32(0, 8);
  chunk.set(new TextEncoder().encode("acTL"), 4);
  view.setUint32(8, 1);
  view.setUint32(16, crc32(chunk.subarray(4, 16)));
  const header = 8 + 25;
  return new Uint8Array([...png.subarray(0, header), ...chunk, ...png.subarray(header)]);
};

describe("templates: publishing and browsing one", () => {
  it("lists it at once, with a cover and no icon or permissions, and serves its package and preview", async () => {
    const a = await author();
    const id = uniqueId("t");
    const tag = `t${randomLetters(8)}`;
    const answer = await publishedTemplate(a, id, "1.0.0", { manifest: { keywords: [tag] } });
    expect(answer).toMatchObject({ id, version: "1.0.0", status: "listed", verified: false });
    const list = await json<{ items: Record<string, unknown>[]; total: number }>(await get(`/templates?keyword=${tag}`));
    expect(list.total).toBe(1);
    expect(list.items[0]).toMatchObject({ id, kind: "template", version: "1.0.0", icon: null, cover: "preview.png", categories: [], keywords: [tag], verified: false, author: { username: a.username } });
    const detail = await json(await get(`/templates/${id}`));
    expect(detail).toMatchObject({
      id, kind: "template", icon: null, cover: "preview.png", screenshots: ["screenshots/one.png"], engines: { zeron: ">=0.3.0" },
      versions: [{ version: "1.0.0", kind: "template", engines: { zeron: ">=0.3.0" }, key: a.signer.key, via: "device", notes: "- What changed in 1.0.0." }],
      package: { url: `/store/v1/templates/${id}/versions/1.0.0/package`, key: a.signer.key }
    });
    expect(detail).not.toHaveProperty("permissions");
    const pkg = await get(`/templates/${id}/versions/1.0.0/package`);
    expect([pkg.status, pkg.headers.get("content-disposition")]).toEqual([200, `attachment; filename="${id}-1.0.0.zeron-template"`]);
    const cover = await get(`/templates/${id}/files/1.0.0/preview.png`);
    expect([cover.status, cover.headers.get("content-type")]).toEqual([200, "image/png"]);
    // (Its other files are its owner's and staff's, as a plugin's are.)
    expect((await get(`/templates/${id}/files/1.0.0/app.luau`)).status).toBe(404);
    expect((await get(`/templates/${id}/files/1.0.0/app.luau`, a.userId)).status).toBe(200);
    expect(await json(await get(`/templates/${id}/versions`))).toMatchObject({ total: 1, versions: [{ version: "1.0.0" }] });
    expect(await json(await get(`/templates/${id}/versions/1.0.0`))).toMatchObject({ version: "1.0.0", kind: "template", engines: { zeron: ">=0.3.0" } });
    // The owner's view is its own: the shape of /me/plugins.
    expect(await json(await get("/me/templates", a.userId))).toMatchObject({ publisher_id: expect.stringMatching(/^pub_/), templates: [{ id, kind: "template", versions: [{ version: "1.0.0" }] }] });
    // A newer version is the card's; a cached first page shows it (purged).
    const firstPage = async () => (await json<{ items: { id: string; version: string }[] }>(await get("/templates?sort=updated"))).items.find((c) => c.id === id);
    expect((await firstPage())?.version).toBe("1.0.0");
    await publishedTemplate(a, id, "1.1.0", { manifest: { keywords: [tag] } });
    expect((await firstPage())?.version).toBe("1.1.0");
    expect((await json(await get(`/templates/${id}`))).version).toBe("1.1.0");
  });

  it("never shows where a plugin is asked for, nor the reverse, cached or not", async () => {
    const a = await author();
    const tag = `t${randomLetters(8)}`;
    const plug = uniqueId("p");
    const temp = uniqueId("t");
    await listed(a, plug, "1.0.0", { manifest: { keywords: [tag] } });
    await listedTemplate(a, temp, "1.0.0", { manifest: { keywords: [tag] } });
    expect((await adminCall("POST", `/store/v1/admin/plugins/${plug}/feature`, { featured: true })).status).toBe(200);
    expect((await adminCall("POST", `/store/v1/admin/templates/${temp}/feature`, { featured: true })).status).toBe(200);
    const ids = async (path: string) => (await json<{ items: { id: string }[] }>(await get(path))).items.map((c) => c.id);
    for (const query of [`keyword=${tag}`, `author=${a.username}`, `author=${a.username}&featured=true`, `author=${a.username}&verified=1`, `author=${a.username}&sort=verified`, `q=${plug}`, `q=${temp}`]) {
      const plugins = await ids(`/plugins?${query}`);
      const templates = await ids(`/templates?${query}`);
      expect(plugins, query).not.toContain(temp);
      expect(templates, query).not.toContain(plug);
      if (!query.startsWith("q=")) {
        expect(plugins, query).toContain(plug);
        expect(templates, query).toContain(temp);
      }
    }
    // The other kind's id is unknown under a prefix: detail, versions, one
    // version, package, files; even with the right one's answer cached.
    for (const [right, wrong, id] of [["plugins", "templates", plug], ["templates", "plugins", temp]] as const) {
      const paths = [`/${id}`, `/${id}/versions`, `/${id}/versions/1.0.0`, `/${id}/versions/1.0.0/package`, `/${id}/files/1.0.0/README.md`];
      for (const path of paths) {
        expect((await get(`/${right}${path}`, path.endsWith("README.md") ? a.userId : undefined)).status, `${right}${path}`).toBe(200);
        // (Twice: the first answer is the cache's to keep.)
        expect(await error(await get(`/${wrong}${path}`)), `${wrong}${path}`).toEqual([404, "not_found"]);
        expect(await error(await get(`/${wrong}${path}`)), `${wrong}${path} again`).toEqual([404, "not_found"]);
      }
    }
    // Store-wide counts: a plugin's by default, a template's when asked.
    const keywords = async (kind: string) => (await json<{ keywords: { keyword: string; count: number }[] }>(await get(`/keywords${kind}`))).keywords.map((k) => k.keyword);
    expect(await keywords("?kind=plugin")).toContain(tag);
    expect(await keywords("?kind=template")).toContain(tag);
    const before = { p: await json(await get("/stats")), t: await json(await get("/stats?kind=template")) };
    expect(before.p.plugins).toBeGreaterThan(0);
    expect(before.t.plugins).toBeGreaterThan(0);
    expect(await json(await get("/categories?kind=template"))).toEqual({ categories: [] });
    expect(await json(await get("/categories?kind=template&top=2"))).toEqual({ categories: [] });
    expect((await json<{ categories: unknown[] }>(await get("/categories"))).categories).toHaveLength(8);
    for (const path of ["/stats?kind=widget", "/keywords?kind=widget", "/categories?kind=widget"]) expect((await get(path)).status, path).toBe(400);
    // The counts are each kind's own: one more template moves only its own.
    const another = uniqueId("t");
    await publishedTemplate(a, another, "1.0.0", { manifest: { keywords: [tag] } });
    expect((await json(await get("/stats"))).plugins).toBe(before.p.plugins);
    expect((await json(await get("/stats?kind=template"))).plugins).toBe(before.t.plugins + 1);
    // /me: each has its own.
    expect((await json<{ plugins: { id: string }[] }>(await get("/me/plugins", a.userId))).plugins.map((p) => p.id)).toEqual([plug]);
    expect((await json<{ templates: { id: string }[] }>(await get("/me/templates", a.userId))).templates.map((p) => p.id).sort()).toEqual([another, temp].sort());
  });
});

describe("templates: what's cached from before them", () => {
  it("serves a plugin's package and pictures whose cached state has no kind, under its own prefix only", async () => {
    const a = await author();
    const id = uniqueId("p");
    await listed(a, id, "1.0.0", { files: { "shot.png": PNG } });
    const paths = [`/versions/1.0.0/package`, `/files/1.0.0/shot.png`];
    const cache = await openCache();
    // As the release before wrote them: the same states, without a kind.
    for (const path of paths) expect((await get(`/plugins/${id}${path}`)).status, path).toBe(200);
    for (const [key, member] of [[packageStateKey(id, "1.0.0"), "kind"], [fileStateKey(id, "1.0.0", "shot.png"), "itemKind"]] as const) {
      const state = (await (await cache.match(key))!.json()) as Record<string, unknown>;
      expect(state[member]).toBe("plugin");
      delete state[member];
      await cache.put(key, Response.json(state, { headers: { "cache-control": "public, max-age=60" } }));
    }
    for (const path of paths) {
      expect((await get(`/plugins/${id}${path}`)).status, path).toBe(200);
      expect((await get(`/templates/${id}${path}`)).status, path).toBe(404);
    }
  });
});

describe("templates: one namespace", () => {
  it("is 409 wrong_kind to the owner under the other prefix, 403 id_taken to anyone else, and never takes a plugin's id", async () => {
    const a = await author();
    const b = await author();
    const plug = uniqueId("p");
    const temp = uniqueId("t");
    await listed(a, plug, "1.0.0");
    await publishedTemplate(a, temp, "1.0.0");
    const template = (id: string, who: typeof a) => makeTemplate({ id, version: "2.0.0", signer: who.signer });
    const plugin = (id: string, who: typeof a) => makePackage({ id, version: "2.0.0", signer: who.signer });
    const own = await publishTemplate(a.userId, plug, await template(plug, a));
    expect([own.status, await own.json()]).toEqual([409, expect.objectContaining({ error: "wrong_kind", message: `"${plug}" is a plugin.` })]);
    expect(await error(await publish(a.userId, temp, await plugin(temp, a)))).toEqual([409, "wrong_kind"]);
    // To anyone else: another account's, before a byte of the body is read.
    const other = await publishTemplate(b.userId, plug, new TextEncoder().encode("not a package"));
    expect(await error(other)).toEqual([403, "id_taken"]);
    expect(await error(await publish(b.userId, temp, new TextEncoder().encode("not a package")))).toEqual([403, "id_taken"]);
    // Under the id's own prefix: another account's, as always.
    expect(await error(await publishTemplate(b.userId, temp, await template(temp, b)))).toEqual([403, "id_taken"]);
    // Nothing of it landed.
    const kinds = await env.STORE_DB.prepare("SELECT id, kind FROM plugins WHERE id IN (?, ?) ORDER BY kind").bind(plug, temp).all<{ id: string; kind: string }>();
    expect(kinds.results).toEqual([{ id: plug, kind: "plugin" }, { id: temp, kind: "template" }]);
    expect((await env.STORE_DB.prepare("SELECT COUNT(*) AS n FROM versions WHERE plugin_id IN (?, ?) AND version = '2.0.0'").bind(plug, temp).first<{ n: number }>())!.n).toBe(0);
  });

  it("gives a first version's id to one kind when both are published at once, and no version of the other", async () => {
    for (let i = 0; i < 3; i++) {
      const a = await author();
      const id = uniqueId("r");
      const [t, p] = await Promise.all([
        publishTemplate(a.userId, id, await makeTemplate({ id, version: "1.0.0", signer: a.signer })),
        publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))
      ]);
      const statuses = [t.status, p.status].sort();
      expect(statuses, `${await t.clone().text()} / ${await p.clone().text()}`).toEqual([201, 409]);
      const row = (await env.STORE_DB.prepare("SELECT kind FROM plugins WHERE id = ?").bind(id).first<{ kind: string }>())!;
      expect(row.kind).toBe(t.status === 201 ? "template" : "plugin");
      const versions = await env.STORE_DB.prepare("SELECT icon FROM versions WHERE plugin_id = ?").bind(id).all<{ icon: string }>();
      expect(versions.results).toEqual([{ icon: row.kind === "template" ? "" : "icon.svg" }]);
    }
  });

  it("never lands a version under an id the other kind claimed while it was being written", async () => {
    const a = await author();
    const b = await author();
    // While a template's objects are being written, its id is claimed and
    // published as a plugin by `who`: a lower version, so nothing but its
    // kind stops the template's.
    const whileWriting = async (who: typeof a) => {
      const id = uniqueId("r");
      let claimed = false;
      const racing = new Proxy(env.PLUGIN_STORE, {
        get: (target, prop) =>
          prop === "put"
            ? async (...args: Parameters<R2Bucket["put"]>) => {
                if (!claimed) {
                  claimed = true;
                  expect((await publish(who.userId, id, await makePackage({ id, version: "1.0.0", signer: who.signer }))).status).toBe(201);
                }
                return target.put(...args);
              }
            : Reflect.get(target, prop, target).bind?.(target) ?? Reflect.get(target, prop, target)
      }) as R2Bucket;
      const request = new Request(`https://edge.test/store/v1/templates/${id}/versions`, {
        method: "POST",
        headers: { authorization: `Bearer ${a.userId}` },
        body: await makeTemplate({ id, version: "2.0.0", signer: a.signer })
      });
      const res = await handlePublish(request, env as never, env.STORE_DB, racing, "template", id);
      expect(claimed).toBe(true);
      const versions = await env.STORE_DB.prepare("SELECT v.version, p.kind FROM versions v JOIN plugins p ON p.id = v.plugin_id WHERE p.id = ?").bind(id).all();
      expect(versions.results).toEqual([{ version: "1.0.0", kind: "plugin" }]);
      return error(res);
    };
    // Its owner: told which kind it is now. Anyone else: it's another account's.
    expect(await whileWriting(a)).toEqual([409, "wrong_kind"]);
    expect(await whileWriting(b)).toEqual([403, "id_taken"]);
  });

  it("shares reserved ids, lookalikes, the weekly limit and the hourly one", async () => {
    const a = await author();
    const ok = await makeTemplate({ id: "zeron-look", version: "1.0.0", signer: a.signer });
    expect(await error(await publishTemplate(a.userId, "zeron-look", ok))).toEqual([403, "id_reserved"]);
    // The site's own page words, and a built-in plugin's.
    for (const id of ["templates", "git"]) {
      expect(await error(await publishTemplate(a.userId, id, await makeTemplate({ id, version: "1.0.0", signer: a.signer }))), id).toEqual([403, "id_reserved"]);
    }
    // A reservation holds an id for either kind.
    const held = uniqueId("h");
    const b = await author();
    expect((await adminCall("POST", "/store/v1/admin/reservations", { id: held, to_username: b.username })).status).toBe(200);
    expect(await error(await publishTemplate(a.userId, held, await makeTemplate({ id: held, version: "1.0.0", signer: a.signer })))).toEqual([403, "id_reserved"]);
    expect((await publishTemplate(b.userId, held, await makeTemplate({ id: held, version: "1.0.0", signer: b.signer }))).status).toBe(201);
    // Five new ids a week, both kinds together.
    const c = await author();
    for (let i = 0; i < 5; i++) {
      const id = uniqueId("w");
      const res = i % 2 ? await publish(c.userId, id, await makePackage({ id, version: "1.0.0", signer: c.signer })) : await publishTemplate(c.userId, id, await makeTemplate({ id, version: "1.0.0", signer: c.signer }));
      expect(res.status, `${i}`).toBe(201);
    }
    const sixth = uniqueId("w");
    expect(await error(await publishTemplate(c.userId, sixth, await makeTemplate({ id: sixth, version: "1.0.0", signer: c.signer })))).toEqual([429, "rate_limited"]);
  });
});

describe("templates: what's refused", () => {
  it("refuses protected files, refused members, a bad preview and the README Zeron wrote, saying why", async () => {
    const a = await author();
    const problems = async (o: { manifest?: Record<string, unknown>; files?: Record<string, string | Uint8Array | undefined> }) => {
      const id = uniqueId("x");
      const res = await publishTemplate(a.userId, id, await makeTemplate({ id, version: "1.0.0", signer: a.signer, ...o }));
      const body = await json<{ error: string; field: string | null; path: string | null }>(res);
      return [res.status, body.error, body.field, body.path] as const;
    };
    expect(await problems({ files: { "screens/plugins/page.luau": "x" } })).toEqual([400, "protected_file", "files", "screens/plugins/page.luau"]);
    expect(await problems({ files: { "Account/x.luau": "x" } })).toEqual([400, "protected_file", "files", "Account/x.luau"]);
    expect(await problems({ files: { ".luaurc": "{}" } })).toEqual([400, "protected_file", "files", ".luaurc"]);
    for (const member of ["builtin_theme", "builtin_template", "permissions", "icon", "categories", "engines"]) {
      expect(await problems({ manifest: { [member]: member === "engines" ? { zeron: ">=1.0.0" } : true } }), member).toEqual([400, "bad_listing", member, null]);
    }
    // Nothing but pictures under screenshots/, however its folder is spelt.
    for (const stray of ["screenshots/x.luau", "Screenshots/x.luau"]) {
      expect(await problems({ files: { [stray]: "return {}" } }), stray).toEqual([400, "bad_listing", "files", stray]);
    }
    // An allow list: no other member.
    for (const member of ["author", "x-future"]) {
      expect(await problems({ manifest: { [member]: "Zeron" } }), member).toEqual([400, "bad_listing", "manifest", null]);
    }
    expect(await problems({ files: { "preview.png": undefined } })).toEqual([400, "missing_listing", "preview", null]);
    // A preview that isn't a picture the store serves: past the device's
    // check (a PNG's first bytes), refused on image-check's.
    expect(await problems({ files: { "preview.png": FAKE_PNG } })).toEqual([400, "bad_listing", "preview", "preview.png"]);
    expect(await problems({ files: { "preview.png": new TextEncoder().encode("not a picture") } })).toEqual([400, "bad_listing", "preview", "preview.png"]);
    // A still one only: an animation is refused, not flattened.
    expect(await problems({ files: { "preview.png": animated(PNG) } })).toEqual([400, "bad_listing", "preview", "preview.png"]);
    expect(await problems({ files: { "README.md": "# x\n\nThis README isn't part of the interface.\n" } })).toEqual([400, "missing_readme", null, "README.md"]);
    expect(await problems({ files: { "README.md": undefined } })).toEqual([400, "missing_readme", null, null]);
    expect(await problems({ manifest: { zeron_version: undefined } })).toEqual([400, "missing_listing", "zeron_version", null]);
    expect(await problems({ manifest: { zeron_version: "0.4" } })).toEqual([400, "not_semver", "zeron_version", null]);
    expect(await problems({ manifest: { name: "Verified Midnight" } })).toEqual([400, "bad_listing", "name", null]);
    expect(await problems({ manifest: { description: "A clean, verified layout." } })).toEqual([400, "bad_listing", "description", null]);
    expect(await problems({ files: { "screenshots/x.png": PNG }, manifest: { screenshots: ["preview.png"] } })).toEqual([400, "bad_listing", "screenshots", "preview.png"]);
  });

  it("refuses the other kind's package at each prefix, another domain's signature, and an id that isn't its own", async () => {
    const a = await author();
    const id = uniqueId("x");
    const plug = uniqueId("x");
    // A plugin's package at the templates' prefix, and the reverse.
    const asPlugin = await publishTemplate(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }));
    expect([asPlugin.status, await json(asPlugin)]).toEqual([400, expect.objectContaining({ error: "bad_package", reason: "not_a_package" })]);
    const asTemplate = await publish(a.userId, plug, await makeTemplate({ id: plug, version: "1.0.0", signer: a.signer }));
    expect([asTemplate.status, await json(asTemplate)]).toEqual([400, expect.objectContaining({ error: "bad_package", reason: "not_a_package" })]);
    // A template signed in the plugin domain.
    const wrongDomain = await publishTemplate(a.userId, id, await makeTemplate({ id, version: "1.0.0", signer: a.signer, prefix: "zeron-plugin:v1\n" }));
    expect(await error(wrongDomain)).toEqual([400, "bad_signature"]);
    const unsigned = await publishTemplate(a.userId, id, await makeTemplate({ id, version: "1.0.0", signer: null }));
    expect([unsigned.status, await json(unsigned)]).toEqual([400, expect.objectContaining({ error: "bad_signature", reason: "unsigned" })]);
    const stranger = await newSigner();
    expect(await error(await publishTemplate(a.userId, id, await makeTemplate({ id, version: "1.0.0", signer: stranger })))).toEqual([403, "key_not_allowed"]);
    expect((await json(await publishTemplate(a.userId, id, await makeTemplate({ id: uniqueId("y"), version: "1.0.0", signer: a.signer })))).reason).toBe("id_mismatch");
  });
});

describe("templates: deploy tokens", () => {
  it("publishes a template's next versions unsigned, for a scope of ids of either kind, and never a first version or a read", async () => {
    const a = await author();
    const plug = uniqueId("p");
    const temp = uniqueId("t");
    const outside = uniqueId("t");
    await listed(a, plug, "1.0.0");
    await publishedTemplate(a, temp, "1.0.0");
    await publishedTemplate(a, outside, "1.0.0");
    const { token } = await deployToken(a.userId, { plugins: [plug, temp] });
    const pkg = async (id: string, version: string) => makeTemplate({ id, version, signer: null });
    const ok = await tokenPublishTemplate(token, temp, await pkg(temp, "1.1.0"));
    expect(ok.status, await ok.clone().text()).toBe(201);
    const row = await env.STORE_DB.prepare("SELECT via, signer_key FROM versions WHERE plugin_id = ? AND version = '1.1.0'").bind(temp).first();
    expect(row).toEqual({ via: "deploy_token", signer_key: null });
    expect((await json(await get(`/templates/${temp}/versions/1.1.0`)))).toMatchObject({ via: "deploy_token", key: null });
    // The same token, on the plugin of its scope.
    expect((await tokenPublish(token, plug, await makePackage({ id: plug, version: "1.1.0", signer: null }))).status).toBe(201);
    // A signed one is refused; one outside its scope, a first version too.
    expect(await error(await tokenPublishTemplate(token, temp, await makeTemplate({ id: temp, version: "1.2.0", signer: a.signer })))).toEqual([400, "bad_signature"]);
    expect(await error(await tokenPublishTemplate(token, outside, await pkg(outside, "1.1.0")))).toEqual([403, "token_not_allowed"]);
    const fresh = uniqueId("t");
    expect(await error(await tokenPublishTemplate(token, fresh, await pkg(fresh, "1.0.0")))).toEqual([403, "token_not_allowed"]);
    // Under the other prefix: its owner's, so told which kind it is.
    expect(await error(await tokenPublishTemplate(token, plug, await pkg(plug, "2.0.0")))).toEqual([409, "wrong_kind"]);
    expect(await error(await tokenPublish(token, temp, await makePackage({ id: temp, version: "2.0.0", signer: null })))).toEqual([409, "wrong_kind"]);
    // Not in the token's scope: that answers first, whatever its prefix.
    const { token: narrow } = await deployToken(a.userId, { plugins: [plug] });
    const { token: narrowest } = await deployToken(a.userId, { plugins: [temp] });
    expect(await error(await tokenPublishTemplate(narrowest, plug, await pkg(plug, "2.0.0")))).toEqual([403, "token_not_allowed"]);
    expect(await error(await tokenPublish(narrow, temp, await makePackage({ id: temp, version: "2.0.0", signer: null })))).toEqual([403, "token_not_allowed"]);
    // It reads nothing.
    for (const path of ["/templates", `/templates/${temp}`, "/me/templates", "/me/plugins"]) {
      expect(await error(await call(undefined, "GET", `/store/v1${path}`, undefined, { authorization: `Bearer ${token}` })), path).toEqual([401, "session_required"]);
    }
    expect(await error(await call(undefined, "POST", `/store/v1/templates/${temp}/versions/1.1.0/yank`, undefined, { authorization: `Bearer ${token}` }))).toEqual([401, "session_required"]);
  });

  it("holds an `all` token to the kinds it names, those claimed later included", async () => {
    const a = await author();
    const { token: plugins } = await deployToken(a.userId, { all: true });
    const { token: both } = await deployToken(a.userId, { all: true, all_kinds: ["plugin", "template"] });
    const plug = uniqueId("p");
    const temp = uniqueId("t");
    await listed(a, plug, "1.0.0");
    await publishedTemplate(a, temp, "1.0.0");
    const template = (version: string) => makeTemplate({ id: temp, version, signer: null });
    const plugin = (version: string) => makePackage({ id: plug, version, signer: null });
    expect((await tokenPublish(plugins, plug, await plugin("1.1.0"))).status).toBe(201);
    expect(await error(await tokenPublishTemplate(plugins, temp, await template("1.1.0")))).toEqual([403, "token_not_allowed"]);
    // Under the other prefix too, before it's told which kind it is.
    expect(await error(await tokenPublish(plugins, temp, await makePackage({ id: temp, version: "1.1.0", signer: null })))).toEqual([403, "token_not_allowed"]);
    expect((await tokenPublish(both, plug, await plugin("1.2.0"))).status).toBe(201);
    expect((await tokenPublishTemplate(both, temp, await template("1.1.0"))).status).toBe(201);
  });
});

describe("templates: what its author and anyone do", () => {
  it("yanks and unyanks, counts installs and takes reports, each under its own prefix only", async () => {
    const a = await author();
    const b = await author();
    const temp = uniqueId("t");
    await listedTemplate(a, temp, "1.0.0");
    await publishedTemplate(a, temp, "1.1.0");
    const yank = (prefix: string, who: string, version: string, action = "yank") => call(who, "POST", `/store/v1/${prefix}/${temp}/versions/${version}/${action}`);
    expect((await yank("plugins", a.userId, "1.1.0")).status).toBe(404);
    expect((await yank("templates", b.userId, "1.1.0")).status).toBe(404);
    expect(await json(await yank("templates", a.userId, "1.1.0"))).toMatchObject({ version: { kind: "template", yanked: true, yanked_by: "author" } });
    expect((await json(await get(`/templates/${temp}`))).version).toBe("1.0.0");
    expect((await yank("templates", a.userId, "1.1.0", "unyank")).status).toBe(200);
    expect((await json(await get(`/templates/${temp}`))).version).toBe("1.1.0");
    expect((await call(b.userId, "POST", `/store/v1/plugins/${temp}/installed`)).status).toBe(404);
    expect(await json(await call(b.userId, "POST", `/store/v1/templates/${temp}/installed`))).toEqual({ counted: true });
    expect((await json(await get(`/templates/${temp}`, b.userId))).installs).toBe(1);
    const report = { reason: "broken", text: "It doesn't open." };
    expect((await call(b.userId, "POST", `/store/v1/plugins/${temp}/reports`, report)).status).toBe(404);
    expect((await call(b.userId, "POST", `/store/v1/templates/${temp}/reports`, report)).status).toBe(201);
    const reports = await json<{ reports: { plugin: { id: string; kind: string } }[] }>(await adminCall("GET", "/store/v1/admin/reports"));
    const row = reports.reports.find((r) => r.plugin.id === temp)!;
    expect(row.plugin).toEqual({ id: temp, name: expect.any(String), kind: "template" });
    expect(row).not.toHaveProperty("kind");
  });
});

describe("templates: updates", () => {
  it("offers a template's newest version beside the plugins', and nothing to a list of the other kind", async () => {
    const a = await author();
    const plug = uniqueId("p");
    const temp = uniqueId("t");
    await listed(a, plug, "1.0.0");
    await listed(a, plug, "1.1.0");
    await listedTemplate(a, temp, "1.0.0");
    await listedTemplate(a, temp, "1.1.0", { manifest: { zeron_version: "0.5.0" } });
    const hashOf = async (id: string, version: string) => (await env.STORE_DB.prepare("SELECT content_hash FROM versions WHERE plugin_id = ? AND version = ?").bind(id, version).first<{ content_hash: string }>())!.content_hash;
    const [p, t] = [{ id: plug, version: "1.0.0", hash: await hashOf(plug, "1.0.0") }, { id: temp, version: "1.0.0", hash: await hashOf(temp, "1.0.0") }];
    const ask = async (body: unknown) => json(await call(undefined, "POST", "/store/v1/updates", body));
    // 1.1.0 was made from Zeron 0.5.0: not for an older one.
    expect((await ask({ zeron: "0.4.0", plugins: [], templates: [t] })).template_updates).toEqual([]);
    const both = await ask({ zeron: "0.5.0", plugins: [p], templates: [t] });
    expect(both.updates).toEqual([expect.objectContaining({ id: plug, kind: "plugin", version: "1.1.0" })]);
    expect(both.template_updates).toEqual([expect.objectContaining({ id: temp, kind: "template", version: "1.1.0", hash: await hashOf(temp, "1.1.0"), added: [], removed: [], verified: true, publisher: a.username })]);
    // Asked about as the other kind: unknown. Never offered, never blocked.
    expect(await ask({ zeron: "0.5.0", plugins: [t], templates: [p] })).toEqual({ updates: [], template_updates: [], blocked: [] });
    // Unverified, with the verified one beside it.
    await publishedTemplate(a, temp, "1.2.0", { manifest: { zeron_version: "0.5.0" } });
    const newest = await ask({ zeron: "0.5.0", plugins: [], templates: [t] });
    expect(newest.template_updates).toEqual([expect.objectContaining({ version: "1.2.0", verified: false, verified_update: expect.objectContaining({ version: "1.1.0", verified: true }) })]);
    expect((await ask({ zeron: "0.5.0", plugins: [], templates: [{ ...t, verified_only: true }] })).template_updates).toEqual([expect.objectContaining({ version: "1.1.0" })]);
    // Taken down: blocked, and named a template.
    const bad = { id: temp, version: "1.2.0", hash: await hashOf(temp, "1.2.0") };
    expect((await adminCall("POST", `/store/v1/admin/templates/${temp}/versions/1.2.0/take-down`, { reason: "It sends your files away." })).status).toBe(200);
    expect((await ask({ zeron: "0.5.0", plugins: [], templates: [bad] })).blocked).toEqual([{ id: temp, version: "1.2.0", reason: "It sends your files away.", item_kind: "template" }]);
    expect((await ask({ zeron: "0.5.0", plugins: [bad] })).blocked).toEqual([]);
    // A taken-down version's record stays, so its id can't be released, not
    // even by an admin, nor claimed as the other kind: the block stands.
    expect(await releaseRows(env.STORE_DB, temp, Infinity, "user_admin", false)).toBeUndefined();
    await releaseUnusedIds(env as never, Date.now() + 10 * UNUSED_ID_DAYS * 86_400_000);
    expect(await error(await publish(a.userId, temp, await makePackage({ id: temp, version: "2.0.0", signer: a.signer })))).toEqual([409, "wrong_kind"]);
    expect((await ask({ zeron: "0.5.0", plugins: [], templates: [bad] })).blocked).toEqual([expect.objectContaining({ id: temp, item_kind: "template" })]);
    // One limit over both lists; a list that isn't one.
    const many = (n: number, prefix: string) => Array.from({ length: n }, (_, i) => ({ id: `${prefix}${i}x`, version: "1.0.0", hash: "0".repeat(64) }));
    expect((await call(undefined, "POST", "/store/v1/updates", { zeron: "0.5.0", plugins: many(MAX_UPDATE_ITEMS, "p"), templates: [] })).status).toBe(200);
    expect((await call(undefined, "POST", "/store/v1/updates", { zeron: "0.5.0", plugins: many(MAX_UPDATE_ITEMS - 1, "p"), templates: many(2, "t") })).status).toBe(400);
    expect((await call(undefined, "POST", "/store/v1/updates", { zeron: "0.5.0", plugins: [], templates: "x" })).status).toBe(400);
    expect((await call(undefined, "POST", "/store/v1/updates", { zeron: "0.5.0", plugins: [], templates: [t, t] })).status).toBe(400);
  });
});

describe("templates: staff", () => {
  it("queues both kinds, reviews a template without permissions, and keeps each prefix to its own kind", async () => {
    const a = await author();
    const plug = uniqueId("p");
    const temp = uniqueId("t");
    expect((await publish(a.userId, plug, await makePackage({ id: plug, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    await publishedTemplate(a, temp, "1.0.0");
    const queue = async (query: string) => (await json<{ items: { kind: string; plugin: { id: string }; icon: string | null; new_asks: string[]; asks_changed: boolean }[]; total: number }>(await adminCall("GET", `/store/v1/admin/queue?limit=50&${query}`))).items;
    const both = await queue("");
    expect(both.find((i) => i.plugin.id === plug)).toMatchObject({ kind: "plugin", icon: "icon.svg" });
    expect(both.find((i) => i.plugin.id === temp)).toMatchObject({ kind: "template", icon: null, new_asks: [], asks_changed: false });
    expect((await queue("kind=template")).every((i) => i.kind === "template")).toBe(true);
    expect((await queue("kind=template")).map((i) => i.plugin.id)).toContain(temp);
    expect((await queue("kind=plugin")).map((i) => i.plugin.id)).not.toContain(temp);
    expect((await adminCall("GET", "/store/v1/admin/queue?kind=widget")).status).toBe(400);
    const view = await json(await adminCall("GET", `/store/v1/admin/versions/${temp}/1.0.0`));
    expect(view).toMatchObject({ kind: "template", plugin: { kind: "template", cover: "preview.png" }, version: { icon: null, kind: "template", engines: { zeron: ">=0.3.0" } }, permissions: null, permissions_diff: null });
    // (The checks of a template: none of what it asks.)
    expect((view.checks as { id: string }[]).map((c) => c.id)).not.toContain("asks");
    expect((await json(await adminCall("GET", `/store/v1/admin/versions/${plug}/1.0.0`))).kind).toBe("plugin");
    // Routes that take only an id take either; the mirrored ones, their own.
    expect((await adminCall("POST", `/store/v1/admin/versions/${temp}/1.0.0/verify`)).status).toBe(200);
    for (const [prefix, id] of [["plugins", temp], ["templates", plug]] as const) {
      expect((await adminCall("GET", `/store/v1/admin/${prefix}/${id}`)).status, prefix).toBe(404);
      expect((await adminCall("POST", `/store/v1/admin/${prefix}/${id}/suspend`, { reason: "x" })).status, prefix).toBe(404);
      expect((await adminCall("POST", `/store/v1/admin/${prefix}/${id}/versions/1.0.0/take-down`, { reason: "x" })).status, prefix).toBe(404);
    }
    expect(await json(await adminCall("GET", `/store/v1/admin/templates/${temp}`))).toMatchObject({ plugin: { id: temp, kind: "template" } });
    // Feature a template: its shelf, and only its own.
    expect((await adminCall("POST", `/store/v1/admin/templates/${temp}/feature`, { featured: true })).status).toBe(200);
    expect((await json<{ items: { id: string }[] }>(await get("/templates?featured=true", a.userId))).items.map((c) => c.id)).toContain(temp);
    expect((await json<{ items: { id: string }[] }>(await get("/plugins?featured=true", a.userId))).items.map((c) => c.id)).not.toContain(temp);
    // The feed and the log say which kind.
    const hash = (view.version as { hash: string }).hash;
    expect(hash).toMatch(/^[0-9a-f]{64}$/);
    const head = (await json<{ head: number }>(await get("/takedowns"))).head;
    expect((await adminCall("POST", `/store/v1/admin/templates/${temp}/versions/1.0.0/take-down`, { reason: "Harmful." })).status).toBe(200);
    const events = await json<{ events: Record<string, unknown>[] }>(await get(`/takedowns?after=${head}`));
    expect(events.events).toEqual([expect.objectContaining({ kind: "take_down", item_kind: "template", id: temp, version: "1.0.0", hash })]);
    const audit = await json<{ entries: { item_kind: string | null; plugin_id: string | null; action: string }[] }>(await adminCall("GET", `/store/v1/admin/audit?plugin=${temp}`));
    expect(audit.entries.length).toBeGreaterThan(0);
    expect(audit.entries.every((e) => e.item_kind === "template")).toBe(true);
    // Transfer: to the new owner, under its own prefix.
    const b = await author();
    expect((await adminCall("POST", `/store/v1/admin/templates/${temp}/transfer`, { to_username: b.username })).status).toBe(200);
    expect((await json<{ templates: { id: string }[] }>(await get("/me/templates", b.userId))).templates.map((t) => t.id)).toContain(temp);
  });

  it("marks each file of every version of a template against Zeron's own at the version it was made from", async () => {
    const a = await author();
    const plug = uniqueId("p");
    const temp = uniqueId("t");
    await listedTemplate(a, uniqueId("t"), "1.0.0");
    await listed(a, plug, "1.0.0");
    // Its own Zeron's version, of this test's alone.
    const zeron = `0.3.0-${randomLetters(8)}`;
    await publishedTemplate(a, temp, "1.0.0", {
      manifest: { zeron_version: zeron },
      files: { "ds/same.luau": "return 1", "ds/changed.luau": "return 2", "DS/Spelt.luau": "return 3", "ds/added.luau": "return 4" }
    });
    const view = async (id: string, version = "1.0.0") => json<{ base?: { version: string }; files: { path: string; sha256: string; zeron: string | null }[]; zeron_base: unknown; publisher: unknown }>(await adminCall("GET", `/store/v1/admin/versions/${id}/${version}`));
    const marks = async () => Object.fromEntries((await view(temp)).files.map((f) => [f.path, f.zeron]));
    // Not known: nothing is marked, and it says so.
    const unknown = await view(temp);
    expect(unknown.zeron_base).toEqual({ version: zeron, known: false });
    expect(unknown.files.length).toBeGreaterThan(4);
    expect(unknown.files.every((f) => f.zeron === null)).toBe(true);
    const sha = (path: string) => unknown.files.find((f) => f.path === path)!.sha256;
    const put = (doc: unknown, key = zeron) => env.PLUGIN_STORE.put(`stock/${key}.json`, typeof doc === "string" ? doc : JSON.stringify(doc));
    const files = { "ds/same.luau": sha("ds/same.luau"), "ds/changed.luau": "0".repeat(64), "ds/spelt.luau": sha("DS/Spelt.luau"), "ds/left-out.luau": "1".repeat(64) };
    // Another version's, or not as the generator writes it: not known.
    await put({ version: zeron, files }, "0.3.0");
    expect((await view(temp)).zeron_base).toEqual({ version: zeron, known: false });
    for (const bad of ["{", "null", { version: "0.3.0", files }, { version: zeron }, { version: zeron, files: {} }, { version: zeron, files: [] }, { version: zeron, files: { x: "y" } }]) {
      await put(bad);
      expect((await view(temp)).zeron_base, JSON.stringify(bad)).toEqual({ version: zeron, known: false });
    }
    await put({ version: zeron, files });
    expect((await view(temp)).zeron_base).toEqual({ version: zeron, known: true });
    expect(await marks()).toMatchObject({ "ds/same.luau": "same", "ds/changed.luau": "changed", "DS/Spelt.luau": "changed", "ds/added.luau": "new", "preview.png": "new" });
    expect(Object.values(await marks())).not.toContain(null);
    // A plugin has none; its author's block counts each kind.
    const own = await view(plug);
    expect(own.zeron_base).toBeNull();
    expect(own.files.every((f) => f.zeron === null)).toBe(true);
    expect(own.publisher).toMatchObject({ plugins: { total: 1, verified: 1 }, templates: { total: 2, verified: 1 } });
    // A later version, with a base to read it against: marked too.
    expect((await adminCall("POST", `/store/v1/admin/versions/${temp}/1.0.0/verify`)).status).toBe(200);
    await publishedTemplate(a, temp, "1.1.0", { manifest: { zeron_version: zeron }, files: { "ds/same.luau": "return 1", "ds/changed.luau": "return 5" } });
    const next = await view(temp, "1.1.0");
    expect([next.base?.version, next.zeron_base]).toEqual(["1.0.0", { version: zeron, known: true }]);
    expect(Object.fromEntries(next.files.map((f) => [f.path, f.zeron]))).toMatchObject({ "ds/same.luau": "same", "ds/changed.luau": "changed", "app.luau": "new" });
  });

  it("reads a release's files once in a while, and again as soon as they're there", async () => {
    const zeron = `0.3.0-${randomLetters(8)}`;
    let reads = 0;
    const bucket = new Proxy(env.PLUGIN_STORE, {
      get: (target, prop) => {
        if (prop === "get") reads++;
        return Reflect.get(target, prop, target).bind(target);
      }
    }) as R2Bucket;
    const put = (hash: string) => env.PLUGIN_STORE.put(`stock/${zeron}.json`, JSON.stringify({ version: zeron, files: { "app.luau": hash.repeat(64) } }));
    const mark = async (now: number) => (await stockOf(bucket, zeron, now))?.("app.luau", "a".repeat(64));
    // Not there: asked each time.
    expect([await mark(0), await mark(0), reads]).toEqual([undefined, undefined, 2]);
    await put("a");
    expect([await mark(0), await mark(1), reads]).toEqual(["same", "same", 3]);
    // Past its while: read again.
    await put("b");
    expect([await mark(599_999), await mark(600_000), reads]).toEqual(["same", "changed", 4]);
    // Not a version, never a key.
    expect(await stockOf(bucket, "../0.3.0")).toBeUndefined();
    expect(reads).toBe(4);
    // Eight at most: the oldest goes.
    for (let i = 0; i < 8; i++) {
      const other = `0.3.${i}-${randomLetters(8)}`;
      await env.PLUGIN_STORE.put(`stock/${other}.json`, JSON.stringify({ version: other, files: { x: "0".repeat(64) } }));
      expect(await stockOf(bucket, other, 600_000)).toBeDefined();
    }
    reads = 0;
    expect([await mark(600_001), reads]).toEqual(["changed", 1]);
  });

  it("holds the schema to the two kinds, one namespace, and the kind of a take-down", async () => {
    const insert = (id: string, kind: string) =>
      env.STORE_DB.prepare("INSERT INTO plugins (id, kind, owner_id, owner_since, skeleton, created_at) VALUES (?, ?, 'user_x', 1, ?, 1)").bind(id, kind, `__schema:${id}`).run();
    const id = uniqueId("s");
    await insert(id, "template");
    await expect(insert(id, "plugin")).rejects.toThrow(/UNIQUE/);
    await expect(insert(uniqueId("s"), "widget")).rejects.toThrow(/CHECK/);
    await expect(env.STORE_DB.prepare("INSERT INTO takedown_events (kind, item_kind, plugin_id, version, at) VALUES ('take_down', 'widget', 'x', '1.0.0', 1)").run()).rejects.toThrow(/CHECK/);
  });
});
