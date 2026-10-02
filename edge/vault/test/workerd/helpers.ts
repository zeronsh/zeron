import { env } from "cloudflare:workers";
import { expect, vi } from "vitest";
import type { GrantRequest, VaultCaller, VaultProviderId, VaultResult } from "../../src/api";
import { GITHUB_DEVICE_CODE_URL, GITHUB_TOKEN_URL, GITHUB_USER_URL } from "../../src/providers/github";
import type { Env } from "../../src/env";
import { Vault } from "../../src/vault";
import { fakeJwt, makeDevice, type TestDevice } from "../support";

export const DAY = 24 * 60 * 60_000;
export const sec = (ms: number) => Math.floor(ms / 1000);

export const vault = (overrides: Partial<Env> = {}) => new Vault({ ...env, ...overrides });

export const userCaller = (userId: string): VaultCaller => ({ userId, orgId: "org_test", kind: "user" });
export const runnerCaller = (userId: string, deviceId: string): VaultCaller => ({
  userId,
  orgId: "org_test",
  kind: "runner",
  deviceId
});

/** Unwrap a VaultResult, failing the test with the refusal when it is one. */
export const value = <T>(result: VaultResult<T>): T => {
  if (!result.ok) throw new Error(`expected ok, got ${result.error} (${result.status}): ${result.message}`);
  return result.value;
};

let lastTs = 0;
/** Strictly increasing per test run, like a real consumer's ts source. */
export const nextTs = (): number => (lastTs = Math.max(Date.now(), lastTs + 1));

export const signedGrant = async (
  device: TestDevice,
  userId: string,
  provider: VaultProviderId,
  ts = nextTs(),
  repo?: string
): Promise<GrantRequest> => ({
  provider,
  deviceId: device.deviceId,
  ts,
  sig: await device.grantSig(userId, provider, ts),
  ...(repo !== undefined ? { repo } : {})
});

/** A fresh user with one enrolled laptop. */
export const setup = async (kind: "laptop" | "cloud" = "laptop") => {
  const userId = `user_${crypto.randomUUID()}`;
  const caller = userCaller(userId);
  const device = await makeDevice(`${kind}-${crypto.randomUUID()}`);
  value(await vault().enrollDevice(caller, { deviceId: device.deviceId, kind, publicKey: device.publicKey }));
  return { userId, caller, device };
};

export const enroll = async (caller: VaultCaller, kind: "laptop" | "cloud" = "laptop") => {
  const device = await makeDevice(`${kind}-${crypto.randomUUID()}`);
  value(await vault().enrollDevice(caller, { deviceId: device.deviceId, kind, publicKey: device.publicKey }));
  return device;
};

export interface CodexTokens {
  readonly id_token: string;
  readonly access_token: string;
  readonly refresh_token: string;
  readonly account_id: string;
}

/** A `codex login` auth.json whose access token expires at `accessExpMs`. */
export const codexTokens = (accessExpMs: number): CodexTokens => ({
  id_token: fakeJwt({ email: "ada@example.com", nonce: crypto.randomUUID() }),
  access_token: fakeJwt({ exp: sec(accessExpMs), jti: crypto.randomUUID() }),
  refresh_token: `rt-${crypto.randomUUID()}`,
  account_id: "acct-ada"
});

export const codexMaterial = (tokens: CodexTokens) => ({
  authJson: { OPENAI_API_KEY: null, tokens, last_refresh: new Date().toISOString() }
});

export const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

export interface UpstreamCall {
  readonly url: string;
  readonly init: RequestInit | undefined;
  readonly body: string;
}

/**
 * Stub every upstream the vault may call. The DOs run in the test isolate, so
 * a spy on the global `fetch` intercepts their provider calls; an unexpected
 * URL fails loudly instead of leaving the sandbox.
 */
export const mockUpstream = (routes: Record<string, (call: UpstreamCall) => Response | Promise<Response>>) => {
  const calls: UpstreamCall[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const call = { url, init, body: typeof init?.body === "string" ? init.body : "" };
    calls.push(call);
    const route = routes[url];
    if (!route) throw new Error(`unexpected upstream call ${url}`);
    return route(call);
  });
  return { calls, count: (url: string) => calls.filter((call) => call.url === url).length };
};

/** Upstream routes for connecting GitHub through the device flow. */
export const githubConnectRoutes = (
  token: Record<string, unknown> = { access_token: "ghu_user_SECRET_token_0001", token_type: "bearer" }
) => ({
  [GITHUB_DEVICE_CODE_URL]: () =>
    json({ device_code: "dc", user_code: "ABCD-EFGH", verification_uri: "https://github.com/login/device", expires_in: 900, interval: 5 }),
  [GITHUB_TOKEN_URL]: () => json(token),
  [GITHUB_USER_URL]: () => json({ login: "octocat" })
});

/** Connect GitHub for `authorizedDevices` (routes from `githubConnectRoutes`). */
export const connectGithub = async (caller: VaultCaller, ...authorizedDevices: string[]) => {
  const start = value(await vault().githubDeviceStart(caller, authorizedDevices));
  expect(value(await vault().githubDevicePoll(caller, start.flowId)).state).toBe("connected");
};
