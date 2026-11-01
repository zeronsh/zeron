import { chatRoute } from "../../src/chat-route";
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import { authenticate } from "../../src/auth";
import { handlePublicCardRoute, handleUsernameRoute } from "../../src/card-routes";
import { publicCardGate } from "../../src/card-gate";
import { handleAdminRoute } from "../../src/cloud/billing";
import {
  autoWakeCloudDevice,
  handleBillingPublicRoute,
  handleCloudRoute,
  handleRunnerPublicRoute
} from "../../src/cloud/routes";
import type { Env } from "../../src/env";
import { deviceParam, forward } from "../../src/forward";
import { runnerChatGate, runnerDeviceGate } from "../../src/runner-access";
import { previewRoute } from "../../src/preview-route";
import { releaseRoute } from "../../src/releases";
import { runnerRefusal } from "../../src/runner-policy";
import { handleVaultRoute } from "../../src/vault-routes";
import { handleUserDirectoryRoute } from "../../src/user-directory";
export { DeviceRoom } from "../../src/device-room";
export { ChatRoom } from "../../src/chat-room";
export { PreviewRoom } from "../../src/preview-room";
export { RegistryRoom } from "../../src/registry-room";
export { CloudAccount } from "../../src/cloud/cloud-account";
export { CloudIndex } from "../../src/cloud/cloud-index";
export { UserDirectory } from "../../src/user-directory";
export { DeleteWorkflow, ProvisionWorkflow, SleepWorkflow, WakeWorkflow } from "../../src/cloud/workflows";

/** Bare SQLite-backed DO; tests reach its real `ctx.storage.sql` via
 * `runInDurableObject` (the cloudflare-os TEST_OVERSEER pattern). */
export class TestLogRoom extends DurableObject {}

// ── VAULT stand-in: the `VaultApi` RPC surface, recording every call.
//    Behavior keys off the caller's user id so tests can pick an outcome.
type Caller = { userId: string; orgId: string; kind: string; deviceId?: string };
const vaultCalls: { method: string; args: unknown[] }[] = [];
const STATUS = { connections: [], devices: [], available: true };
const rebindConnections = new Map<string, Map<string, string[]>>();
const ACCOUNTS = [{ slot: "a1", status: "connected", active: true, kind: "oauth", account: "me@example.com", authorizedDevices: ["cloud-1"], createdAt: 1, updatedAt: 1 }];

export class VaultStub extends WorkerEntrypoint {
  private answer(method: string, args: unknown[], value: unknown) {
    vaultCalls.push({ method, args });
    const caller = args[0] as Caller;
    if (caller.userId.startsWith("vault-throw")) throw new Error("vault exploded");
    if (caller.userId.startsWith("vault-fail")) {
      return { ok: false, error: "needs_reconnect", message: "Reconnect Codex.", status: 409 };
    }
    if (caller.userId.startsWith("vault-weird")) return { ok: false, error: "upstream", message: "odd", status: 200 };
    if (caller.userId.startsWith("vault-down")) return { ok: false, error: "unavailable", message: "KMS down", status: 503 };
    return { ok: true, value };
  }
  /** `rebind*` users have connections that remember what they're authorized for. */
  private connections(caller: Caller) {
    if (!caller.userId.startsWith("rebind")) return STATUS;
    const mine = rebindConnections.get(caller.userId) ?? new Map<string, string[]>();
    return {
      ...STATUS,
      connections: [...mine].map(([provider, authorizedDevices]) => ({ provider, status: "connected", authorizedDevices, updatedAt: 1 }))
    };
  }
  status(caller: Caller) { return this.answer("status", [caller], this.connections(caller)); }
  putCredential(caller: Caller, provider: string, req: unknown) { return this.answer("putCredential", [caller, provider, req], STATUS); }
  authorize(caller: Caller, provider: string, devices: string[]) {
    if (caller.userId.startsWith("rebind")) {
      const mine = rebindConnections.get(caller.userId) ?? new Map<string, string[]>();
      mine.set(provider, devices);
      rebindConnections.set(caller.userId, mine);
    }
    return this.answer("authorize", [caller, provider, devices], this.connections(caller));
  }
  disconnect(caller: Caller, provider: string) { return this.answer("disconnect", [caller, provider], STATUS); }
  enrollDevice(caller: Caller, req: unknown) { return this.answer("enrollDevice", [caller, req], STATUS); }
  revokeDevice(caller: Caller, deviceId: string) { return this.answer("revokeDevice", [caller, deviceId], STATUS); }
  setDisabled(caller: Caller, disabled: boolean) { return this.answer("setDisabled", [caller, disabled], STATUS); }
  grant(caller: Caller, req: { provider: string }) {
    return this.answer("grant", [caller, req], { provider: req.provider, accessToken: "tok", expiresAt: 1, generation: 1 });
  }
  githubDeviceStart(caller: Caller, devices: string[]) {
    return this.answer("githubDeviceStart", [caller, devices], { flowId: "f1", userCode: "ABCD-1234", verificationUri: "https://github.com/login/device", intervalSecs: 5, expiresAt: 1 });
  }
  githubDevicePoll(caller: Caller, flowId: string) { return this.answer("githubDevicePoll", [caller, flowId], { state: "pending" }); }
  githubRepos(caller: Caller, query?: string) {
    return this.answer("githubRepos", [caller, query], [
      { fullName: "acme/app", cloneUrl: "https://github.com/acme/app.git", defaultBranch: "main", private: true, pushedAt: 2 }
    ]);
  }
  githubBranches(caller: Caller, repo: string) {
    return this.answer("githubBranches", [caller, repo], ["main", "dev"]);
  }
  accounts(caller: Caller, provider: string) { return this.answer("accounts", [caller, provider], ACCOUNTS); }
  activateAccount(caller: Caller, provider: string, slot: string | null) {
    return this.answer("activateAccount", [caller, provider, slot], ACCOUNTS);
  }
  forgetAccount(caller: Caller, provider: string, slot: string) { return this.answer("forgetAccount", [caller, provider, slot], []); }
  accountUsage(caller: Caller, provider: string, slot: string) {
    return this.answer("accountUsage", [caller, provider, slot], { body: { five_hour: { utilization: 10 } }, fetchedAt: 1 });
  }
  calls() { return vaultCalls; }
}

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });

/**
 * The production routing seam, in src/index.ts's order, minus the routes
 * that need loro's WASM (which cannot load in this tier): releases, admin + runner
 * pre-bearer routes, bearer gate, runner policy, /cloud, /vault, preview,
 * /device (with Cloud auto-wake), /registry, /chat2 and /blob (with the
 * runner chat gate) — all through the production `forward`.
 */
export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);
    const release = await releaseRoute(request, env, url);
    if (release) return release;
    const admin = await handleAdminRoute(request, env, url);
    if (admin) return admin;
    const billing = await handleBillingPublicRoute(request, env, url);
    if (billing) return billing;
    const runner = await handleRunnerPublicRoute(request, env, url);
    if (runner) return runner;
    const users = await handleUserDirectoryRoute(request, env, url);
    if (users) return users;
    const card = await publicCardGate(request, env, url, () => handlePublicCardRoute(request, env, ctx, url));
    if (card) return card;
    const auth = await authenticate(env, request);
    if (!auth) return new Response("Unauthorized", { status: 401 });
    const refused = runnerRefusal(auth, request.method, url);
    if (refused) return json({ error: "forbidden", message: refused }, 403);
    const username = await handleUsernameRoute(request, env, auth, url);
    if (username) return username;
    const cloud = await handleCloudRoute(request, env, auth, url);
    if (cloud) return cloud;
    const vault = await handleVaultRoute(request, env, auth, url);
    if (vault) return vault;
    const preview = previewRoute(request, env, auth);
    if (preview) return preview;
    const parts = url.pathname.split("/").filter(Boolean);
    const id = /^[A-Za-z0-9_-]{1,128}$/;
    if (parts[0] === "device" && parts[1] && id.test(parts[1])) {
      const deviceId = parts[1];
      const room = `d2/${deviceId}`;
      if (parts[2] === "ws" || parts[2] === "nudge") {
        const refusedDevice = await runnerDeviceGate(env, auth, deviceId);
        if (refusedDevice) return refusedDevice;
      }
      if (parts[2] === "ws") {
        const role = url.searchParams.get("role") === "host" ? "host" : "client";
        return forward(env.DEVICE_ROOMS, room, request, auth, "/ws", `?role=${role}&connId=${crypto.randomUUID()}`);
      }
      if (parts[2] === "nudge" && request.method === "POST") {
        autoWakeCloudDevice(env, ctx, auth, deviceId);
        return forward(env.DEVICE_ROOMS, room, request, auth, "/nudge", "");
      }
      if (parts[2] === "status") return forward(env.DEVICE_ROOMS, room, request, auth, "/status", "");
      if (parts[2] === "sidecar" && parts[3]) return forward(env.DEVICE_ROOMS, room, request, auth, `/sidecar/${parts[3]}`, "");
    }
    if (parts[0] === "registry" && parts[1] === auth.orgId) {
      const room = `reg1/${parts[1]}/${auth.userId}`;
      if (parts[2] === "ws") return forward(env.REGISTRY_ROOMS, room, request, auth, "/ws", `?${deviceParam(url).replace(/^&/, "")}`);
      if (parts[2] === "rows" || parts[2] === "push" || parts[2] === "stats") {
        return forward(env.REGISTRY_ROOMS, room, request, auth, `/${parts[2]}`, url.search);
      }
    }
    if (parts[0] === "chat2" && parts[1] && id.test(parts[1])) {
      const notHost = await runnerChatGate(env, auth, parts[1]);
      if (notHost) return notHost;
    }
    const chat = chatRoute(request, env, auth.userId);
    if (chat) return chat;
    if (parts[0] === "blob" && parts[1] && parts[2]) {
      const notHost = await runnerChatGate(env, auth, parts[1]);
      if (notHost) return notHost;
      const key = `blob/${auth.userId}/${parts[1]}/${parts[2]}`;
      if (request.method === "PUT") {
        await env.BLOBS.put(key, await request.arrayBuffer());
        return json({ ok: true });
      }
      const object = await env.BLOBS.get(key);
      return object ? new Response(object.body) : json({ error: "not_found" }, 404);
    }
    return new Response("test fixture", { status: 404 });
  }
};
