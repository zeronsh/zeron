import { env, exports } from "cloudflare:workers";
import { createExecutionContext, createScheduledController, runInDurableObject, waitOnExecutionContext } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import worker from "../../src/index";
import { runCanary } from "../../src/canary";
import { accountStub, devicesStub } from "../../src/names";
import { VAULT_PROVIDERS } from "../../src/api";
import { CODEX_TOKEN_URL } from "../../src/providers/codex";
import { GITHUB_DEVICE_CODE_URL, GITHUB_TOKEN_URL, GITHUB_USER_URL } from "../../src/providers/github";
import { fakeJwt } from "../support";
import { DAY, codexMaterial, codexTokens, json, mockUpstream, sec, setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

/** Every table of an object, flattened to one string. */
const dumpStorage = (stub: DurableObjectStub) =>
  runInDurableObject(stub, (_instance, state) => {
    const tables = state.storage.sql
      .exec<{ name: string }>("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '\\_%' ESCAPE '\\' AND name NOT LIKE 'sqlite_%'")
      .toArray();
    return JSON.stringify(tables.map(({ name }) => state.storage.sql.exec(`SELECT * FROM "${name}"`).toArray()));
  });

describe("status", () => {
  it("never contains secret material (nor does the audit log or storage)", async () => {
    const { userId, caller, device } = await setup();
    const codex = codexTokens(Date.now() + 5 * DAY);
    const anthropicKey = "sk-ant-api03-SECRETSECRETSECRET-wxyz";
    const openaiKey = "sk-proj-SECRETSECRETSECRET-9876";
    const github = {
      access: "ghu_SECRETSECRETSECRET_gh",
      refresh: "ghr_SECRETSECRETSECRET_gh",
      installation: "ghs_SECRETSECRETSECRET_inst"
    };
    const deviceCode = "dc_SECRETSECRETSECRET_flow";
    const claude = {
      accessToken: "sk-ant-oat01-SECRETSECRETSECRET-access",
      refreshToken: "sk-ant-ort01-SECRETSECRETSECRET-refresh",
      expiresAt: Date.now() + 8 * 60 * 60_000,
      scopes: ["user:inference", "user:profile"],
      subscriptionType: "max"
    };
    mockUpstream({
      [GITHUB_DEVICE_CODE_URL]: () =>
        json({ device_code: deviceCode, user_code: "ABCD-EFGH", verification_uri: "https://github.com/login/device", expires_in: 900, interval: 5 }),
      [GITHUB_TOKEN_URL]: () =>
        json({ access_token: github.access, expires_in: 28800, refresh_token: github.refresh, refresh_token_expires_in: 15897600 }),
      [GITHUB_USER_URL]: () => json({ login: "octocat" }),
      "https://api.github.com/user/installations?per_page=100": () =>
        json({ installations: [{ id: 9, account: { login: "octocat" }, suspended_at: null }] }),
      "https://api.github.com/app/installations/9/access_tokens": () =>
        json({ token: github.installation, expires_at: new Date(Date.now() + 60 * 60_000).toISOString() }, 201)
    });

    const devices = [device.deviceId];
    value(await vault().putCredential(caller, "codex", { material: codexMaterial(codex), authorizedDevices: devices }));
    value(await vault().putCredential(caller, "anthropic-key", { material: { key: anthropicKey }, authorizedDevices: devices }));
    value(await vault().putCredential(caller, "openai-key", { material: { key: openaiKey }, authorizedDevices: devices }));
    value(await vault().putCredential(caller, "claude", { material: { claudeAiOauth: claude }, authorizedDevices: devices }));
    const start = value(await vault().githubDeviceStart(caller, devices));
    expect(value(await vault().githubDevicePoll(caller, start.flowId)).state).toBe("connected");
    for (const provider of VAULT_PROVIDERS) {
      const repo = provider === "github" ? "octocat/app" : undefined;
      value(await vault().grant(caller, await signedGrant(device, userId, provider, undefined, repo)));
    }

    const status = value(await vault().status(caller));
    expect(status.available).toBe(true);
    expect(status.devices).toEqual([expect.objectContaining({ deviceId: device.deviceId, kind: "laptop" })]);
    expect(Object.fromEntries(status.connections.map((c) => [c.provider, c.account]))).toEqual({
      codex: "ada@example.com",
      claude: "max",
      github: "octocat",
      "anthropic-key": "…wxyz",
      "openai-key": "…9876"
    });

    const secrets = [
      codex.access_token,
      codex.refresh_token,
      codex.id_token,
      anthropicKey,
      openaiKey,
      github.access,
      github.refresh,
      github.installation,
      deviceCode,
      claude.accessToken,
      claude.refreshToken,
      "SECRETSECRETSECRET"
    ];
    const audit = value(await devicesStub(env, userId).auditLog(userId));
    expect(audit.map((entry) => entry.event)).toEqual(
      expect.arrayContaining(["enroll", "upload", "github_connect", "grant"])
    );
    const surfaces: Record<string, string> = {
      status: JSON.stringify(status),
      viaEntrypoint: JSON.stringify(await exports.VaultApi.status(caller)),
      audit: JSON.stringify(audit),
      devicesStorage: await dumpStorage(devicesStub(env, userId)),
      ...Object.fromEntries(
        await Promise.all(
          VAULT_PROVIDERS.map(async (provider) => [`${provider}Storage`, await dumpStorage(accountStub(env, userId, provider))])
        )
      )
    };
    for (const [surface, text] of Object.entries(surfaces)) {
      for (const secret of secrets) expect(text.includes(secret), `${surface} leaks ${secret.slice(0, 12)}…`).toBe(false);
    }
  });

  it("disconnect wipes the record; generations never repeat", async () => {
    const { userId, caller, device } = await setup();
    const put = () =>
      vault().putCredential(caller, "openai-key", { material: { key: "sk-proj-abcdefgh1234" }, authorizedDevices: [device.deviceId] });
    value(await put());
    const after = value(await vault().disconnect(caller, "openai-key"));
    expect(after.connections.find((c) => c.provider === "openai-key")).toBeUndefined();
    expect(await vault().grant(caller, await signedGrant(device, userId, "openai-key"))).toMatchObject({
      ok: false,
      error: "not_found"
    });
    value(await put());
    expect(value(await vault().grant(caller, await signedGrant(device, userId, "openai-key"))).generation).toBe(2);
    value(await vault().disconnect(caller, "openai-key"));
    value(await vault().disconnect(caller, "openai-key"));
  });
});

describe("worker surface", () => {
  it("has no HTTP surface", async () => {
    for (const path of ["/", "/vault/org/grant", "/status"]) {
      const response = await exports.default.fetch(`https://vault.internal${path}`, { method: "POST" });
      expect(response.status).toBe(404);
    }
  });
});

describe("canary", () => {
  it("force-refreshes codex, reports missing claude/github, checks the GitHub App, records and logs results", async () => {
    const { userId, caller, device } = await setup();
    value(
      await vault().putCredential(caller, "codex", {
        material: codexMaterial(codexTokens(Date.now() + 5 * DAY)),
        authorizedDevices: [device.deviceId]
      })
    );
    const fresh = fakeJwt({ exp: sec(Date.now() + 10 * DAY), jti: "canary" });
    const upstream = mockUpstream({
      [CODEX_TOKEN_URL]: () => json({ access_token: fresh }),
      "https://api.github.com/app": () => json({ slug: "zeron-test" })
    });
    const logs: string[] = [];
    vi.spyOn(console, "log").mockImplementation((line: unknown) => void logs.push(String(line)));

    const results = await runCanary({ ...env, CANARY_USER_ID: userId });
    expect(results.map(({ provider, ok, status }) => ({ provider, ok, status }))).toEqual([
      { provider: "codex", ok: true, status: "refreshed" },
      { provider: "claude", ok: false, status: "missing" },
      { provider: "github", ok: false, status: "missing" },
      { provider: "github", ok: true, status: "app_valid" }
    ]);
    expect(upstream.count(CODEX_TOKEN_URL)).toBe(1);
    const appCall = upstream.calls.find((call) => call.url === "https://api.github.com/app")!;
    expect(new Headers(appCall.init?.headers).get("authorization")).toMatch(/^Bearer ey[\w-]+\.[\w-]+\.[\w-]+$/);
    const canaryLines = logs.map((line) => JSON.parse(line)).filter((entry) => entry.event === "vault.canary");
    expect(canaryLines).toEqual([
      { event: "vault.canary", provider: "codex", ok: true, status: "refreshed" },
      { event: "vault.canary", provider: "claude", ok: false, status: "missing" },
      { event: "vault.canary", provider: "github", ok: false, status: "missing" },
      { event: "vault.canary", provider: "github", ok: true, status: "app_valid" }
    ]);
    expect(value(await accountStub(env, userId, "codex").canaryLog(userId, "codex"))[0]).toMatchObject({
      ok: true,
      status: "refreshed"
    });
    // The canary's refresh is a real generation bump.
    expect(value(await vault().grant(caller, await signedGrant(device, userId, "codex")))).toMatchObject({
      accessToken: fresh,
      generation: 2
    });
  });

  it("the cron handler is a no-op without CANARY_USER_ID", async () => {
    const logs: string[] = [];
    vi.spyOn(console, "log").mockImplementation((line: unknown) => void logs.push(String(line)));
    const ctx = createExecutionContext();
    await worker.scheduled(createScheduledController({ cron: "0 6 * * *", scheduledTime: Date.now() }), env, ctx);
    await waitOnExecutionContext(ctx);
    expect(logs.map((line) => JSON.parse(line))).toContainEqual({
      event: "vault.canary",
      skipped: "CANARY_USER_ID not set"
    });
  });
});
