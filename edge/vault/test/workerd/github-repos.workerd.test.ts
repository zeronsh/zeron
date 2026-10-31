import { afterEach, describe, expect, it, vi } from "vitest";
import { GITHUB_TOKEN_URL } from "../../src/providers/github";
import {
  connectGithub as connect,
  githubConnectRoutes,
  json,
  mockUpstream,
  runnerCaller,
  setup,
  value,
  vault,
  type UpstreamCall
} from "./helpers";

afterEach(() => vi.restoreAllMocks());

const TOKEN = "ghu_repos_SECRETSECRET_token_0001";
const API = "https://api.github.com";
const INSTALLATIONS = `${API}/user/installations?per_page=100`;
const installationRepos = (id: number, page?: number) =>
  `${API}/user/installations/${id}/repositories?per_page=100${page ? `&page=${page}` : ""}`;

const repo = (fullName: string, pushedAt: string, extra: Record<string, unknown> = {}) => ({
  id: Math.floor(Math.random() * 1e9),
  full_name: fullName,
  clone_url: `https://github.com/${fullName}.git`,
  default_branch: "main",
  private: false,
  description: null,
  pushed_at: pushedAt,
  ...extra
});

const paged = (body: unknown, next?: string) =>
  new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json", ...(next ? { link: `<${next}>; rel="next", <${next}>; rel="last"` } : {}) }
  });

const connectRoutes = (token: Record<string, unknown> = { access_token: TOKEN, token_type: "bearer" }) =>
  githubConnectRoutes(token);

const bearer = (call: UpstreamCall) => new Headers(call.init?.headers).get("authorization");

describe("githubRepos", () => {
  it("lists installation repositories across pages, deduped, newest push first, token never returned", async () => {
    const { caller, device } = await setup();
    const upstream = mockUpstream({
      ...connectRoutes(),
      [INSTALLATIONS]: () => json({ total_count: 2, installations: [{ id: 11 }, { id: 22 }] }),
      [installationRepos(11)]: () =>
        paged(
          {
            total_count: 3,
            repositories: [
              repo("octo/alpha", "2026-09-01T00:00:00Z"),
              repo("octo/Zeron-private", "2026-09-20T00:00:00Z", { private: true, default_branch: "trunk" })
            ]
          },
          installationRepos(11, 2)
        ),
      [installationRepos(11, 2)]: () => paged({ total_count: 3, repositories: [repo("octo/gamma", "2026-08-01T00:00:00Z")] }),
      [installationRepos(22)]: () =>
        paged({
          total_count: 2,
          repositories: [
            repo("octo/Zeron-private", "2026-09-20T00:00:00Z", { private: true, default_branch: "trunk" }),
            repo("zeron-org/desktop", "2026-09-25T00:00:00Z", { description: "Zeron desktop" })
          ]
        })
    });
    await connect(caller, device.deviceId);

    const repos = value(await vault().githubRepos(caller));
    expect(repos.map((r) => r.fullName)).toEqual(["zeron-org/desktop", "octo/Zeron-private", "octo/alpha", "octo/gamma"]);
    expect(repos[0]).toEqual({
      fullName: "zeron-org/desktop",
      cloneUrl: "https://github.com/zeron-org/desktop.git",
      defaultBranch: "main",
      private: false,
      description: "Zeron desktop",
      pushedAt: Date.parse("2026-09-25T00:00:00Z")
    });
    expect(repos[1]).toMatchObject({ private: true, defaultBranch: "trunk" });
    expect(repos[1]!.description).toBeUndefined();

    const apiCalls = upstream.calls.filter((call) => call.url.startsWith(`${API}/user/`));
    expect(apiCalls.map((call) => call.url)).toEqual([INSTALLATIONS, installationRepos(11), installationRepos(11, 2), installationRepos(22)]);
    for (const call of apiCalls) {
      expect(bearer(call)).toBe(`Bearer ${TOKEN}`);
      expect(new Headers(call.init?.headers).get("user-agent")).toBeTruthy();
    }
    expect(JSON.stringify(repos)).not.toContain(TOKEN);

    const filtered = value(await vault().githubRepos(caller, "ZER"));
    expect(filtered.map((r) => r.fullName)).toEqual(["zeron-org/desktop", "octo/Zeron-private"]);
  });

  it("lists nothing without an installation: never /user/repos, whose repos sessions can't reach", async () => {
    const { caller, device } = await setup();
    const upstream = mockUpstream({
      ...connectRoutes(),
      [INSTALLATIONS]: () => json({ total_count: 0, installations: [] })
    });
    await connect(caller, device.deviceId);
    expect(value(await vault().githubRepos(caller))).toEqual([]);
    expect(upstream.calls.some((call) => call.url.includes("/user/repos"))).toBe(false);
  });

  it("never follows a next link off api.github.com", async () => {
    const { caller, device } = await setup();
    const upstream = mockUpstream({
      ...connectRoutes(),
      [INSTALLATIONS]: () => json({ installations: [{ id: 7 }] }),
      [installationRepos(7)]: () =>
        paged({ repositories: [repo("octo/one", "2026-09-02T00:00:00Z")] }, "https://evil.example.com/steal?page=2")
    });
    await connect(caller, device.deviceId);
    expect(value(await vault().githubRepos(caller)).map((r) => r.fullName)).toEqual(["octo/one"]);
    expect(upstream.calls.some((call) => call.url.includes("evil.example.com"))).toBe(false);
  });

  it("refreshes a near-expiry token first, through the shared path", async () => {
    const { caller, device } = await setup();
    let tokenCalls = 0;
    const upstream = mockUpstream({
      ...connectRoutes(),
      [GITHUB_TOKEN_URL]: () =>
        ++tokenCalls === 1
          ? json({ access_token: "ghu_short", expires_in: 300, refresh_token: "ghr_1", refresh_token_expires_in: 15897600 })
          : json({ access_token: "ghu_refreshed", expires_in: 28800, refresh_token: "ghr_2", refresh_token_expires_in: 15897600 }),
      [INSTALLATIONS]: () => json({ installations: [] })
    });
    await connect(caller, device.deviceId);
    value(await vault().githubRepos(caller));
    expect(tokenCalls).toBe(2);
    expect(bearer(upstream.calls.find((call) => call.url === INSTALLATIONS)!)).toBe("Bearer ghu_refreshed");
  });

  it("a 401 from GitHub marks the connection needs_reconnect", async () => {
    const { caller, device } = await setup();
    mockUpstream({
      ...connectRoutes(),
      [INSTALLATIONS]: () => json({ message: "Bad credentials" }, 401)
    });
    await connect(caller, device.deviceId);
    expect(await vault().githubRepos(caller)).toMatchObject({ ok: false, error: "needs_reconnect", status: 409 });
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "github")?.status).toBe("needsReconnect");
  });

  it("other GitHub failures are upstream errors and change nothing", async () => {
    const { caller, device } = await setup();
    mockUpstream({ ...connectRoutes(), [INSTALLATIONS]: () => json({ message: "rate limited" }, 403) });
    await connect(caller, device.deviceId);
    expect(await vault().githubRepos(caller)).toMatchObject({ ok: false, error: "upstream", status: 502 });
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "github")?.status).toBe("connected");
  });

  it("is refused for runners, not_found when GitHub is not connected", async () => {
    const { userId, caller, device } = await setup();
    expect(await vault().githubRepos(runnerCaller(userId, device.deviceId))).toMatchObject({
      ok: false,
      error: "forbidden",
      status: 403
    });
    expect(await vault().githubRepos(caller)).toMatchObject({ ok: false, error: "not_found", status: 404 });
  });
});

describe("githubBranches", () => {
  const BRANCHES = `${API}/repos/octo/app/branches?per_page=100`;

  it("lists branch names across pages as the user, token never returned", async () => {
    const { caller, device } = await setup();
    const page2 = `${BRANCHES}&page=2`;
    const upstream = mockUpstream({
      ...connectRoutes(),
      [BRANCHES]: () => paged([{ name: "main" }, { name: "feature/x" }], page2),
      [page2]: () => paged([{ name: "release" }, { nope: true }])
    });
    await connect(caller, device.deviceId);
    const branches = value(await vault().githubBranches(caller, "octo/app"));
    expect(branches).toEqual(["main", "feature/x", "release"]);
    expect(bearer(upstream.calls.find((call) => call.url === BRANCHES)!)).toBe(`Bearer ${TOKEN}`);
    expect(JSON.stringify(branches)).not.toContain(TOKEN);
  });

  it("validates the repo, maps 404 and 401, and is refused for runners", async () => {
    const { userId, caller, device } = await setup();
    let status = 404;
    mockUpstream({ ...connectRoutes(), [BRANCHES]: () => json({ message: "Not Found" }, status) });
    await connect(caller, device.deviceId);
    expect(await vault().githubBranches(caller, "../etc")).toMatchObject({ ok: false, error: "bad_request" });
    expect(await vault().githubBranches(runnerCaller(userId, device.deviceId), "octo/app")).toMatchObject({
      ok: false,
      error: "forbidden"
    });
    expect(await vault().githubBranches(caller, "octo/app")).toMatchObject({ ok: false, error: "not_found" });
    status = 401;
    expect(await vault().githubBranches(caller, "octo/app")).toMatchObject({ ok: false, error: "needs_reconnect" });
  });
});
