import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { fromBase64, fromBase64Url, fromUtf8, utf8 } from "../../src/encoding";
import { accountStub } from "../../src/names";
import {
  connectGithub,
  githubConnectRoutes,
  json,
  mockUpstream,
  setup,
  signedGrant,
  value,
  vault,
  type UpstreamCall
} from "./helpers";

afterEach(() => vi.restoreAllMocks());

const API = "https://api.github.com";
const INSTALLATIONS = `${API}/user/installations?per_page=100`;
const USER_TOKEN = "ghu_user_SECRET_token_0001";
const mintUrl = (id: number) => `${API}/app/installations/${id}/access_tokens`;
const HOUR = 60 * 60_000;

const installation = (id: number, login: string, extra: Record<string, unknown> = {}) => ({
  id,
  account: { login, type: "Organization" },
  suspended_at: null,
  ...extra
});

const minted = (token: string, expiresAt = Date.now() + HOUR) =>
  json({ token, expires_at: new Date(expiresAt).toISOString(), permissions: { contents: "write" } }, 201);

const bearer = (call: UpstreamCall) => new Headers(call.init?.headers).get("authorization")?.replace(/^Bearer /, "");

/** Verify an App JWT against the test key and return its claims. */
const verifyAppJwt = async (jwt: string): Promise<Record<string, number | string>> => {
  const [header, claims, signature] = jwt.split(".");
  const spki = fromBase64(env.TEST_GITHUB_APP_PUBLIC_KEY.replace(/-----[A-Z ]+-----/g, "").replace(/\s+/g, ""));
  const key = await crypto.subtle.importKey("spki", spki, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, [
    "verify"
  ]);
  const valid = await crypto.subtle.verify(
    "RSASSA-PKCS1-v1_5",
    key,
    fromBase64Url(signature!),
    utf8(`${header}.${claims}`)
  );
  expect(valid).toBe(true);
  expect(JSON.parse(fromUtf8(fromBase64Url(header!)))).toEqual({ alg: "RS256", typ: "JWT" });
  return JSON.parse(fromUtf8(fromBase64Url(claims!)));
};

const tokenRows = (userId: string) =>
  runInDurableObject(accountStub(env, userId, "github"), (_instance, state) =>
    state.storage.sql
      .exec<{ installation_id: number; envelope: string; expires_at: number }>("SELECT * FROM installation_tokens")
      .toArray()
  );

/** A user whose GitHub is connected for one Cloud session device. */
const connected = async (routes: Parameters<typeof mockUpstream>[0]) => {
  const ctx = await setup("cloud");
  const upstream = mockUpstream({ ...githubConnectRoutes(), ...routes });
  await connectGithub(ctx.caller, ctx.device.deviceId);
  const grant = async (repo?: string) =>
    vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "github", undefined, repo));
  return { ...ctx, upstream, grant };
};

describe("github installation grants", () => {
  it("mint an App installation token for the repository's owner; the user token never leaves", async () => {
    const { upstream, grant } = await connected({
      [INSTALLATIONS]: () => json({ installations: [installation(11, "octo-org"), installation(12, "octocat")] }),
      [mintUrl(11)]: () => minted("ghs_install_eleven")
    });
    const before = Date.now();
    const result = value(await grant("Octo-Org/app"));
    expect(result).toMatchObject({ provider: "github", accessToken: "ghs_install_eleven", account: "octocat", generation: 1 });
    expect(result.expiresAt).toBeGreaterThan(before + HOUR - 60_000);
    expect(JSON.stringify(result)).not.toContain(USER_TOKEN);

    // The installation list is read AS the user (the ownership check)…
    const lookup = upstream.calls.find((call) => call.url === INSTALLATIONS)!;
    expect(bearer(lookup)).toBe(USER_TOKEN);
    // …and the token is minted AS the App, with a short-lived RS256 JWT.
    const mint = upstream.calls.find((call) => call.url === mintUrl(11))!;
    expect(mint.init?.method).toBe("POST");
    const claims = await verifyAppJwt(bearer(mint)!);
    expect(claims.iss).toBe("Iv1.testclient");
    expect(Number(claims.exp) - Number(claims.iat)).toBeLessThanOrEqual(600);
    expect(Number(claims.iat)).toBeLessThanOrEqual(Math.floor(Date.now() / 1000));
    expect(upstream.count(mintUrl(12))).toBe(0);
  });

  it("reuse a sealed token until 15 minutes before it expires, re-checking access every time", async () => {
    let mints = 0;
    const { userId, upstream, grant } = await connected({
      [INSTALLATIONS]: () => json({ installations: [installation(21, "octocat", { account: { login: "octocat" } })] }),
      [mintUrl(21)]: () => minted(`ghs_install_${++mints}`)
    });
    expect(value(await grant("octocat/one")).accessToken).toBe("ghs_install_1");
    expect(value(await grant("octocat/two")).accessToken).toBe("ghs_install_1");
    expect(mints).toBe(1);
    expect(upstream.count(INSTALLATIONS)).toBe(2);
    const [row] = await tokenRows(userId);
    expect(row?.installation_id).toBe(21);
    expect(row?.envelope).not.toContain("ghs_install_1");

    // Ten minutes left is too little to hand out again.
    await runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => {
      state.storage.sql.exec("UPDATE installation_tokens SET expires_at = ?", Date.now() + 10 * 60_000);
    });
    expect(value(await grant("octocat/one")).accessToken).toBe("ghs_install_2");
    expect(mints).toBe(2);
  });

  it("refuse an owner the App isn't installed on (or is suspended on), naming the install link", async () => {
    const { upstream, grant } = await connected({
      [INSTALLATIONS]: () =>
        json({ installations: [installation(31, "elsewhere"), installation(32, "acme", { suspended_at: "2026-09-01T00:00:00Z" })] })
    });
    for (const repo of ["octocat/app", "acme/app"]) {
      const result = await grant(repo);
      expect(result).toMatchObject({ ok: false, error: "not_found", status: 404 });
      expect(!result.ok && result.message).toContain("github_app_not_installed");
      expect(!result.ok && result.message).toContain("https://github.com/apps/zeron-test/installations/new");
    }
    expect(upstream.calls.some((call) => call.url.includes("/access_tokens"))).toBe(false);
  });

  it("need a well-formed repository, checked before the device's ts is spent", async () => {
    const { userId, caller, device } = await connected({
      [INSTALLATIONS]: () => json({ installations: [] })
    });
    const ts = Date.now();
    const at = (repo?: string) => signedGrant(device, userId, "github", ts, repo);
    expect(await vault().grant(caller, await at())).toMatchObject({ ok: false, error: "bad_request" });
    for (const repo of ["octocat", "../x", "octocat/app/extra", "-bad/app"]) {
      expect(await vault().grant(caller, await at(repo))).toMatchObject({ ok: false, error: "bad_request" });
    }
    // None of those spent `ts`: the same ts still passes the replay fence.
    expect(await vault().grant(caller, await at("octocat/app"))).toMatchObject({ ok: false, error: "not_found" });
  });

  it("a 401 on the installation lookup means the user must reconnect", async () => {
    const { caller, grant } = await connected({
      [INSTALLATIONS]: () => json({ message: "Bad credentials" }, 401)
    });
    expect(await grant("octocat/app")).toMatchObject({ ok: false, error: "needs_reconnect", status: 409 });
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "github")?.status).toBe("needsReconnect");
  });

  it("an uninstalled App is not_found; rejected App credentials are ours and leave the user connected", async () => {
    let mintStatus = 404;
    const { caller, grant } = await connected({
      [INSTALLATIONS]: () => json({ installations: [installation(41, "octocat")] }),
      [mintUrl(41)]: () => json({ message: "nope" }, mintStatus)
    });
    expect(await grant("octocat/app")).toMatchObject({ ok: false, error: "not_found" });
    mintStatus = 401;
    expect(await grant("octocat/app")).toMatchObject({ ok: false, error: "unavailable", status: 503 });
    mintStatus = 500;
    expect(await grant("octocat/app")).toMatchObject({ ok: false, error: "upstream", status: 502 });
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "github")?.status).toBe("connected");
  });

  it("disconnecting GitHub drops the cached installation tokens", async () => {
    const { userId, caller, grant } = await connected({
      [INSTALLATIONS]: () => json({ installations: [installation(51, "octocat")] }),
      [mintUrl(51)]: () => minted("ghs_install_51")
    });
    value(await grant("octocat/app"));
    expect(await tokenRows(userId)).toHaveLength(1);
    value(await vault().disconnect(caller, "github"));
    expect(await tokenRows(userId)).toHaveLength(0);
  });

  it("status carries the App's install link", async () => {
    const { caller } = await setup();
    expect(value(await vault().status(caller)).githubInstallUrl).toBe(
      "https://github.com/apps/zeron-test/installations/new"
    );
  });
});
