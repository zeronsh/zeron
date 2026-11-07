/**
 * Several GitHub logins on one user: the rules docs/design/cloud-device.md
 * states under "GitHub". A fake GitHub answers by bearer token, so every check
 * proves WHICH login's token asked.
 */
import { env } from "cloudflare:workers";
import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { VaultCaller } from "../../src/api";
import { accountStub } from "../../src/names";
import { GITHUB_DEVICE_CODE_URL, GITHUB_TOKEN_URL, GITHUB_USER_URL } from "../../src/providers/github";
import { makeDevice, type TestDevice } from "../support";
import { json, setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const API = "https://api.github.com";
const HOUR = 60 * 60_000;

interface FakeLogin {
  readonly login: string;
  readonly id: number;
  token: string;
  /** Set: the token expires soon and refreshes with this. */
  refreshToken?: string;
  refreshAnswer?: "ok" | "bad_refresh_token" | "down";
  refreshes: number;
  /** GitHub answers 401 to this login's token. */
  revoked?: boolean;
  /** GitHub answers 500 to this login's API calls. */
  down?: boolean;
  /** GitHub doesn't answer `/user` for it (a login lookup that fails). */
  anonymous?: boolean;
  /** Revoked with GitHub: its user token no longer works. */
  userTokenRevoked?: boolean;
  readonly installations: { id: number; target: string; type?: string }[];
  /** `owner/name` → may this login push. */
  repos: Record<string, boolean>;
}

const person = (
  login: string,
  id: number,
  installations: FakeLogin["installations"],
  repos: FakeLogin["repos"],
  extra: Partial<FakeLogin> = {}
): FakeLogin => ({ login, id, token: `ghu_${login}_SECRET`, refreshes: 0, installations, repos, ...extra });

/** GitHub's names are case-insensitive. */
const sees = (who: FakeLogin, repo: string): boolean | undefined =>
  Object.entries(who.repos).find(([name]) => name.toLowerCase() === repo.toLowerCase())?.[1];

/** The fake: `pending` is who approves the next device flow. */
const fakeGithub = (...people: FakeLogin[]) => {
  const pending: FakeLogin[] = [];
  const asked: { url: string; token: string | undefined; method: string }[] = [];
  /** Installation tokens GitHub revoked, and user tokens. */
  const revoked = { installation: new Set<string>(), user: new Set<string>() };
  /** While true, GitHub doesn't answer revocations. */
  const outage = { revoke: false };
  /** Called as GitHub mints an installation token (a test's hook into the middle of a grant). */
  const hooks: { onMint?: () => Promise<void> } = {};
  let minted = 0;
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const token = new Headers(init?.headers).get("authorization")?.replace(/^Bearer /, "");
    const method = init?.method ?? "GET";
    asked.push({ url, token, method });
    if (url === `${API}/installation/token` && method === "DELETE") {
      if (outage.revoke) return json({ message: "boom" }, 502);
      if (!token || revoked.installation.has(token)) return json({ message: "Bad credentials" }, 401);
      revoked.installation.add(token);
      return new Response(null, { status: 204 });
    }
    if (url === `${API}/applications/Iv1.testclient/token` && method === "DELETE") {
      if (outage.revoke) throw new Error("network down");
      const body = JSON.parse(typeof init?.body === "string" ? init.body : "{}") as { access_token?: string };
      const owner = people.find((p) => p.token === body.access_token);
      if (!owner || owner.userTokenRevoked) return json({ message: "Not Found" }, 404);
      owner.userTokenRevoked = true;
      revoked.user.add(owner.login);
      return new Response(null, { status: 204 });
    }
    if (url === GITHUB_DEVICE_CODE_URL) {
      return json({ device_code: "dc", user_code: "ABCD-EFGH", verification_uri: "https://github.com/login/device", expires_in: 900, interval: 5 });
    }
    if (url === GITHUB_TOKEN_URL) {
      const form = new URLSearchParams(typeof init?.body === "string" ? init.body : "");
      if (form.get("grant_type") === "refresh_token") {
        const who = people.find((p) => p.refreshToken === form.get("refresh_token"));
        // Reuse of a rotated token is theft to GitHub: only the current one works.
        if (!who) return json({ error: "bad_refresh_token" });
        who.refreshes++;
        if (who.refreshAnswer === "bad_refresh_token") return json({ error: "bad_refresh_token" });
        if (who.refreshAnswer === "down") return json({}, 500);
        who.token = `ghu_${who.login}_SECRET_${who.refreshes}`;
        who.refreshToken = `ghr_${who.login}_${who.refreshes}`;
        return json({ access_token: who.token, expires_in: 28800, refresh_token: who.refreshToken, refresh_token_expires_in: 15897600 });
      }
      const who = pending.shift();
      if (!who) return json({ error: "authorization_pending" });
      return json(
        who.refreshToken
          ? { access_token: who.token, expires_in: 300, refresh_token: who.refreshToken, refresh_token_expires_in: 15897600 }
          : { access_token: who.token, token_type: "bearer" }
      );
    }
    const who = people.find((p) => p.token === token);
    const mint = url.match(/\/app\/installations\/(\d+)\/access_tokens$/);
    if (mint) {
      const n = ++minted;
      await hooks.onMint?.();
      return json({ token: `ghs_inst${mint[1]}_${n}`, expires_at: new Date(Date.now() + HOUR).toISOString() }, 201);
    }
    if (!who) throw new Error(`unexpected upstream call ${url}`);
    if (who.revoked) return json({ message: "Bad credentials" }, 401);
    if (who.down) return json({ message: "boom" }, 500);
    if (url === GITHUB_USER_URL) {
      return who.anonymous ? json({ message: "boom" }, 500) : json({ login: who.login, id: who.id });
    }
    if (url === `${API}/user/installations?per_page=100`) {
      return json({
        installations: who.installations.map((i) => ({
          id: i.id,
          account: { login: i.target, type: i.type ?? "Organization" },
          suspended_at: null
        }))
      });
    }
    const listing = url.match(/\/user\/installations\/(\d+)\/repositories/);
    if (listing) {
      const target = who.installations.find((i) => i.id === Number(listing[1]))?.target.toLowerCase();
      return json({
        repositories: Object.entries(who.repos)
          .filter(([name]) => name.split("/")[0]!.toLowerCase() === target)
          .map(([name, push]) => ({
            full_name: name,
            clone_url: `https://github.com/${name}.git`,
            default_branch: "main",
            private: true,
            pushed_at: "2026-09-01T00:00:00Z",
            permissions: { pull: true, push }
          }))
      });
    }
    const branches = url.match(/\/repos\/([^/]+\/[^/]+)\/branches/);
    if (branches) {
      return sees(who, branches[1]!) === undefined ? json({ message: "Not Found" }, 404) : json([{ name: `${who.login}-branch` }]);
    }
    const repo = url.match(/\/repos\/([^/]+\/[^/?]+)$/);
    if (repo) {
      const push = sees(who, repo[1]!);
      return push === undefined ? json({ message: "Not Found" }, 404) : json({ permissions: { pull: true, push } });
    }
    const owner = url.match(/\/users\/([^/?]+)$/);
    if (owner) return owner[1] === "nameless" ? json({ message: "Not Found" }, 404) : json({ id: 9000 + owner[1]!.length });
    throw new Error(`unexpected upstream call ${url}`);
  });
  return {
    asked,
    revoked,
    outage,
    hooks,
    approveAs: (who: FakeLogin) => pending.push(who),
    tokensAsking: (fragment: string) => asked.filter((a) => a.url.includes(fragment)).map((a) => a.token)
  };
};

type Fake = ReturnType<typeof fakeGithub>;

const connect = async (caller: VaultCaller, github: Fake, who: FakeLogin, devices: string[]) => {
  github.approveAs(who);
  const start = value(await vault().githubDeviceStart(caller, devices));
  const done = value(await vault().githubDevicePoll(caller, start.flowId));
  expect(done.state).toBe("connected");
  return done;
};

/** A user with a logical Cloud device, everything authorized through it. */
const cloudUser = async () => {
  const ctx = await setup("cloud");
  const session = async (): Promise<TestDevice> => {
    const device = await makeDevice(`cloud-${crypto.randomUUID()}`);
    value(
      await vault().enrollDevice(ctx.caller, {
        deviceId: device.deviceId,
        kind: "cloud",
        publicKey: device.publicKey,
        parentId: ctx.device.deviceId
      })
    );
    return device;
  };
  const grant = async (device: TestDevice, repo: string, inheritFrom?: string, caller: VaultCaller = ctx.caller) => {
    const request = await signedGrant(device, ctx.userId, "github", undefined, repo);
    return vault().grant(caller, { ...request, ...(inheritFrom ? { inheritFrom } : {}) });
  };
  return { ...ctx, parent: ctx.device.deviceId, session, grant };
};

const sessionRows = (userId: string) =>
  runInDurableObject(accountStub(env, userId, "github"), (_instance, state) =>
    state.storage.sql.exec<Record<string, string | number>>("SELECT * FROM github_sessions").toArray()
  );

const workInstall = { id: 100, target: "work-org" };
const homeInstall = { id: 200, target: "home-user", type: "User" };
const sharedInstall = { id: 300, target: "shared-org" };

describe("connecting several GitHub logins", () => {
  it("adds a login without removing one, and lists them each with its picture", async () => {
    const { caller, parent } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(work, home);
    expect(await connect(caller, github, work, [parent])).toEqual({ state: "connected", account: "work-me" });
    expect(await connect(caller, github, home, [parent])).toEqual({ state: "connected", account: "home-me" });
    const accounts = value(await vault().accounts(caller, "github"));
    expect(accounts.map((a) => a.account)).toEqual(["work-me", "home-me"]);
    expect(accounts.every((a) => a.status === "connected" && a.kind === "github")).toBe(true);
    expect(accounts[0]!.avatarUrl).toBe("https://github.com/work-me.png?size=64");
  });

  it("the same login again replaces itself and says so, so the app can explain", async () => {
    const { caller, parent } = await cloudUser();
    const work = person("work-me", 1, [workInstall], {});
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const slot = value(await vault().accounts(caller, "github"))[0]!.slot;
    expect(await connect(caller, github, work, [parent])).toEqual({ state: "connected", account: "work-me", alreadyConnected: true });
    const accounts = value(await vault().accounts(caller, "github"));
    expect(accounts.map((a) => a.slot)).toEqual([slot]);
  });

  it("holds ten logins; an eleventh is refused and the rest stay, while a held one can still sign in again", async () => {
    const { caller, parent } = await cloudUser();
    const people = Array.from({ length: 11 }, (_, i) => person(`login-${i}`, i, [], {}));
    const github = fakeGithub(...people);
    for (const who of people.slice(0, 10)) await connect(caller, github, who, [parent]);
    github.approveAs(people[10]!);
    const start = value(await vault().githubDeviceStart(caller, [parent]));
    expect(value(await vault().githubDevicePoll(caller, start.flowId))).toMatchObject({
      state: "failed",
      error: expect.stringContaining("at most 10")
    });
    expect(value(await vault().accounts(caller, "github"))).toHaveLength(10);
    await connect(caller, github, people[3]!, [parent]);
    expect(value(await vault().accounts(caller, "github"))).toHaveLength(10);
  });
});

describe("the merged repository list", () => {
  it("merges two logins with distinct repositories; each knows its login and installation", async () => {
    const { caller, parent } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    await connect(caller, github, home, [parent]);
    const listing = value(await vault().githubListing(caller));
    const byName = Object.fromEntries(listing.repos.map((r) => [r.fullName, r]));
    expect(byName["work-org/api"]).toMatchObject({ account: "work-me", accounts: ["work-me"], installationId: 100 });
    expect(byName["home-user/blog"]).toMatchObject({ account: "home-me", accounts: ["home-me"], installationId: 200 });
    expect(listing.unavailable).toBeUndefined();
    expect(listing.installations).toEqual([
      {
        id: 200,
        target: "home-user",
        targetType: "User",
        reachedBy: ["home-me"],
        manageUrl: "https://github.com/settings/installations/200"
      },
      {
        id: 100,
        target: "work-org",
        targetType: "Organization",
        reachedBy: ["work-me"],
        manageUrl: "https://github.com/organizations/work-org/settings/installations/100"
      }
    ]);
    // Asked with each login's own token; none of them comes back.
    expect(github.tokensAsking("/user/installations/100/")).toEqual([work.token]);
    expect(github.tokensAsking("/user/installations/200/")).toEqual([home.token]);
    expect(JSON.stringify(listing)).not.toContain("SECRET");
    expect(value(await vault().githubListing(caller, "BLOG")).repos.map((r) => r.fullName)).toEqual(["home-user/blog"]);
  });

  it("lists a repository two logins reach once, naming the most recently connected", async () => {
    const { caller, parent } = await cloudUser();
    const older = person("older", 1, [sharedInstall], { "shared-org/app": true });
    const newer = person("newer", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(older, newer);
    await connect(caller, github, older, [parent]);
    await connect(caller, github, newer, [parent]);
    const listing = value(await vault().githubListing(caller));
    expect(listing.repos.map((r) => r.fullName)).toEqual(["shared-org/app"]);
    expect(listing.repos[0]).toMatchObject({ account: "newer", accounts: ["newer", "older"], installationId: 300 });
    // One installation record, reached by both: access isn't owned by a login.
    expect(listing.installations).toHaveLength(1);
    expect([...listing.installations[0]!.reachedBy].sort()).toEqual(["newer", "older"]);
  });

  it("prefers a login that can push over a newer one that can only read", async () => {
    const { caller, parent } = await cloudUser();
    const writer = person("writer", 1, [sharedInstall], { "shared-org/app": true });
    const reader = person("reader", 2, [sharedInstall], { "shared-org/app": false });
    const github = fakeGithub(writer, reader);
    await connect(caller, github, writer, [parent]);
    await connect(caller, github, reader, [parent]);
    expect(value(await vault().githubListing(caller)).repos[0]).toMatchObject({ account: "writer", accounts: ["writer", "reader"] });
  });

  it("branches come through a login that can see the repository", async () => {
    const { caller, parent } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    await connect(caller, github, home, [parent]);
    expect(value(await vault().githubBranches(caller, "work-org/api"))).toEqual(["work-me-branch"]);
    expect(value(await vault().githubBranches(caller, "home-user/blog"))).toEqual(["home-me-branch"]);
    expect(await vault().githubBranches(caller, "other/none")).toMatchObject({ ok: false, error: "not_found" });
  });
});

describe("a session's login", () => {
  it("a grant uses the login that reaches the repository: its token checks, and it authors", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    await connect(caller, github, home, [parent]);

    const onWork = await session();
    const workGrant = value(await grant(onWork, "work-org/api"));
    expect(workGrant).toMatchObject({ provider: "github", account: "work-me", installationId: 100 });
    expect(workGrant.accessToken).toMatch(/^ghs_inst100_/);
    // The older login, though the newer is active, and never a user token.
    expect(JSON.stringify(workGrant)).not.toContain("SECRET");
    expect(github.asked.some((a) => a.url === `${API}/repos/work-org/api` && a.token === work.token)).toBe(true);

    const onHome = await session();
    expect(value(await grant(onHome, "home-user/blog"))).toMatchObject({ account: "home-me", installationId: 200 });
  });

  it("records both the login and the installation, apart", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("Work-Me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const device = await session();
    value(await grant(device, "work-org/API"));
    expect(await sessionRows(userId)).toEqual([
      expect.objectContaining({
        device_id: device.deviceId,
        repo: "work-org/api",
        login: "login:work-me",
        account: "Work-Me",
        installation_id: 100
      })
    ]);
  });

  it("stays on the login it started with; a new session takes the most recently connected", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const first = person("first", 1, [sharedInstall], { "shared-org/app": true });
    const second = person("second", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(first, second);
    await connect(caller, github, first, [parent]);
    const old = await session();
    expect(value(await grant(old, "shared-org/app")).account).toBe("first");

    await connect(caller, github, second, [parent]);
    expect(value(await grant(old, "shared-org/app")).account).toBe("first");
    const fresh = await session();
    expect(value(await grant(fresh, "shared-org/app")).account).toBe("second");
    // And both keep doing so.
    expect(value(await grant(old, "shared-org/app")).account).toBe("first");
    expect(value(await grant(fresh, "shared-org/app")).account).toBe("second");
  });

  it("the user can pick which login new sessions try first, and running ones don't move", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const first = person("first", 1, [sharedInstall], { "shared-org/app": true });
    const second = person("second", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(first, second);
    await connect(caller, github, first, [parent]);
    await connect(caller, github, second, [parent]);
    const running = await session();
    expect(value(await grant(running, "shared-org/app")).account).toBe("second");
    const slot = value(await vault().accounts(caller, "github")).find((a) => a.account === "first")!.slot;
    value(await vault().activateAccount(caller, "github", slot));
    expect(value(await vault().githubListing(caller)).repos[0]).toMatchObject({ account: "first" });
    expect(value(await grant(await session(), "shared-org/app")).account).toBe("first");
    expect(value(await grant(running, "shared-org/app")).account).toBe("second");
  });

  it("takes a login that can push; a read-only pick waits while another login can't answer", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const writer = person("writer", 1, [sharedInstall], { "shared-org/app": true });
    const reader = person("reader", 2, [sharedInstall], { "shared-org/app": false });
    const github = fakeGithub(writer, reader);
    await connect(caller, github, writer, [parent]);
    await connect(caller, github, reader, [parent]);
    writer.down = true;
    const device = await session();
    expect(value(await grant(device, "shared-org/app")).account).toBe("reader");
    writer.down = false;
    expect(value(await grant(device, "shared-org/app")).account).toBe("writer");
    const mints = github.asked.filter((a) => a.url.endsWith("/access_tokens"));
    expect(mints).toHaveLength(2);
    // Once chosen with everyone answering, it's kept.
    reader.repos["shared-org/app"] = true;
    expect(value(await grant(device, "shared-org/app")).account).toBe("writer");
  });

  it("a delegate inherits its parent's login, not the most recent", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const first = person("first", 1, [sharedInstall], { "shared-org/app": true });
    const second = person("second", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(first, second);
    await connect(caller, github, first, [parent]);
    const parentSession = await session();
    value(await grant(parentSession, "shared-org/app"));
    await connect(caller, github, second, [parent]);

    const child = await session();
    const runner: VaultCaller = { userId, orgId: "org_test", kind: "runner", deviceId: child.deviceId };
    const inherited = value(await grant(child, "shared-org/app", parentSession.deviceId, runner));
    expect(inherited).toMatchObject({ account: "first", installationId: 300 });
    // And its own record now holds, parent or no parent.
    expect(value(await grant(child, "shared-org/app", undefined, runner)).account).toBe("first");
  });

  it("only a runner's request carries a parent: a user's is ignored", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const first = person("first", 1, [sharedInstall], { "shared-org/app": true });
    const second = person("second", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(first, second);
    await connect(caller, github, first, [parent]);
    const parentSession = await session();
    value(await grant(parentSession, "shared-org/app"));
    await connect(caller, github, second, [parent]);
    expect(value(await grant(await session(), "shared-org/app", parentSession.deviceId)).account).toBe("second");
  });

  it("a machine gets grants and nothing else: no listing, no logins, no way to change them", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const device = await session();
    const runner: VaultCaller = { userId, orgId: "org_test", kind: "runner", deviceId: device.deviceId };
    const slot = value(await vault().accounts(caller, "github"))[0]!.slot;
    for (const refused of [
      await vault().githubListing(runner),
      await vault().githubRepos(runner),
      await vault().githubBranches(runner, "work-org/api"),
      await vault().accounts(runner, "github"),
      await vault().forgetAccount(runner, "github", slot),
      await vault().activateAccount(runner, "github", slot)
    ]) {
      expect(refused).toMatchObject({ ok: false, error: "forbidden", status: 403 });
    }
    const granted = value(await grant(device, "work-org/api", undefined, runner));
    expect(granted.accessToken).toMatch(/^ghs_/);
    expect(JSON.stringify(granted)).not.toContain(work.token);
  });

  it("refuses a device no login authorizes", async () => {
    const { caller, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(work);
    await connect(caller, github, work, ["some-other-device"]);
    expect(await grant(await session(), "work-org/api")).toMatchObject({ ok: false, error: "not_authorized" });
  });
});

describe("one login's failure", () => {
  it("refreshes each login on its own, one refresh per login however many ask", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true }, { refreshToken: "ghr_work_0" });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true }, { refreshToken: "ghr_home_0" });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    await connect(caller, github, home, [parent]);
    const devices = await Promise.all(Array.from({ length: 6 }, () => session()));
    const grants = await Promise.all(devices.map((device, i) => grant(device, i % 2 ? "work-org/api" : "home-user/blog")));
    grants.forEach((g, i) => expect(value(g).account).toBe(i % 2 ? "work-me" : "home-me"));
    // Each login's single-use refresh token went up once.
    expect(work.refreshes).toBe(1);
    expect(home.refreshes).toBe(1);
    expect(github.tokensAsking("/repos/work-org/api").every((token) => token === work.token)).toBe(true);
  });

  it("a refused refresh marks that login; the other keeps working and the app can say which", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const dead = person("dead-me", 1, [workInstall], { "work-org/api": true }, { refreshToken: "ghr_dead", refreshAnswer: "bad_refresh_token" });
    const fine = person("fine-me", 2, [homeInstall], { "home-user/blog": true }, { refreshToken: "ghr_fine" });
    const github = fakeGithub(dead, fine);
    await connect(caller, github, dead, [parent]);
    const onDead = await session();
    await connect(caller, github, fine, [parent]);
    const onFine = await session();

    const listing = value(await vault().githubListing(caller));
    expect(listing.repos.map((r) => r.fullName)).toEqual(["home-user/blog"]);
    expect(listing.unavailable).toEqual([expect.objectContaining({ account: "dead-me", reason: "needs_reconnect" })]);

    const accounts = value(await vault().accounts(caller, "github"));
    expect(accounts.find((a) => a.account === "dead-me")?.status).toBe("needsReconnect");
    expect(accounts.find((a) => a.account === "fine-me")?.status).toBe("connected");

    expect(value(await grant(onFine, "home-user/blog")).account).toBe("fine-me");
    // Nothing else reaches it, and the refusal names who to reconnect.
    const refused = await grant(onDead, "work-org/api");
    expect(refused).toMatchObject({ ok: false, error: "needs_reconnect", status: 409 });
    expect(!refused.ok && refused.message).toContain("github_account_needs_reconnect");
    expect(!refused.ok && refused.message).toContain("dead-me");
    expect(fine.refreshes).toBe(1);
  });

  it("a session on a dead login stays there with a clear error, never moving to another that reaches it", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const first = person("first", 1, [sharedInstall], { "shared-org/app": true });
    const second = person("second", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(first, second);
    await connect(caller, github, first, [parent]);
    const running = await session();
    value(await grant(running, "shared-org/app"));
    await connect(caller, github, second, [parent]);

    first.revoked = true;
    const refused = await grant(running, "shared-org/app");
    expect(refused).toMatchObject({ ok: false, error: "needs_reconnect" });
    expect(!refused.ok && refused.message).toContain("first");

    // Connecting it again carries on, same login.
    first.revoked = false;
    await connect(caller, github, first, [parent]);
    expect(value(await grant(running, "shared-org/app")).account).toBe("first");
  });

  it("a login GitHub isn't answering for is a retry, and the others list", async () => {
    const { caller, parent } = await cloudUser();
    const flaky = person("flaky", 1, [workInstall], { "work-org/api": true });
    const fine = person("fine", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(flaky, fine);
    await connect(caller, github, flaky, [parent]);
    await connect(caller, github, fine, [parent]);
    flaky.down = true;
    const listing = value(await vault().githubListing(caller));
    expect(listing.repos.map((r) => r.fullName)).toEqual(["home-user/blog"]);
    expect(listing.unavailable).toEqual([expect.objectContaining({ account: "flaky", reason: "upstream" })]);
    // Still connected: an outage says nothing about the login.
    expect(value(await vault().accounts(caller, "github")).find((a) => a.account === "flaky")?.status).toBe("connected");
  });

  it("an unreadable login's record doesn't stop the others", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const broken = person("broken", 1, [sharedInstall], { "shared-org/app": true });
    const fine = person("fine", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(broken, fine);
    await connect(caller, github, fine, [parent]);
    await connect(caller, github, broken, [parent]);
    await runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => {
      state.storage.sql.exec("UPDATE accounts SET envelope = 'v2.nope.nope.nope' WHERE account = 'broken'");
    });
    expect(value(await vault().githubListing(caller)).repos[0]).toMatchObject({ account: "fine" });
    expect(value(await grant(await session(), "shared-org/app")).account).toBe("fine");
  });

  it("when every login fails the listing fails as it always did", async () => {
    const { caller, parent } = await cloudUser();
    const a = person("a-me", 1, [], {});
    const b = person("b-me", 2, [], {});
    const github = fakeGithub(a, b);
    await connect(caller, github, a, [parent]);
    await connect(caller, github, b, [parent]);
    a.revoked = true;
    b.down = true;
    const failed = await vault().githubRepos(caller);
    expect(failed).toMatchObject({ ok: false, error: "needs_reconnect", status: 409 });
    expect(!failed.ok && failed.message).toContain("a-me");
  });
});

describe("disconnecting one login", () => {
  it("leaves the others; its sessions get a clear error and come back when it does", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    await connect(caller, github, home, [parent]);
    const onWork = await session();
    const onHome = await session();
    value(await grant(onWork, "work-org/api"));
    value(await grant(onHome, "home-user/blog"));

    const before = value(await vault().accounts(caller, "github"));
    const slot = before.find((a) => a.account === "work-me")!.slot;
    const left = value(await vault().forgetAccount(caller, "github", slot));
    expect(left.map((a) => a.account)).toEqual(["home-me"]);

    // Its repositories, installation and cached tokens went with it.
    const listing = value(await vault().githubListing(caller));
    expect(listing.repos.map((r) => r.fullName)).toEqual(["home-user/blog"]);
    expect(listing.installations.map((i) => i.id)).toEqual([200]);
    const cached = await runInDurableObject(accountStub(env, userId, "github"), (_instance, state) =>
      state.storage.sql.exec<{ installation_id: number }>("SELECT installation_id FROM github_tokens").toArray()
    );
    expect(cached.map((row) => row.installation_id)).toEqual([200]);

    const refused = await grant(onWork, "work-org/api");
    expect(refused).toMatchObject({ ok: false, error: "needs_reconnect", status: 409 });
    expect(!refused.ok && refused.message).toContain("github_account_disconnected");
    expect(!refused.ok && refused.message).toContain("work-me");
    // The other session never noticed.
    expect(value(await grant(onHome, "home-user/blog")).account).toBe("home-me");

    // The same login again (a new row): its sessions resume.
    await connect(caller, github, work, [parent]);
    expect(value(await grant(onWork, "work-org/api")).account).toBe("work-me");
  });

  it("doesn't move a session to another login that reaches its repository", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const first = person("first", 1, [sharedInstall], { "shared-org/app": true });
    const second = person("second", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(first, second);
    await connect(caller, github, first, [parent]);
    const running = await session();
    value(await grant(running, "shared-org/app"));
    await connect(caller, github, second, [parent]);
    const slot = value(await vault().accounts(caller, "github")).find((a) => a.account === "first")!.slot;
    value(await vault().forgetAccount(caller, "github", slot));

    expect(await grant(running, "shared-org/app")).toMatchObject({ ok: false, error: "needs_reconnect" });
    // A new session is free to use what's left, and the installation stays.
    expect(value(await grant(await session(), "shared-org/app")).account).toBe("second");
  });

  it("forgetting the login tried first hands that place to another", async () => {
    const { caller, parent } = await cloudUser();
    const first = person("first", 1, [workInstall], { "work-org/api": true });
    const second = person("second", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(first, second);
    await connect(caller, github, first, [parent]);
    await connect(caller, github, second, [parent]);
    const active = value(await vault().accounts(caller, "github")).find((a) => a.active)!;
    expect(active.account).toBe("second");
    const left = value(await vault().forgetAccount(caller, "github", active.slot));
    expect(left).toEqual([expect.objectContaining({ account: "first", active: true })]);
    expect(value(await vault().githubListing(caller)).repos.map((r) => r.fullName)).toEqual(["work-org/api"]);
  });

  it("forgetting a login that's already gone changes nothing", async () => {
    const { caller, parent } = await cloudUser();
    const work = person("work-me", 1, [workInstall], {});
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    expect(value(await vault().forgetAccount(caller, "github", "agone"))).toHaveLength(1);
    expect(await vault().forgetAccount(caller, "github", "bad id!")).toMatchObject({ ok: false, error: "bad_request" });
  });
});

describe("what's already stored", () => {
  /** What the object's constructor does to stored rows. */
  const restart = (instance: unknown) => {
    const object = instance as { migrateSingleRecord(): void; settleGithubLogins(): void };
    object.migrateSingleRecord();
    object.settleGithubLogins();
  };

  /** Rewind the object to one login stored the old way: no identity, no sessions. */
  const toLegacy = (userId: string) =>
    runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => {
      state.storage.sql.exec("UPDATE accounts SET identity = NULL");
    });

  it("a single login from before keeps working untouched: same list, same grants", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const only = person("only-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(only);
    await connect(caller, github, only, [parent]);
    await toLegacy(userId);
    expect(value(await vault().githubRepos(caller)).map((r) => r.fullName)).toEqual(["work-org/api"]);
    expect(value(await grant(await session(), "work-org/api"))).toMatchObject({ account: "only-me", generation: 1 });
  });

  it("the single-record layout migrates into a login that signs in to itself", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const only = person("only-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(only);
    await connect(caller, github, only, [parent]);
    const stub = accountStub(env, userId, "github");
    await runInDurableObject(stub, (instance, state) => {
      const sql = state.storage.sql;
      const row = sql.exec<Record<string, SqlStorageValue>>("SELECT * FROM accounts").toArray()[0]!;
      sql.exec(
        `CREATE TABLE record (id INTEGER PRIMARY KEY CHECK (id = 1), generation INTEGER NOT NULL, status TEXT NOT NULL,
           account TEXT, authorized_devices TEXT NOT NULL, envelope TEXT NOT NULL, updated_at INTEGER NOT NULL,
           hard_expires_at INTEGER, last_refresh_at INTEGER, last_attempt_at INTEGER, last_error TEXT, failures INTEGER NOT NULL DEFAULT 0)`
      );
      sql.exec(
        "INSERT INTO record (id, generation, status, account, authorized_devices, envelope, updated_at) VALUES (1, ?, ?, ?, ?, ?, ?)",
        row.generation,
        row.status,
        row.account,
        row.authorized_devices,
        row.envelope,
        row.updated_at
      );
      sql.exec("DELETE FROM accounts");
      sql.exec("DELETE FROM meta WHERE key = 'active'");
      restart(instance);
    });

    const migrated = value(await vault().accounts(caller, "github"));
    expect(migrated).toEqual([expect.objectContaining({ account: "only-me", active: true, status: "connected" })]);
    expect(value(await grant(await session(), "work-org/api")).account).toBe("only-me");
    // Connecting it again replaces it instead of adding a double.
    expect(await connect(caller, github, only, [parent])).toMatchObject({ alreadyConnected: true });
    expect(value(await vault().accounts(caller, "github")).map((a) => a.slot)).toEqual([migrated[0]!.slot]);
  });

  it("merges a login stored twice, keeping the newest and a working session's standing", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const only = person("only-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(only);
    await connect(caller, github, only, [parent]);
    await toLegacy(userId);
    await connect(caller, github, only, [parent]);
    const stub = accountStub(env, userId, "github");
    await runInDurableObject(stub, (instance) => restart(instance));
    const accounts = value(await vault().accounts(caller, "github"));
    expect(accounts).toHaveLength(1);
    expect(accounts[0]!.active).toBe(true);
    expect(value(await grant(await session(), "work-org/api")).account).toBe("only-me");
  });

  it("a login GitHub wouldn't name at connect time learns its name on first use, sessions included", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const shy = person("shy-me", 1, [workInstall], { "work-org/api": true }, { anonymous: true });
    const github = fakeGithub(shy);
    expect(await connect(caller, github, shy, [parent])).toEqual({ state: "connected" });
    expect(value(await vault().accounts(caller, "github"))[0]!.account).toBeUndefined();

    const device = await session();
    const nameless = value(await grant(device, "work-org/api"));
    expect(nameless.account).toBeUndefined();
    expect((await sessionRows(userId))[0]!.login).toMatch(/^slot:/);

    shy.anonymous = false;
    expect(value(await grant(device, "work-org/api")).account).toBe("shy-me");
    expect(await sessionRows(userId)).toEqual([expect.objectContaining({ login: "login:shy-me", account: "shy-me" })]);
    // Signing in as it now replaces it.
    expect(await connect(caller, github, shy, [parent])).toMatchObject({ alreadyConnected: true });
    expect(value(await vault().accounts(caller, "github"))).toHaveLength(1);
  });
});

describe("old clients on the new vault", () => {
  it("still read every single-account shape", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    await connect(caller, github, home, [parent]);

    // `githubRepos` is still a plain array of what an old client parses.
    const repos = value(await vault().githubRepos(caller));
    expect(Array.isArray(repos)).toBe(true);
    for (const repo of repos) {
      expect(repo).toMatchObject({
        fullName: expect.any(String),
        cloneUrl: expect.any(String),
        defaultBranch: expect.any(String),
        private: expect.any(Boolean),
        pushedAt: expect.any(Number)
      });
    }

    // Status: one summary per provider, as before.
    const status = value(await vault().status(caller));
    const connections = status.connections.filter((c) => c.provider === "github");
    expect(connections).toHaveLength(1);
    expect(connections[0]).toMatchObject({ status: "connected", account: "home-me", accounts: 2, hasActive: true });
    expect(status.githubInstallUrl).toBe("https://github.com/apps/zeron-test/installations/new");

    // A grant still answers the fields an old machine reads.
    const granted = value(await grant(await session(), "work-org/api"));
    expect(granted).toMatchObject({
      provider: "github",
      accessToken: expect.any(String),
      expiresAt: expect.any(Number),
      generation: expect.any(Number),
      account: "work-me"
    });

    // Its "Disconnect" still means all of GitHub.
    value(await vault().disconnect(caller, "github"));
    expect(value(await vault().accounts(caller, "github"))).toEqual([]);
    expect(await vault().githubRepos(caller)).toMatchObject({ ok: false, error: "not_found", status: 404 });
  });
});

describe("installing the App", () => {
  it("a repository's owner without it gets a link aimed at that owner", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], {});
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const refused = await grant(await session(), "elsewhere/app");
    expect(refused).toMatchObject({ ok: false, error: "not_found", status: 404 });
    expect(!refused.ok && refused.message).toContain("github_app_not_installed");
    expect(!refused.ok && refused.message).toContain("no GitHub account you connected");
    expect(!refused.ok && refused.message).toContain(
      `https://github.com/apps/zeron-test/installations/new/permissions?suggested_target_id=${9000 + "elsewhere".length}`
    );
  });

  it("the link still works when GitHub won't say who the owner is", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], {});
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const refused = await grant(await session(), "nameless/app");
    expect(refused).toMatchObject({ ok: false, error: "not_found" });
    expect(!refused.ok && refused.message).toContain("install it at https://github.com/apps/zeron-test/installations/new,");
  });
});

describe("revoking a disconnected login's tokens", () => {
  const queued = (userId: string) =>
    runInDurableObject(accountStub(env, userId, "github"), (_instance, state) =>
      state.storage.sql.exec<{ kind: string; attempts: number; next_at: number }>("SELECT kind, attempts, next_at FROM github_revocations").toArray()
    );
  const alarmAt = (userId: string) =>
    runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => state.storage.getAlarm());
  /** Make every queued revocation due now, then run the object's alarm. */
  const alarmNow = async (userId: string) => {
    await runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => {
      state.storage.sql.exec("UPDATE github_revocations SET next_at = ?", Date.now() - 1);
    });
    return runDurableObjectAlarm(accountStub(env, userId, "github"));
  };
  const slotOf = async (caller: VaultCaller, login: string) =>
    value(await vault().accounts(caller, "github")).find((a) => a.account === login)!.slot;

  it("revokes its user token and every installation token minted for its sessions, and no one else's", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [sharedInstall], { "shared-org/app": true });
    const home = person("home-me", 2, [sharedInstall], { "shared-org/app": true });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    const onWork = await session();
    const onWork2 = await session();
    const workTokens = [value(await grant(onWork, "shared-org/app")).accessToken];
    // A second token for the same repository (the first nearly spent).
    await runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => {
      state.storage.sql.exec("UPDATE github_tokens SET expires_at = ?", Date.now() + 10 * 60_000);
    });
    workTokens.push(value(await grant(onWork2, "shared-org/app")).accessToken);
    expect(new Set(workTokens).size).toBe(2);
    await connect(caller, github, home, [parent]);
    const onHome = await session();
    const homeToken = value(await grant(onHome, "shared-org/app")).accessToken;
    // Never one token for two logins, even on one repository.
    expect(workTokens).not.toContain(homeToken);

    value(await vault().forgetAccount(caller, "github", await slotOf(caller, "work-me")));
    expect([...github.revoked.installation].sort()).toEqual([...workTokens].sort());
    expect([...github.revoked.user]).toEqual(["work-me"]);
    expect(await queued(userId)).toEqual([]);

    // The other login's session keeps its very token; the disconnected one's says why.
    expect(value(await grant(onHome, "shared-org/app")).accessToken).toBe(homeToken);
    const refused = await grant(onWork, "shared-org/app");
    expect(refused).toMatchObject({ ok: false, error: "needs_reconnect" });
    expect(!refused.ok && refused.message).toContain("github_account_disconnected");
  });

  it("disconnects even when GitHub can't be reached, and revokes on the alarm once it can", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const token = value(await grant(await session(), "work-org/api")).accessToken;

    github.outage.revoke = true;
    const left = await vault().forgetAccount(caller, "github", await slotOf(caller, "work-me"));
    expect(value(left)).toEqual([]);
    expect(github.revoked.installation.size + github.revoked.user.size).toBe(0);
    const waiting = await queued(userId);
    expect(waiting.map((r) => r.kind).sort()).toEqual(["installation", "user"]);
    expect(waiting.every((r) => r.attempts === 1 && r.next_at > Date.now())).toBe(true);
    expect(await alarmAt(userId)).not.toBeNull();

    // Still down: tried again, later still.
    expect(await alarmNow(userId)).toBe(true);
    expect((await queued(userId)).every((r) => r.attempts === 2)).toBe(true);

    github.outage.revoke = false;
    expect(await alarmNow(userId)).toBe(true);
    expect([...github.revoked.installation]).toEqual([token]);
    expect([...github.revoked.user]).toEqual(["work-me"]);
    expect(await queued(userId)).toEqual([]);
  });

  it("a revocation past its token's end is dropped, not sent", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    value(await grant(await session(), "work-org/api"));
    github.outage.revoke = true;
    value(await vault().forgetAccount(caller, "github", await slotOf(caller, "work-me")));
    await runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => {
      state.storage.sql.exec("UPDATE github_revocations SET give_up_at = ?", Date.now() - 1);
    });
    github.outage.revoke = false;
    const before = github.asked.length;
    await alarmNow(userId);
    expect(github.asked.slice(before).filter((a) => a.method === "DELETE")).toEqual([]);
    expect(await queued(userId)).toEqual([]);
  });

  it("a token GitHub mints while its login is being disconnected is revoked, never handed out", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const slot = await slotOf(caller, "work-me");
    github.hooks.onMint = async () => {
      github.hooks.onMint = undefined;
      value(await vault().forgetAccount(caller, "github", slot));
    };
    const raced = await grant(await session(), "work-org/api");
    expect(raced).toMatchObject({ ok: false, error: "needs_reconnect" });
    expect(github.revoked.installation.size).toBe(1);
    expect(await queued(userId)).toEqual([]);
  });

  it("disconnecting all of GitHub revokes every login's tokens", async () => {
    const { caller, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const home = person("home-me", 2, [homeInstall], { "home-user/blog": true });
    const github = fakeGithub(work, home);
    await connect(caller, github, work, [parent]);
    await connect(caller, github, home, [parent]);
    const tokens = [
      value(await grant(await session(), "work-org/api")).accessToken,
      value(await grant(await session(), "home-user/blog")).accessToken
    ];
    value(await vault().disconnect(caller, "github"));
    expect([...github.revoked.installation].sort()).toEqual(tokens.sort());
    expect([...github.revoked.user].sort()).toEqual(["home-me", "work-me"]);
  });

  it("a token GitHub has already forgotten counts as revoked", async () => {
    const { caller, userId, parent, session, grant } = await cloudUser();
    const work = person("work-me", 1, [workInstall], { "work-org/api": true });
    const github = fakeGithub(work);
    await connect(caller, github, work, [parent]);
    const token = value(await grant(await session(), "work-org/api")).accessToken;
    github.revoked.installation.add(token);
    work.userTokenRevoked = true;
    value(await vault().forgetAccount(caller, "github", await slotOf(caller, "work-me")));
    expect(await queued(userId)).toEqual([]);
  });
});
