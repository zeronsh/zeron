import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { call, newUser, userBearer } from "./cloud-helpers";

const lastCall = async (method: string) => (await env.VAULT.calls()).filter((c) => c.method === method).at(-1)!;

describe("/vault routes → VAULT service binding", () => {
  it("maps each route to its RPC with the verified caller", async () => {
    const u = newUser();
    const bearer = userBearer(u);
    const caller = { userId: u.userId, orgId: "org1", kind: "user" };

    expect(await (await call("GET", "/vault/org1", bearer)).json()).toEqual({ connections: [], devices: [], available: true });
    expect((await lastCall("status")).args).toEqual([caller]);

    const put = { material: { claudeAiOauth: { accessToken: "a" } }, authorizedDevices: ["cloud-1"] };
    expect((await call("PUT", "/vault/org1/credentials/claude", bearer, put)).status).toBe(200);
    expect((await lastCall("putCredential")).args).toEqual([caller, "claude", put]);
    expect((await call("PATCH", "/vault/org1/credentials/codex", bearer, { authorizedDevices: ["cloud-1", "laptop-2"] })).status).toBe(200);
    expect((await lastCall("authorize")).args).toEqual([caller, "codex", ["cloud-1", "laptop-2"]]);
    expect((await call("DELETE", "/vault/org1/credentials/openai-key", bearer)).status).toBe(200);
    expect((await lastCall("disconnect")).args).toEqual([caller, "openai-key"]);

    const enroll = { deviceId: "laptop-2", kind: "laptop", publicKey: "a".repeat(43) };
    expect((await call("POST", "/vault/org1/devices/enroll", bearer, enroll)).status).toBe(200);
    expect((await lastCall("enrollDevice")).args).toEqual([caller, enroll]);
    expect((await call("POST", "/vault/org1/devices/laptop-2/revoke", bearer)).status).toBe(200);
    expect((await lastCall("revokeDevice")).args).toEqual([caller, "laptop-2"]);
    expect((await call("POST", "/vault/org1/disable", bearer, { disabled: true })).status).toBe(200);
    expect((await lastCall("setDisabled")).args).toEqual([caller, true]);

    const grant = { provider: "github", deviceId: "laptop-2", ts: 1_800_000_000_000, sig: "c2ln", repo: "octocat/app" };
    expect(await (await call("POST", "/vault/org1/grant", bearer, grant)).json()).toMatchObject({ provider: "github", accessToken: "tok" });
    expect((await lastCall("grant")).args).toEqual([caller, grant]);

    expect(await (await call("POST", "/vault/org1/github/device", bearer, { authorizedDevices: ["cloud-1"] })).json()).toMatchObject({ flowId: "f1" });
    expect((await call("POST", "/vault/org1/github/device/f1", bearer)).status).toBe(200);
    expect((await lastCall("githubDevicePoll")).args).toEqual([caller, "f1"]);

    expect(await (await call("GET", "/vault/org1/github/branches?repo=acme/app", bearer)).json()).toEqual({
      branches: ["main", "dev"]
    });
    expect((await lastCall("githubBranches")).args).toEqual([caller, "acme/app"]);
  });

  it("validates shapes before calling the vault", async () => {
    const bearer = userBearer(newUser());
    const bad = async (method: string, path: string, body?: unknown) =>
      expect((await call(method, path, bearer, body)).status, `${method} ${path}`).toBe(400);
    await bad("PUT", "/vault/org1/credentials/bogus", { material: {}, authorizedDevices: [] });
    await bad("PUT", "/vault/org1/credentials/codex", { material: {} });
    await bad("PUT", "/vault/org1/credentials/codex", { material: "x", authorizedDevices: [] });
    await bad("PATCH", "/vault/org1/credentials/codex", { authorizedDevices: ["bad id!"] });
    await bad("POST", "/vault/org1/devices/enroll", { deviceId: "cloud-1", kind: "laptop", publicKey: "a".repeat(43) });
    await bad("POST", "/vault/org1/devices/enroll", { deviceId: "laptop-1", kind: "cloud", publicKey: "a".repeat(43) });
    await bad("POST", "/vault/org1/disable", { disabled: "yes" });
    await bad("POST", "/vault/org1/grant", { provider: "nope", deviceId: "d", ts: 1, sig: "s" });
    await bad("POST", "/vault/org1/grant", { provider: "codex", deviceId: "d", ts: "1", sig: "s" });
    await bad("POST", "/vault/org1/grant", { provider: "github", deviceId: "d", ts: 1, sig: "c2ln", repo: 7 });
    await bad("GET", "/vault/org1/github/branches");
    await bad("PUT", "/vault/org1/credentials/codex", { material: { key: "x".repeat(70_000) }, authorizedDevices: [] });
    expect((await call("GET", "/vault/other-org", bearer)).status).toBe(403);
    expect((await call("GET", "/vault/org1/nope", bearer)).status).toBe(404);
  });

  it("translates vault results into HTTP status + {error, message}", async () => {
    const failing = await call("GET", "/vault/org1", userBearer(newUser("vault-fail")));
    expect(failing.status).toBe(409);
    expect(await failing.json()).toEqual({ error: "needs_reconnect", message: "Reconnect Codex." });

    const down = await call("GET", "/vault/org1", userBearer(newUser("vault-down")));
    expect(down.status).toBe(503);
    expect(await down.json()).toEqual({ error: "unavailable", message: "KMS down" });

    const thrown = await call("GET", "/vault/org1", userBearer(newUser("vault-throw")));
    expect(thrown.status).toBe(503);
    expect(await thrown.json()).toMatchObject({ error: "unavailable" });

    // A non-error status on a failed result is a contract bug: bad gateway.
    expect((await call("GET", "/vault/org1", userBearer(newUser("vault-weird")))).status).toBe(502);
  });
});
