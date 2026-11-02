import { SELF, env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { CARD_SCENES, cardHash, deriveCard } from "../../src/card";
import { UserDirectory, signWorkOsPayload } from "../../src/user-directory";

const SECRET = "workos_whsec_test";
const JOINED = "2026-10-01T10:00:00.000Z";

const user = (id: string, overrides: Record<string, unknown> = {}) => ({
  object: "user",
  id,
  email: `${id}@example.com`,
  first_name: "Ada",
  last_name: "Lovelace",
  profile_picture_url: "https://workoscdn.com/images/v1/abc",
  email_verified: true,
  created_at: JOINED,
  updated_at: JOINED,
  ...overrides
});

const webhook = async (event: string, data: Record<string, unknown>) => {
  const payload = JSON.stringify({ id: `event_${crypto.randomUUID()}`, event, data, created_at: new Date().toISOString() });
  const t = Date.now();
  const v1 = await signWorkOsPayload(SECRET, t, payload);
  return SELF.fetch("https://edge.test/workos/webhook", {
    method: "POST",
    headers: { "content-type": "application/json", "workos-signature": `t=${t}, v1=${v1}` },
    body: payload
  });
};

const signUp = async (...ids: string[]) => {
  for (const id of ids) await webhook("user.created", user(id));
};

/** Dev auth: the bearer is the user id. */
const claim = (userId: string, username: unknown, extra: Record<string, unknown> = {}) =>
  SELF.fetch("https://edge.test/username", {
    method: "POST",
    headers: { authorization: `Bearer ${userId}`, "content-type": "application/json" },
    body: JSON.stringify({ username, ...extra })
  });

const check = async (userId: string, username: string) =>
  (await (
    await SELF.fetch(`https://edge.test/username/check?username=${encodeURIComponent(username)}`, {
      headers: { authorization: `Bearer ${userId}` }
    })
  ).json()) as { available: boolean; reason?: string };

const admin = (path: string, body?: unknown, token = "test-admin-token") =>
  SELF.fetch(`https://edge.test${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body)
  });

const readCard = (username: string) => SELF.fetch(`https://edge.test/cards/${username}`);

const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));

/** An id whose scene differs between `from` (today's count by default) and
 * `scenes` scenes. */
const idMovedBy = (scenes: number, prefix: string, from = CARD_SCENES): string => {
  for (let i = 0; ; i++) {
    const id = `${prefix}${i}`;
    if (cardHash(`${id}:scene`) % from !== cardHash(`${id}:scene`) % scenes) return id;
  }
};

describe("claiming a username", () => {
  it("gives a signed-in user a name and a card from their id", async () => {
    await signUp("user_claim1");
    expect(await check("user_claim1", "firstclaim")).toEqual({ available: true });
    const res = await claim("user_claim1", "FirstClaim");
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({ username: "firstclaim", ...deriveCard("user_claim1", CARD_SCENES) });
    const mine = await SELF.fetch("https://edge.test/username", { headers: { authorization: "Bearer user_claim1" } });
    expect(await mine.json()).toEqual({ username: "firstclaim" });
    expect(await check("user_claim1", "firstclaim")).toEqual({ available: false, reason: "taken" });
  });

  it("requires a bearer", async () => {
    const res = await SELF.fetch("https://edge.test/username", { method: "POST", body: '{"username":"nobearer"}' });
    expect(res.status).toBe(401);
    expect((await SELF.fetch("https://edge.test/username/check?username=nobearer")).status).toBe(401);
  });

  it("claims for the token's user whatever the body says", async () => {
    await signUp("user_self", "user_victim");
    await claim("user_self", "selfclaim", { userId: "user_victim", user_id: "user_victim" });
    const username = async (id: string) =>
      (await (await SELF.fetch("https://edge.test/username", { headers: { authorization: `Bearer ${id}` } })).json()) as { username: string | null };
    expect(await username("user_self")).toEqual({ username: "selfclaim" });
    expect(await username("user_victim")).toEqual({ username: null });
  });

  it("refuses a Cloud runner's token", async () => {
    await signUp("user_runner");
    const res = await SELF.fetch("https://edge.test/username", {
      method: "POST",
      headers: { authorization: "Bearer runner:user_runner@org_runner:cloud-1" },
      body: '{"username":"runnerclaim"}'
    });
    expect(res.status).toBe(403);
    expect(await check("user_runner", "runnerclaim")).toEqual({ available: true });
  });

  it("is one claim: the same again succeeds, another is refused", async () => {
    await signUp("user_once");
    expect((await claim("user_once", "onceonly")).status).toBe(200);
    expect((await claim("user_once", "ONCEONLY")).status).toBe(200);
    const again = await claim("user_once", "secondname");
    expect(again.status).toBe(409);
    expect(await again.json()).toMatchObject({ error: "already_claimed" });
    expect((await readCard("secondname")).status).toBe(404);
  });

  it("treats names differing in case as one", async () => {
    await signUp("user_case1", "user_case2");
    expect((await claim("user_case1", "CaseFold")).status).toBe(200);
    const res = await claim("user_case2", "cAsEfOlD");
    expect(res.status).toBe(409);
    expect(await res.json()).toMatchObject({ error: "username_taken" });
    expect(await check("user_case2", "CASEFOLD")).toEqual({ available: false, reason: "taken" });
    expect((await readCard("CASEFOLD")).status).toBe(200);
  });

  it("refuses bad and reserved names", async () => {
    await signUp("user_rules");
    expect((await claim("user_rules", "no")).status).toBe(400);
    expect((await claim("user_rules", "has space")).status).toBe(400);
    expect((await claim("user_rules", 7)).status).toBe(400);
    for (const reserved of ["admin", "API", "support", "zeron", "zeron-team"]) {
      const res = await claim("user_rules", reserved);
      expect(res.status).toBe(403);
      expect(await res.json()).toMatchObject({ error: "reserved_username" });
    }
    expect(await check("user_rules", "no")).toEqual({ available: false, reason: "invalid" });
    expect(await check("user_rules", "admin")).toEqual({ available: false, reason: "reserved" });
  });

  it("waits for the directory, and refuses a deleted account", async () => {
    const early = await claim("user_nowebhook", "toosoon");
    expect(early.status).toBe(409);
    expect(await early.json()).toMatchObject({ error: "not_ready" });

    await signUp("user_deleted_claim");
    await webhook("user.deleted", user("user_deleted_claim"));
    expect((await claim("user_deleted_claim", "ghostclaim")).status).toBe(403);
  });

  it("lets one of two simultaneous claims win", async () => {
    const ids = Array.from({ length: 8 }, (_, i) => `user_race${i}`);
    await signUp(...ids);
    const results = await Promise.all(ids.map((id) => claim(id, "racedname")));
    expect(results.map((r) => r.status).sort()).toEqual([200, ...ids.slice(1).map(() => 409)]);
    const held = (await (await admin("/admin/users")).json()) as { users: { username: string | null }[] };
    expect(held.users.filter((u) => u.username === "racedname")).toHaveLength(1);
  });

  it("gives one user one name however their claims race", async () => {
    await signUp("user_race_self");
    const names = ["racea", "raceb", "racec", "raced"];
    const results = await Promise.all(names.map((name) => claim("user_race_self", name)));
    expect(results.filter((r) => r.status === 200)).toHaveLength(1);
    expect(results.filter((r) => r.status === 409)).toHaveLength(3);
  });

  it("holds uniqueness in the database itself", async () => {
    await signUp("user_db1", "user_db2");
    await claim("user_db1", "dbheld");
    await runInDurableObject(directory(), (_instance: UserDirectory, state) => {
      const insert = (username: string, userId: string) =>
        state.storage.sql.exec("INSERT INTO usernames (username, user_id, claimed_at) VALUES (?, ?, 1)", username, userId);
      expect(() => insert("dbheld", "user_db2")).toThrow(/UNIQUE/);
      // A user's second live name, and names outside the rules.
      expect(() => insert("dbsecond", "user_db1")).toThrow(/UNIQUE/);
      expect(() => insert("DBHELD", "user_db2")).toThrow(/CHECK/);
      expect(() => insert("-dash", "user_db2")).toThrow(/CHECK/);
      expect(() => insert("no", "user_db2")).toThrow(/CHECK/);
      expect(() => insert("has space", "user_db2")).toThrow(/CHECK/);
    });
  });

  it("limits attempts per user", async () => {
    await signUp("user_limited", "user_unlimited");
    const answers = [];
    for (let i = 0; i < 61; i++) answers.push(await check("user_limited", "limitedname"));
    expect(answers[59]).toEqual({ available: true });
    const res = await SELF.fetch("https://edge.test/username/check?username=limitedname", { headers: { authorization: "Bearer user_limited" } });
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("60");
    const blocked = await claim("user_limited", "limitedname");
    expect(blocked.status).toBe(429);
    // Nobody else is held up.
    expect((await claim("user_unlimited", "limitedname")).status).toBe(200);
  });
});

describe("deleted users", () => {
  it("takes the card down and retires the name for good", async () => {
    await signUp("user_retired", "user_heir");
    await claim("user_retired", "retiredname");
    expect((await readCard("retiredname")).status).toBe(200);
    await webhook("user.deleted", user("user_retired"));
    expect((await readCard("retiredname")).status).toBe(404);

    expect(await check("user_heir", "retiredname")).toEqual({ available: false, reason: "taken" });
    expect((await claim("user_heir", "RetiredName")).status).toBe(409);
    const taken = await admin("/admin/usernames/set", { userId: "user_heir", username: "retiredname" });
    expect(taken.status).toBe(409);
    expect(await taken.json()).toMatchObject({ error: "username_retired" });

    // A late profile update changes nothing.
    await webhook("user.updated", user("user_retired", { updated_at: "2026-10-09T00:00:00.000Z" }));
    expect((await readCard("retiredname")).status).toBe(404);
    // Nor can an operator put a name on a deleted user.
    expect((await admin("/admin/usernames/set", { userId: "user_retired", username: "otherghost" })).status).toBe(409);
  });
});

describe("admin usernames", () => {
  it("needs the admin token", async () => {
    expect((await admin("/admin/usernames/set", { userId: "user_x", username: "whoever" }, "nope")).status).toBe(401);
    expect((await admin("/admin/usernames/clear", { userId: "user_x" }, "user_x")).status).toBe(401);
    expect((await admin("/admin/usernames/other", {})).status).toBe(404);
  });

  it("validates its input", async () => {
    expect((await admin("/admin/usernames/set", { userId: "nope", username: "fine" })).status).toBe(400);
    expect((await admin("/admin/usernames/set", { userId: "user_x", username: "x" })).status).toBe(400);
    expect((await admin("/admin/usernames/set", { userId: "user_nobody", username: "nobodyhere" })).status).toBe(404);
  });

  it("sets a name, reserved ones included, and shows it in the directory", async () => {
    await signUp("user_house");
    const res = await admin("/admin/usernames/set", { userId: "user_house", username: "Zeron" });
    expect(await res.json()).toMatchObject({ username: "zeron", ...deriveCard("user_house", CARD_SCENES) });
    const listed = (await (await admin("/admin/users")).json()) as { users: { userId: string; username: string | null }[] };
    expect(listed.users.find((u) => u.userId === "user_house")?.username).toBe("zeron");
    const csv = await (await admin("/admin/users?format=csv")).text();
    expect(csv).toMatch(/\n\d+,user_house,.*,zeron,\n/);
  });

  it("won't take a name from someone holding it", async () => {
    await signUp("user_holder", "user_taker");
    await claim("user_holder", "heldname");
    const res = await admin("/admin/usernames/set", { userId: "user_taker", username: "heldname" });
    expect(res.status).toBe(409);
    expect(await res.json()).toMatchObject({ error: "username_taken" });
  });

  it("renames a user: the old name stays retired, and theirs to get back", async () => {
    await signUp("user_rename", "user_other");
    await claim("user_rename", "typoname");
    await admin("/admin/usernames/set", { userId: "user_rename", username: "goodname" });
    expect((await readCard("typoname")).status).toBe(404);
    expect((await readCard("goodname")).status).toBe(200);
    expect((await claim("user_other", "typoname")).status).toBe(409);
    expect((await admin("/admin/usernames/set", { userId: "user_rename", username: "typoname" })).status).toBe(200);
    expect((await readCard("typoname")).status).toBe(200);
    expect((await readCard("goodname")).status).toBe(404);
  });

  it("clears a name: retired by default, freed on request, and the user can claim anew", async () => {
    await signUp("user_cleared", "user_next");
    await claim("user_cleared", "clearedname");
    expect(await (await admin("/admin/usernames/clear", { userId: "user_cleared" })).json()).toEqual({ cleared: "clearedname" });
    expect(await (await admin("/admin/usernames/clear", { userId: "user_cleared" })).json()).toEqual({ cleared: null });
    expect((await readCard("clearedname")).status).toBe(404);
    expect((await claim("user_next", "clearedname")).status).toBe(409);
    expect((await claim("user_cleared", "clearedname")).status).toBe(409);

    expect((await claim("user_cleared", "squatname")).status).toBe(200);
    await admin("/admin/usernames/clear", { userId: "user_cleared", release: true });
    expect((await claim("user_next", "squatname")).status).toBe(200);
  });
});

describe("the public card", () => {
  it("exposes exactly the card's fields", async () => {
    await webhook("user.created", user("user_public", { first_name: "Grace", last_name: "Hopper" }));
    await admin("/admin/badges", { id: "public-badge", name: "Public Badge", description: "Internal text", icon: "star" });
    await admin("/admin/badges", { id: "plain-badge", name: "Plain Badge" });
    await admin("/admin/badges/grant", { badgeId: "public-badge", userIds: ["user_public"], note: "private operator note" });
    await claim("user_public", "gracehopper");
    await admin("/admin/badges/grant", { badgeId: "plain-badge", userIds: ["user_public"] });
    const number = ((await (await admin("/admin/users")).json()) as { users: { userId: string; userNumber: number }[] }).users.find(
      (u) => u.userId === "user_public"
    )?.userNumber;

    const res = await readCard("gracehopper");
    expect(res.status).toBe(200);
    expect(res.headers.get("access-control-allow-origin")).toBe("*");
    expect(res.headers.get("cache-control")).toBe("public, max-age=60");
    const text = await res.text();
    expect(JSON.parse(text)).toEqual({
      username: "gracehopper",
      firstName: "Grace",
      userNumber: number,
      joinedOn: "2026-10-01",
      badges: [
        { id: "public-badge", name: "Public Badge", icon: "star" },
        { id: "plain-badge", name: "Plain Badge", icon: null }
      ],
      ...deriveCard("user_public", CARD_SCENES)
    });
    for (const secret of ["user_public", "example.com", "workoscdn", "operator note", "Internal text", "org_", "Hopper", "T10:"]) {
      expect(text).not.toContain(secret);
    }
  });

  /** The whole public contract. A field added to a card fails this test: add
   * it here only once it's meant to be public. */
  const PUBLIC_FIELDS = ["username", "firstName", "userNumber", "joinedOn", "badges", "scene", "holo", "sky"];
  const BADGE_FIELDS = ["id", "name", "icon"];

  it("has exactly the public fields, in order, wherever a card is served", async () => {
    await webhook("user.created", user("user_fields", { first_name: "Ada Lovelace", last_name: "Lovelace" }));
    await admin("/admin/badges", { id: "fields-badge", name: "Fields", description: "secret", icon: "star" });
    await admin("/admin/badges/grant", { badgeId: "fields-badge", userIds: ["user_fields"], note: "secret note" });
    await claim("user_fields", "fieldsholder");

    const fresh = await directory().card("fieldsholder");
    // Twice: the first read fills the edge cache, the second is served from it.
    const first = await (await readCard("fieldsholder")).json();
    const cached = await (await readCard("fieldsholder")).json();
    const html = await (await SELF.fetch("https://edge.test/@fieldsholder")).text();
    const page = JSON.parse(/id="card">(.*?)<\/script>/s.exec(html)![1]!);
    for (const card of [fresh, first, cached, page] as Record<string, unknown>[]) {
      expect(Object.keys(card)).toEqual(PUBLIC_FIELDS);
      for (const badge of card.badges as Record<string, unknown>[]) expect(Object.keys(badge)).toEqual(BADGE_FIELDS);
      expect(card).toMatchObject({ firstName: "Ada", joinedOn: "2026-10-01" });
      expect(card.joinedOn).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      expect(typeof card.sky).toBe("number");
    }
  });

  it("answers every name that isn't a live card the same way", async () => {
    await signUp("user_gone404", "user_cleared404", "user_released404", "user_renamed404");
    await claim("user_gone404", "gone404");
    await webhook("user.deleted", user("user_gone404"));
    await claim("user_cleared404", "cleared404");
    await admin("/admin/usernames/clear", { userId: "user_cleared404" });
    await claim("user_released404", "released404");
    await admin("/admin/usernames/clear", { userId: "user_released404", release: true });
    await claim("user_renamed404", "renamed404");
    await admin("/admin/usernames/set", { userId: "user_renamed404", username: "renamed404b" });

    const answer = async (path: string) => {
      const res = await SELF.fetch(`https://edge.test${path}`);
      return {
        status: res.status,
        headers: [...res.headers].filter(([k]) => k !== "date"),
        body: await res.text()
      };
    };
    for (const prefix of ["/cards/", "/@"]) {
      const names = ["neverclaimed404", "gone404", "cleared404", "released404", "renamed404", "x", "bad%20name"];
      // Uncached and cached alike.
      for (let pass = 0; pass < 2; pass++) {
        const answers = await Promise.all(names.map((n) => answer(`${prefix}${n}`)));
        expect(answers[0]!.status).toBe(404);
        for (const other of answers.slice(1)) expect(other).toEqual(answers[0]);
      }
    }
  });

  it("limits reads per client IP, cached or not, and per IP only", async () => {
    await signUp("user_limit_read");
    await claim("user_limit_read", "limitread");
    const read = (ip: string, path = "/cards/limitread") =>
      SELF.fetch(`https://edge.test${path}`, { headers: { "cf-connecting-ip": ip } });
    const statuses = [];
    for (let i = 0; i < 60; i++) statuses.push((await read("203.0.113.7", i % 2 ? "/cards/limitread" : `/@probe${i}`)).status);
    expect(statuses.filter((s) => s === 429)).toEqual([]);
    const blocked = await read("203.0.113.7");
    expect(blocked.status).toBe(429);
    expect(blocked.headers.get("retry-after")).toBe("60");
    expect(blocked.headers.get("cache-control")).toBe("no-store");
    expect(await blocked.json()).toEqual({ error: "rate_limited" });
    expect((await read("203.0.113.7", "/@limitread/player")).status).toBe(429);
    expect((await read("198.51.100.9")).status).toBe(200);
  });

  it("answers 404 for a name no one holds, and for one that can't be", async () => {
    for (const path of ["/cards/nobodyatall", "/cards/x", "/cards/a%20b", "/cards/", "/cards/a/b"]) {
      const res = await SELF.fetch(`https://edge.test${path}`);
      expect(res.status).toBe(404);
      expect(await res.json()).toMatchObject({ error: "not_found" });
    }
  });

  it("shows a claim made just after a cached miss", async () => {
    await signUp("user_cached");
    expect((await readCard("cachedmiss")).status).toBe(404);
    await claim("user_cached", "cachedmiss");
    expect((await readCard("cachedmiss")).status).toBe(200);
  });

  it("is read-only", async () => {
    const res = await SELF.fetch("https://edge.test/cards/whoever", { method: "POST" });
    expect(res.status).toBe(405);
  });
});

describe("the card's page", () => {
  it("serves /c/username and /@username, the raw and the percent-encoded @", async () => {
    await signUp("user_page");
    await claim("user_page", "pageholder");
    for (const path of ["/c/pageholder", "/c/PageHolder", "/@pageholder", "/%40pageholder", "/@PageHolder", "/@pageholder/"]) {
      const res = await SELF.fetch(`https://edge.test${path}`);
      expect(res.status).toBe(200);
      expect(res.headers.get("content-type")).toContain("text/html");
      expect(await res.text()).toContain('"username":"pageholder"');
    }
  });

  it("names /c/username as the card's address, whichever way it was reached", async () => {
    await signUp("user_c");
    await claim("user_c", "caddress");
    for (const path of ["/c/caddress", "/@caddress"]) {
      const html = await (await SELF.fetch(`https://edge.test${path}`)).text();
      expect(html).toContain('<link rel="canonical" href="https://edge.test/c/caddress" />');
      expect(html).toContain('<meta name="twitter:player" content="https://edge.test/c/caddress/player" />');
      expect(html).toContain('<meta name="twitter:image" content="https://edge.test/c/caddress/card.png" />');
    }
    expect((await SELF.fetch("https://edge.test/c/caddress/player")).status).toBe(200);
  });

  it("keeps the fields a card page reads in its data block, nothing more", async () => {
    await signUp("user_seam");
    await claim("user_seam", "seamholder");
    const html = await (await SELF.fetch("https://edge.test/@seamholder")).text();
    const data = /<script type="application\/json" id="card">(.*?)<\/script>/s.exec(html)?.[1];
    expect(JSON.parse(data!)).toMatchObject({ username: "seamholder", ...deriveCard("user_seam", CARD_SCENES) });
    expect(html).not.toContain("user_seam");
    expect(html).not.toContain("example.com");
  });

  it("escapes a member's name and allows no script", async () => {
    await webhook("user.created", user("user_xss", { first_name: "<script>alert(1)</script>", last_name: "</script>" }));
    await claim("user_xss", "xssholder");
    const res = await SELF.fetch("https://edge.test/@xssholder");
    const html = await res.text();
    expect(html).not.toContain("<script>alert");
    // The page's own scripts and no other: the data block and the page's
    // module (apps/card-web/site/pages.mjs); the name can't add one.
    expect(html.match(/<script/g)).toHaveLength(2);
    expect(html.match(/<\/script>/g)).toHaveLength(2);
    expect(html).toContain('<script type="module" src="https://edge.test/card/card-page.js"></script>');
    expect(res.headers.get("content-security-policy")).toContain("default-src 'none'");
    expect(res.headers.get("content-security-policy")).toContain("script-src 'self'");
    const data = /id="card">(.*?)<\/script>/s.exec(html)?.[1];
    expect(JSON.parse(data!).firstName).toBe("<script>alert(1)</script>");
  });

  it("answers 404 for what isn't a card, without a bearer", async () => {
    for (const path of ["/@", "/@nobodyatall", "/@no", "/@pageholder/extra", "/%40", "/c/", "/c/nobodyatall", "/c/pageholder/extra"]) {
      expect((await SELF.fetch(`https://edge.test${path}`)).status).toBe(404);
    }
    expect((await SELF.fetch("https://edge.test/@whoever", { method: "POST" })).status).toBe(405);
  });
});

describe("a card over time", () => {
  it("stays the same when scenes are added", async () => {
    const early = idMovedBy(CARD_SCENES + 4, "user_early");
    const late = idMovedBy(CARD_SCENES + 4, "user_late");
    await signUp(early, late);
    await claim(early, "earlymember");
    const before = await (await readCard("earlymember")).json();
    expect(before).toMatchObject(deriveCard(early, CARD_SCENES));

    // The manifest grows: new claims see more scenes, the old card doesn't.
    const grown = CARD_SCENES + 4;
    expect(await directory().claimUsername(late, "latemember", grown)).toMatchObject({ card: { scene: deriveCard(late, grown).scene } });
    expect(deriveCard(late, grown).scene).not.toBe(deriveCard(late, CARD_SCENES).scene);
    expect(await directory().card("earlymember")).toEqual(before);
  });

  it("stays the same through a rename or a clear, whatever the count is by then", async () => {
    const id = idMovedBy(32, "user_kept");
    await signUp(id);
    await claim(id, "keptcard");
    const face = deriveCard(id, CARD_SCENES);
    expect(await directory().card("keptcard")).toMatchObject(face);

    expect(await directory().setUsername(id, "keptcard2", 32)).toMatchObject({ card: face });
    await directory().clearUsername(id, false);
    expect(await directory().claimUsername(id, "keptcard3", 32)).toMatchObject({ card: face });
  });

  it("keeps the stored count through profile updates", async () => {
    const id = idMovedBy(28, "user_steady");
    await signUp(id);
    await claim(id, "steadycard");
    await webhook("user.updated", user(id, { first_name: "Renamed", updated_at: "2026-10-05T00:00:00.000Z" }));
    expect(await directory().card("steadycard")).toMatchObject({ firstName: "Renamed", ...deriveCard(id, CARD_SCENES) });
  });

  it("lets an operator pick a card's scene and foil, and put them back", async () => {
    const id = "user_facepick";
    await signUp(id);
    expect((await claim(id, "facepick")).status).toBe(200);
    const seeded = deriveCard(id, CARD_SCENES);
    const picked = await admin("/admin/usernames/face", { userId: id, scene: 2, holo: "glyph" });
    expect(picked.status).toBe(200);
    expect(await picked.json()).toMatchObject({ username: "facepick", scene: 2, holo: "glyph", sky: seeded.sky });
    expect(await (await readCard("facepick")).json()).toMatchObject({ scene: 2, holo: "glyph" });
    expect(Object.keys((await (await readCard("facepick")).json()) as object)).toEqual([
      "username", "firstName", "userNumber", "joinedOn", "badges", "scene", "holo", "sky"
    ]);
    // A rename keeps the pick.
    expect(await (await admin("/admin/usernames/set", { userId: id, username: "facepick2" })).json()).toMatchObject({ scene: 2, holo: "glyph" });
    // Null puts each back to the seed's.
    expect(await (await admin("/admin/usernames/face", { userId: id, scene: null, holo: null })).json()).toMatchObject({
      scene: seeded.scene,
      holo: seeded.holo
    });
    expect((await admin("/admin/usernames/face", { userId: id, scene: CARD_SCENES })).status).toBe(400);
    expect((await admin("/admin/usernames/face", { userId: id, holo: "neon" })).status).toBe(400);
    expect((await admin("/admin/usernames/face", { userId: id, scene: 1 }, "wrong-token")).status).toBe(401);
    expect((await admin("/admin/usernames/face", { userId: "user_nobodyhere", scene: 1 })).status).toBe(404);
  });

  it("keeps a card claimed at 20 scenes once more are added", async () => {
    expect(CARD_SCENES).toBe(26);
    const id = idMovedBy(26, "user_at20", 20);
    const fresh = idMovedBy(26, "user_at21", 20);
    await signUp(id, fresh);
    // Claimed before the bump, stored at 20.
    await directory().claimUsername(id, "claimedat20", 20);
    const face = deriveCard(id, 20);
    expect(face.scene).not.toBe(deriveCard(id, 26).scene);
    expect(await (await readCard("claimedat20")).json()).toMatchObject(face);
    // Today's routes pass 26; the stored 20 wins, through a rename too.
    expect(await (await claim(id, "claimedat20")).json()).toMatchObject(face);
    expect(await (await admin("/admin/usernames/set", { userId: id, username: "claimedat20b" })).json()).toMatchObject(face);
    // A claim made now draws from all 26.
    expect(await (await claim(fresh, "claimedat21")).json()).toMatchObject(deriveCard(fresh, 26));
  });

  it("adds the stored count to a directory that predates it", async () => {
    const stub = env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("legacy-usernames"));
    await runInDurableObject(stub, (_instance: UserDirectory, state) => {
      state.storage.sql.exec("DROP TABLE usernames");
      state.storage.sql.exec("ALTER TABLE users DROP COLUMN card_scenes");
      state.storage.sql.exec("INSERT INTO users (user_id, created_at, updated_at, recorded_at, user_number) VALUES ('user_old', ?, ?, 1, 1)", JOINED, JOINED);
      const directory = new UserDirectory(state, env as never);
      expect(directory.claimUsername("user_old", "oldmember", 20)).toMatchObject({ card: { username: "oldmember" } });
      new UserDirectory(state, env as never);
      expect(directory.card("oldmember")).toMatchObject(deriveCard("user_old", 20));
    });
  });
});
