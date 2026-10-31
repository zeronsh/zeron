import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { accountStub } from "../../src/names";
import { DEVICE_GRANT_TYPE, GITHUB_DEVICE_CODE_URL, GITHUB_TOKEN_URL, GITHUB_USER_URL } from "../../src/providers/github";
import { json, mockUpstream, setup, signedGrant, value, vault, type UpstreamCall } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const DEVICE_CODE = "dc-secret-3584d83530557fdd1f46af8289938c8ef79f9dc5";

const deviceCodeRoute = () =>
  json({
    device_code: DEVICE_CODE,
    user_code: "WDJB-MJHT",
    verification_uri: "https://github.com/login/device",
    expires_in: 900,
    interval: 5
  });

/** Make the flow's next poll due now (instead of sleeping out GitHub's interval). */
const rewindPoll = (userId: string, flowId: string) =>
  runInDurableObject(accountStub(env, userId, "github"), (_instance, state) => {
    state.storage.sql.exec("UPDATE github_flows SET next_poll_at = 0 WHERE flow_id = ?", flowId);
  });

const flowRow = (userId: string, flowId: string) =>
  runInDurableObject(accountStub(env, userId, "github"), (_instance, state) =>
    state.storage.sql
      .exec<{ interval_secs: number; envelope: string }>(
        "SELECT interval_secs, envelope FROM github_flows WHERE flow_id = ?",
        flowId
      )
      .toArray()[0]
  );

const form = (call: UpstreamCall) => Object.fromEntries(new URLSearchParams(call.body));

const INSTALLATIONS = "https://api.github.com/user/installations?per_page=100";

/** The App is installed on `login` (installation 1) and mints `ghs_installation`. */
const installedOn = (login: string) => ({
  [INSTALLATIONS]: () => json({ installations: [{ id: 1, account: { login }, suspended_at: null }] }),
  "https://api.github.com/app/installations/1/access_tokens": () =>
    json({ token: "ghs_installation", expires_at: new Date(Date.now() + 60 * 60_000).toISOString() }, 201)
});

describe("github device flow", () => {
  it("goes pending → connected and the vault stores the token", async () => {
    const { userId, caller, device } = await setup();
    const answers = [
      () => json({ error: "authorization_pending" }),
      () =>
        json({
          access_token: "ghu_live_token_0123456789",
          expires_in: 28800,
          refresh_token: "ghr_live_refresh_0123456789",
          refresh_token_expires_in: 15897600,
          token_type: "bearer",
          scope: ""
        })
    ];
    const upstream = mockUpstream({
      [GITHUB_DEVICE_CODE_URL]: deviceCodeRoute,
      [GITHUB_TOKEN_URL]: () => answers.shift()!(),
      [GITHUB_USER_URL]: () => json({ login: "octocat", id: 1 })
    });

    const start = value(await vault().githubDeviceStart(caller, [device.deviceId]));
    expect(start).toMatchObject({ userCode: "WDJB-MJHT", verificationUri: "https://github.com/login/device", intervalSecs: 5 });
    expect(start.flowId).toMatch(/^[A-Za-z0-9_-]{22}$/);
    expect(form(upstream.calls[0]!)).toEqual({ client_id: "Iv1.testclient" });
    // The device code is sealed at rest like any credential.
    expect((await flowRow(userId, start.flowId))?.envelope).not.toContain(DEVICE_CODE);

    expect(value(await vault().githubDevicePoll(caller, start.flowId))).toEqual({ state: "pending" });
    expect(upstream.count(GITHUB_TOKEN_URL)).toBe(1);
    expect(form(upstream.calls[1]!)).toEqual({
      client_id: "Iv1.testclient",
      device_code: DEVICE_CODE,
      grant_type: DEVICE_GRANT_TYPE
    });

    // Polling faster than GitHub's interval is answered locally.
    expect(value(await vault().githubDevicePoll(caller, start.flowId))).toEqual({ state: "pending" });
    expect(upstream.count(GITHUB_TOKEN_URL)).toBe(1);

    await rewindPoll(userId, start.flowId);
    expect(value(await vault().githubDevicePoll(caller, start.flowId))).toEqual({ state: "connected", account: "octocat" });
    const userCall = upstream.calls.find((call) => call.url === GITHUB_USER_URL)!;
    const headers = new Headers(userCall.init?.headers);
    expect(headers.get("user-agent")).toBeTruthy();
    expect(headers.get("authorization")).toBe("Bearer ghu_live_token_0123456789");

    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "github")).toMatchObject({
      status: "connected",
      account: "octocat",
      authorizedDevices: [device.deviceId]
    });
    // The user token itself is never granted: a GitHub grant must name a
    // repository and is an App installation token (github-app.workerd.test.ts).
    expect(await vault().grant(caller, await signedGrant(device, userId, "github"))).toMatchObject({
      ok: false,
      error: "bad_request"
    });

    // The flow is spent.
    expect(await vault().githubDevicePoll(caller, start.flowId)).toMatchObject({ ok: false, error: "not_found" });
  });

  it("slow_down widens the interval; access_denied fails the flow", async () => {
    const { userId, caller } = await setup();
    const answers = [() => json({ error: "slow_down", interval: 10 }), () => json({ error: "access_denied" })];
    mockUpstream({
      [GITHUB_DEVICE_CODE_URL]: deviceCodeRoute,
      [GITHUB_TOKEN_URL]: () => answers.shift()!()
    });
    const start = value(await vault().githubDeviceStart(caller, []));
    expect(value(await vault().githubDevicePoll(caller, start.flowId))).toEqual({ state: "pending" });
    expect((await flowRow(userId, start.flowId))?.interval_secs).toBe(10);
    await rewindPoll(userId, start.flowId);
    expect(value(await vault().githubDevicePoll(caller, start.flowId))).toEqual({
      state: "failed",
      error: "access_denied"
    });
    expect(await flowRow(userId, start.flowId)).toBeUndefined();
  });

  it("refreshes an expiring GitHub token with the App secret", async () => {
    const { userId, caller, device } = await setup();
    const answers = [
      () => json({ access_token: "ghu_short", expires_in: 300, refresh_token: "ghr_first", refresh_token_expires_in: 15897600 }),
      () => json({ access_token: "ghu_refreshed", expires_in: 28800, refresh_token: "ghr_second", refresh_token_expires_in: 15897600 })
    ];
    const upstream = mockUpstream({
      [GITHUB_DEVICE_CODE_URL]: deviceCodeRoute,
      [GITHUB_TOKEN_URL]: () => answers.shift()!(),
      [GITHUB_USER_URL]: () => json({ login: "octocat" }),
      ...installedOn("octocat")
    });
    const start = value(await vault().githubDeviceStart(caller, [device.deviceId]));
    expect(value(await vault().githubDevicePoll(caller, start.flowId)).state).toBe("connected");

    // 5 minutes left < the 10-minute refresh window: the installation lookup
    // goes out with the refreshed token.
    const grant = value(await vault().grant(caller, await signedGrant(device, userId, "github", undefined, "octocat/app")));
    expect(grant).toMatchObject({ accessToken: "ghs_installation", generation: 2, account: "octocat" });
    const lookup = upstream.calls.find((call) => call.url === INSTALLATIONS)!;
    expect(new Headers(lookup.init?.headers).get("authorization")).toBe("Bearer ghu_refreshed");
    const refreshCall = upstream.calls.filter((call) => call.url === GITHUB_TOKEN_URL)[1]!;
    expect(form(refreshCall)).toEqual({
      client_id: "Iv1.testclient",
      client_secret: "test-client-secret",
      grant_type: "refresh_token",
      refresh_token: "ghr_first"
    });
  });

  it("a non-expiring token never refreshes", async () => {
    const { userId, caller, device } = await setup();
    const upstream = mockUpstream({
      [GITHUB_DEVICE_CODE_URL]: deviceCodeRoute,
      [GITHUB_TOKEN_URL]: () => json({ access_token: "gho_forever", token_type: "bearer" }),
      [GITHUB_USER_URL]: () => json({ login: "octocat" }),
      ...installedOn("octocat")
    });
    const start = value(await vault().githubDeviceStart(caller, [device.deviceId]));
    value(await vault().githubDevicePoll(caller, start.flowId));
    const grant = value(await vault().grant(caller, await signedGrant(device, userId, "github", undefined, "octocat/app")));
    expect(grant.accessToken).toBe("ghs_installation");
    expect(upstream.count(GITHUB_TOKEN_URL)).toBe(1);
    const lookup = upstream.calls.find((call) => call.url === INSTALLATIONS)!;
    expect(new Headers(lookup.init?.headers).get("authorization")).toBe("Bearer gho_forever");
  });
});
