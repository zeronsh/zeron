import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { PUBLISHES_PER_HOUR, handlePublish } from "../../src/store/publish";
import { ORPHAN_GRACE_MS, collectOrphans } from "../../src/store/release";
import { author, call, makePackage, member, newSigner, publish, uniqueId, adminCall } from "./store-helpers";

const json = async (res: Response) => (await res.json()) as Record<string, unknown>;

describe("POST /store/v1/plugins/:id/versions", () => {
  it("takes a signed package as a pending version, with its checks, and stores it", async () => {
    const a = await author();
    const id = uniqueId("focus");
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer });
    const res = await publish(a.userId, id, pkg);
    expect(res.status, await res.clone().text()).toBe(201);
    const body = await json(res);
    expect(body).toMatchObject({ id, version: "1.0.0", status: "pending" });
    const checks = body.checks as { id: string; level: string }[];
    expect(checks.map((c) => c.id)).toEqual(expect.arrayContaining(["signature", "manifest", "size", "version", "newer", "readme", "binary_file", "asks"]));
    expect(checks.every((c) => c.level === "ok" || c.level === "warn")).toBe(true);
    const row = await env.STORE_DB.prepare("SELECT * FROM versions WHERE plugin_id = ?").bind(id).first<Record<string, unknown>>();
    expect(row).toMatchObject({ status: "pending", publisher_id: a.userId, via: "session", signer_key: a.signer.key, package_bytes: pkg.length });
    const objects = await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` });
    expect(objects.objects).toHaveLength(1);
    expect(new Uint8Array(await (await env.PLUGIN_STORE.get(objects.objects[0]!.key))!.arrayBuffer())).toEqual(pkg);
    const files = await env.STORE_DB.prepare("SELECT path, kind FROM version_files WHERE plugin_id = ? ORDER BY path").bind(id).all();
    expect(files.results).toEqual([
      { path: "CHANGELOG.md", kind: "text" },
      { path: "README.md", kind: "text" },
      { path: "icon.svg", kind: "svg" },
      { path: "init.luau", kind: "text" },
      { path: "shots/one.png", kind: "png" }
    ]);
    // Pending: not in the catalog.
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(404);
  });

  it("answers the same bytes again as the first time, and refuses other bytes for that version", async () => {
    const a = await author();
    const id = uniqueId();
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer });
    expect((await publish(a.userId, id, pkg)).status).toBe(201);
    const again = await publish(a.userId, id, pkg);
    expect(again.status).toBe(201);
    expect(await json(again)).toMatchObject({ id, version: "1.0.0", status: "pending" });
    const other = await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer, files: { "init.luau": "return 2" } }));
    expect(other.status).toBe(409);
    expect((await json(other)).error).toBe("version_exists");
    // Build metadata doesn't make another version.
    const build = await publish(a.userId, id, await makePackage({ id, version: "1.0.0+build.7", signer: a.signer }));
    expect((await json(build)).error).toBe("version_exists");
    const count = await env.STORE_DB.prepare("SELECT COUNT(*) AS n FROM versions WHERE plugin_id = ?").bind(id).first<{ n: number }>();
    expect(count?.n).toBe(1);
    // Refused before anything was written: only the first's objects.
    expect((await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` })).objects).toHaveLength(1);
    expect((await env.PLUGIN_STORE.list({ prefix: `files/${id}/` })).objects).toHaveLength(1);
  });

  it("lets one of two publishes of the same version with different bytes win, and the other is told so", async () => {
    const a = await author();
    const id = uniqueId();
    const [one, two] = await Promise.all([
      makePackage({ id, version: "2.0.0", signer: a.signer, files: { "init.luau": "return 1" } }),
      makePackage({ id, version: "2.0.0", signer: a.signer, files: { "init.luau": "return 2" } })
    ]);
    const answers = await Promise.all([publish(a.userId, id, one), publish(a.userId, id, two)]);
    const statuses = answers.map((r) => r.status).sort();
    expect(statuses).toEqual([201, 409]);
    const row = await env.STORE_DB.prepare("SELECT package_sha256 FROM versions WHERE plugin_id = ?").bind(id).all<{ package_sha256: string }>();
    expect(row.results).toHaveLength(1);
    await collectOrphans(env.STORE_DB, env.PLUGIN_STORE, Date.now() + ORPHAN_GRACE_MS + 1);
    const pkgs = (await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` })).objects;
    expect(pkgs.map((o) => o.key)).toEqual([`pkg/${id}/2.0.0/${row.results[0]!.package_sha256}.zeron-plugin`]);
  });

  it("never deletes on a failed publish: a retry's row may yet name the same objects", async () => {
    const a = await author();
    const id = uniqueId("retry");
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer });
    // The retry: its D1 batch throws after its objects are written.
    const failing = new Proxy(env.STORE_DB, {
      get: (target, prop) => (prop === "batch" ? () => Promise.reject(new Error("D1 is down")) : Reflect.get(target, prop, target).bind?.(target) ?? Reflect.get(target, prop, target))
    }) as D1Database;
    const request = new Request(`https://edge.test/store/v1/plugins/${id}/versions`, { method: "POST", headers: { authorization: `Bearer ${a.userId}` }, body: pkg });
    await expect(handlePublish(request, env as never, failing, env.PLUGIN_STORE, id)).rejects.toThrow("D1 is down");
    const objects = (await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` })).objects;
    expect(objects).toHaveLength(1);
    const orphans = await env.STORE_DB.prepare("SELECT COUNT(*) AS n FROM object_orphans WHERE key LIKE ?").bind(`%/${id}/%`).first<{ n: number }>();
    expect(orphans?.n).toBe(2);
    // The first attempt's row lands after: the sweep keeps what it names.
    expect((await publish(a.userId, id, pkg)).status).toBe(201);
    await collectOrphans(env.STORE_DB, env.PLUGIN_STORE, Date.now() + ORPHAN_GRACE_MS + 1);
    expect((await env.PLUGIN_STORE.list({ prefix: `pkg/${id}/` })).objects.map((o) => o.key)).toEqual(objects.map((o) => o.key));
    expect(await env.PLUGIN_STORE.head(`files/${id}/1.0.0/${objects[0]!.key.split("/")[3]!.replace(".zeron-plugin", "")}.bin`)).not.toBeNull();
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/versions/1.0.0/package`)).status).toBe(200);
  });

  it("never lets the sweep take a retry's objects while its batch is on the way", async () => {
    const a = await author();
    const id = uniqueId("retry");
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer });
    const request = () => new Request(`https://edge.test/store/v1/plugins/${id}/versions`, { method: "POST", headers: { authorization: `Bearer ${a.userId}` }, body: pkg });
    const failing = new Proxy(env.STORE_DB, {
      get: (target, prop) => (prop === "batch" ? () => Promise.reject(new Error("D1 is down")) : Reflect.get(target, prop, target).bind?.(target) ?? Reflect.get(target, prop, target))
    }) as D1Database;
    // An attempt failed and left its objects, recorded, an hour and more ago.
    await expect(handlePublish(request(), env as never, failing, env.PLUGIN_STORE, id)).rejects.toThrow("D1 is down");
    await env.STORE_DB.prepare("UPDATE object_orphans SET at = ? WHERE key LIKE ?").bind(Date.now() - 2 * ORPHAN_GRACE_MS, `%/${id}/%`).run();
    // Its retry: the hourly sweep runs after its objects are written and
    // before its batch lands.
    let swept = -1;
    const slow = new Proxy(env.STORE_DB, {
      get: (target, prop) =>
        prop === "batch"
          ? async (statements: D1PreparedStatement[]) => {
              swept = await collectOrphans(env.STORE_DB, env.PLUGIN_STORE, Date.now());
              return target.batch(statements);
            }
          : Reflect.get(target, prop, target).bind?.(target) ?? Reflect.get(target, prop, target)
    }) as D1Database;
    expect((await handlePublish(request(), env as never, slow, env.PLUGIN_STORE, id)).status).toBe(201);
    expect(swept).toBe(0);
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/versions/1.0.0/package`)).status).toBe(200);
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/files/1.0.0/init.luau`)).status).toBe(200);
    // Named by its row, the records are gone: no later sweep looks at them.
    expect(await env.STORE_DB.prepare("SELECT COUNT(*) AS n FROM object_orphans WHERE key LIKE ?").bind(`%/${id}/%`).first("n")).toBe(0);
  });

  it("writes back an object a sweep deleted after the retry wrote it and before its row landed", async () => {
    const a = await author();
    const id = uniqueId("retry");
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer });
    const request = () => new Request(`https://edge.test/store/v1/plugins/${id}/versions`, { method: "POST", headers: { authorization: `Bearer ${a.userId}` }, body: pkg });
    const failing = new Proxy(env.STORE_DB, {
      get: (target, prop) => (prop === "batch" ? () => Promise.reject(new Error("D1 is down")) : Reflect.get(target, prop, target).bind?.(target) ?? Reflect.get(target, prop, target))
    }) as D1Database;
    // An earlier attempt left its objects, recorded more than an hour ago.
    await expect(handlePublish(request(), env as never, failing, env.PLUGIN_STORE, id)).rejects.toThrow("D1 is down");
    await env.STORE_DB.prepare("UPDATE object_orphans SET at = ? WHERE key LIKE ?").bind(Date.now() - 2 * ORPHAN_GRACE_MS, `%/${id}/%`).run();
    // The sweep claims a record, finds no row and no fresh record, and is
    // about to delete: held there.
    let reached!: () => void;
    const atDelete = new Promise<void>((resolve) => (reached = resolve));
    let release!: () => void;
    const go = new Promise<void>((resolve) => (release = resolve));
    const sweepBucket = new Proxy(env.PLUGIN_STORE, {
      get: (target, prop) =>
        prop === "delete"
          ? async (key: string) => {
              reached();
              await go;
              return target.delete(key);
            }
          : Reflect.get(target, prop, target).bind?.(target) ?? Reflect.get(target, prop, target)
    }) as R2Bucket;
    const sweep = collectOrphans(env.STORE_DB, sweepBucket, Date.now());
    await atDelete;
    // The retry: it refreshes the records and writes the objects; the
    // sweep's delete lands then, before the retry's batch.
    const slow = new Proxy(env.STORE_DB, {
      get: (target, prop) =>
        prop === "batch"
          ? async (statements: D1PreparedStatement[]) => {
              release();
              expect(await sweep).toBe(1);
              return target.batch(statements);
            }
          : Reflect.get(target, prop, target).bind?.(target) ?? Reflect.get(target, prop, target)
    }) as D1Database;
    expect((await handlePublish(request(), env as never, slow, env.PLUGIN_STORE, id)).status).toBe(201);
    // Both objects are there and served.
    const sha = (await env.STORE_DB.prepare("SELECT package_sha256 FROM versions WHERE plugin_id = ?").bind(id).first<string>("package_sha256"))!;
    expect(await env.PLUGIN_STORE.head(`pkg/${id}/1.0.0/${sha}.zeron-plugin`)).not.toBeNull();
    expect(await env.PLUGIN_STORE.head(`files/${id}/1.0.0/${sha}.bin`)).not.toBeNull();
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/versions/1.0.0/package`)).status).toBe(200);
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/files/1.0.0/init.luau`)).status).toBe(200);
  });

  it("keeps an orphan record for its grace, and deletes what no row names after it", async () => {
    const key = `pkg/${uniqueId("gone")}/1.0.0/${"0".repeat(64)}.zeron-plugin`;
    await env.PLUGIN_STORE.put(key, "x");
    await env.STORE_DB.prepare("INSERT INTO object_orphans (key, at) VALUES (?, ?)").bind(key, Date.now()).run();
    expect(await collectOrphans(env.STORE_DB, env.PLUGIN_STORE, Date.now())).toBe(0);
    expect(await env.PLUGIN_STORE.head(key)).not.toBeNull();
    await collectOrphans(env.STORE_DB, env.PLUGIN_STORE, Date.now() + ORPHAN_GRACE_MS + 1);
    expect(await env.PLUGIN_STORE.head(key)).toBeNull();
  });

  it("lets two publishes of the same bytes at once both succeed, and neither deletes what the other stored", async () => {
    const a = await author();
    const id = uniqueId();
    const pkg = await makePackage({ id, version: "1.0.0", signer: a.signer });
    const answers = await Promise.all([publish(a.userId, id, pkg), publish(a.userId, id, pkg), publish(a.userId, id, pkg)]);
    expect(answers.map((r) => r.status)).toEqual([201, 201, 201]);
    for (const r of answers) expect(await r.json()).toMatchObject({ id, version: "1.0.0", status: "pending" });
    const rows = await env.STORE_DB.prepare("SELECT package_sha256 FROM versions WHERE plugin_id = ?").bind(id).all<{ package_sha256: string }>();
    expect(rows.results).toHaveLength(1);
    const sha = rows.results[0]!.package_sha256;
    // Both content-addressed objects are there, and are these bytes.
    const stored = await env.PLUGIN_STORE.get(`pkg/${id}/1.0.0/${sha}.zeron-plugin`);
    expect(new Uint8Array(await stored!.arrayBuffer())).toEqual(pkg);
    expect(await env.PLUGIN_STORE.head(`files/${id}/1.0.0/${sha}.bin`)).not.toBeNull();
    // And the version reads back: its files are served from that object.
    expect((await call(a.userId, "GET", `/store/v1/plugins/${id}/files/1.0.0/init.luau`)).status).toBe(200);
  });

  it("gives a new id to the first account and refuses it to anyone else", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId();
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const res = await publish(b.userId, id, await makePackage({ id, version: "1.0.1", signer: b.signer }));
    expect(res.status).toBe(403);
    expect((await json(res)).error).toBe("id_taken");
  });

  it("refuses reserved ids and ids that look like one already taken", async () => {
    const a = await author();
    for (const id of ["git", "github", "parallax-scenes", "github-review", "zer0n-tools", "admin-panel", "openai-chat"]) {
      const res = await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }));
      expect(res.status, id).toBe(403);
      expect((await json(res)).error, id).toBe("id_reserved");
    }
    const id = uniqueId("lookalike");
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const b = await author();
    const twin = id.replace("lookalike", "look-alike");
    const res = await publish(b.userId, twin, await makePackage({ id: twin, version: "1.0.0", signer: b.signer }));
    expect(res.status).toBe(403);
    expect((await json(res)).error).toBe("id_reserved");
  });

  it("lets an admin keep an id back, or give a reserved one to one account", async () => {
    const a = await author();
    const b = await author();
    const kept = uniqueId("kept");
    expect((await adminCall("POST", "/store/v1/admin/reservations", { id: kept, note: "for a partner" })).status).toBe(200);
    const refused = await publish(a.userId, kept, await makePackage({ id: kept, version: "1.0.0", signer: a.signer }));
    expect((await json(refused)).error).toBe("id_reserved");
    const given = "github-helper";
    expect((await adminCall("POST", "/store/v1/admin/reservations", { id: given, to_username: b.username })).status).toBe(200);
    expect((await publish(a.userId, given, await makePackage({ id: given, version: "1.0.0", signer: a.signer }))).status).toBe(403);
    expect((await publish(b.userId, given, await makePackage({ id: given, version: "1.0.0", signer: b.signer }))).status).toBe(201);
  });

  it("needs a username, a sign-in, and a key of the account's", async () => {
    const signer = await newSigner();
    const nameless = await member();
    const id = uniqueId();
    const pkg = await makePackage({ id, version: "1.0.0", signer });
    const r1 = await publish(nameless, id, pkg);
    expect(r1.status).toBe(403);
    expect((await json(r1)).error).toBe("needs_username");
    expect((await call(undefined, "POST", `/store/v1/plugins/${id}/versions`, pkg)).status).toBe(401);
    const a = await author();
    const r2 = await publish(a.userId, id, pkg); // signed by a key not theirs
    expect(r2.status).toBe(403);
    expect((await json(r2)).error).toBe("key_not_allowed");
    // A runner (a Cloud machine) never publishes.
    expect((await publish(`runner:${a.userId}@org_1:dev1`, id, pkg)).status).toBe(403);
  });

  it("refuses what a device would refuse, with the engine's reason", async () => {
    const a = await author();
    const id = uniqueId();
    const good = await makePackage({ id, version: "1.0.0", signer: a.signer });
    const text = new TextDecoder().decode(good);
    const tampered = new TextEncoder().encode(text.replace(/"init\.luau":"[^"]+"/, `"init.luau":"${btoa("return 666")}"`));
    const cases: [Uint8Array, number, string, string?][] = [
      [new TextEncoder().encode("not json"), 400, "bad_package", "not_a_package"],
      [tampered, 400, "bad_signature", "tampered"],
      [await makePackage({ id, version: "1.0.0", signer: a.signer, manifest: { permissions: ["root"] } }), 400, "bad_package", "bad_manifest"],
      [await makePackage({ id, version: "1.0.0", signer: a.signer, files: { "../evil": "x" } }), 400, "bad_package", "bad_file"],
      [await makePackage({ id, version: "1.0.0", signer: a.signer, manifest: { entry: "missing.luau" } }), 400, "bad_package", "missing_entry"],
      [await makePackage({ id: `${id}-x`, version: "1.0.0", signer: a.signer }), 400, "bad_package", "id_mismatch"]
    ];
    for (const [pkg, status, error, reason] of cases) {
      const res = await publish(a.userId, id, pkg);
      const body = await json(res);
      expect([res.status, body.error, body.reason], reason).toEqual([status, error, reason]);
    }
  });

  it("refuses what the store needs and a device doesn't", async () => {
    const a = await author();
    const id = uniqueId();
    const cases: [Record<string, unknown>, Record<string, string | undefined>, string][] = [
      [{ version: "1.0" }, {}, "not_semver"],
      [{ version: "01.0.0" }, {}, "not_semver"],
      [{ version: "9007199254740992.0.0" }, {}, "not_semver"],
      [{}, { "README.md": undefined }, "missing_readme"],
      [{ description: undefined }, {}, "missing_listing"],
      [{ icon: undefined }, {}, "missing_listing"],
      [{ icon: "init.luau" }, {}, "bad_listing"],
      [{ categories: [] }, {}, "missing_listing"],
      [{ categories: ["games"] }, {}, "bad_listing"],
      [{ categories: ["git", "chat", "agents"] }, {}, "bad_listing"],
      [{ keywords: ["Upper"] }, {}, "bad_listing"],
      [{ screenshots: ["init.luau"] }, {}, "bad_listing"],
      [{ homepage: "http://example.com" }, {}, "bad_listing"],
      [{ engines: { zeron: ">= 0.4" } }, {}, "bad_listing"],
      [{ description: "x".repeat(301) }, {}, "bad_listing"]
    ];
    for (const [manifest, files, error] of cases) {
      const res = await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer, manifest, files }));
      expect([res.status, (await json(res)).error], JSON.stringify(manifest) + JSON.stringify(files)).toEqual([400, error]);
    }
  });

  it("refuses a version not later than those before it; a release only follows releases", async () => {
    const a = await author();
    const id = uniqueId();
    const go = async (version: string) => publish(a.userId, id, await makePackage({ id, version, signer: a.signer }));
    expect((await go("2.0.0")).status).toBe(201);
    const older = await go("1.9.9");
    expect([older.status, (await json(older)).error]).toEqual([409, "not_newer"]);
    expect((await go("2.1.0-beta.1")).status).toBe(201);
    // A release while a later beta is out: it follows the releases.
    expect((await go("2.0.1")).status).toBe(201);
    // A pre-release follows everything.
    expect((await json(await go("2.0.2-rc.1"))).error).toBe("not_newer");
    expect((await go("2.1.0-beta.2")).status).toBe(201);
    // A rejected version doesn't stand in the way, but its number stays taken.
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/2.1.0-beta.2/reject`, { reason: "broken", message: "It crashes." })).status).toBe(200);
    const same = await go("2.1.0-beta.2");
    expect([same.status, (await json(same)).status]).toEqual([201, "rejected"]);
    const changed = await publish(a.userId, id, await makePackage({ id, version: "2.1.0-beta.2", signer: a.signer, files: { "init.luau": "return 3" } }));
    expect((await json(changed)).error).toBe("version_exists");
    expect((await go("2.1.0-beta.3")).status).toBe(201);
  });

  it("refuses an upload past the store's limit before reading it all", async () => {
    const a = await author();
    const id = uniqueId();
    const res = await call(a.userId, "POST", `/store/v1/plugins/${id}/versions`, new Uint8Array(21 * 1024 * 1024));
    expect([res.status, (await json(res)).error]).toEqual([413, "too_large"]);
  });

  it("limits how many versions an account submits an hour", async () => {
    const a = await author();
    const id = uniqueId();
    for (let i = 0; i < PUBLISHES_PER_HOUR; i++) {
      expect((await publish(a.userId, id, await makePackage({ id, version: `1.0.${i}`, signer: a.signer }))).status).toBe(201);
    }
    const res = await publish(a.userId, id, await makePackage({ id, version: "1.1.0", signer: a.signer }));
    expect([res.status, (await json(res)).error]).toEqual([429, "rate_limited"]);
  });

  it("takes nothing new for a suspended plugin", async () => {
    const a = await author();
    const id = uniqueId();
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/suspend`, { reason: "Malware." })).status).toBe(200);
    const res = await publish(a.userId, id, await makePackage({ id, version: "1.0.1", signer: a.signer }));
    expect([res.status, (await json(res)).error]).toEqual([403, "suspended"]);
  });
});
