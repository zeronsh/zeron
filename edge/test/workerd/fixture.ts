import { handleAttachmentBlob } from "../../src/attachment-blobs";
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
import { dialDeviceRoom } from "../../src/device-link";
import { deviceParam, forward } from "../../src/forward";
import { runnerChatGate, runnerDeviceGate } from "../../src/runner-access";
import type { RunnerDrop } from "../../src/runner-revocation";
import { previewRoute } from "../../src/preview-route";
import { releaseRoute } from "../../src/releases";
import { modelListRoute } from "../../src/model-list";
import { runnerRefusal } from "../../src/runner-policy";
import { handleVaultRoute } from "../../src/vault-routes";
import { handleUserDirectoryRoute } from "../../src/user-directory";
import { handleOnboardingRoute } from "../../src/onboarding-routes";
import { handlePluginKeyRoute } from "../../src/plugin-keys";
import { handleStoreRoute } from "../../src/store/routes";
import { pluginOwner } from "../../src/store/db";
import { handleWebAuthRoute } from "../../src/web-auth-routes";
import { handleRoleAdminRoute } from "../../src/roles";
import { handleDeployTokenAdminRoute, handleDeployTokenRoute } from "../../src/store/identity/deploy-tokens";
import { sha256Hex } from "../../src/store/identity/signing";
import { BodyRefused, authenticatePublisher, hasCredentials, bodyStream, optionalUser, readBody, requireAdmin, withSessionCookie } from "../../src/store/principal";
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
  githubListing(caller: Caller, query?: string) {
    return this.answer("githubListing", [caller, query], {
      repos: [
        { fullName: "acme/app", cloneUrl: "https://github.com/acme/app.git", defaultBranch: "main", private: true, pushedAt: 2, installationId: 7, account: "ada", accounts: ["ada"] }
      ],
      installations: [{ id: 7, target: "acme", targetType: "Organization", reachedBy: ["ada"] }]
    });
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

/**
 * The web site's server as the contract has it (amendment 8): it relays a
 * browser's request to the edge over its service binding, at an address of
 * its choosing, copying the browser's Cookie, Origin and X-CSRF-Token (and a
 * body with its type) verbatim and nothing else, and hands the edge's answer
 * (its Set-Cookie too) back to the browser.
 */
export class SiteStub extends WorkerEntrypoint<{ EDGE: Fetcher }> {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const headers = new Headers();
    // (`x-test-principal`: the tests' own marker for the principal probes.)
    for (const name of ["cookie", "origin", "x-csrf-token", "content-type", "x-test-principal"]) {
      const value = request.headers.get(name);
      if (value !== null) headers.set(name, value);
    }
    const hasBody = request.method !== "GET" && request.method !== "HEAD";
    return this.env.EDGE.fetch(`https://edge.internal${url.pathname}${url.search}`, {
      method: request.method,
      headers,
      body: hasBody ? await request.arrayBuffer() : undefined,
      redirect: "manual"
    });
  }
}

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });

/**
 * `POST /__test/owner {pluginId, userId}`: make `userId` own store id
 * `pluginId` in the real catalog (null: no one), so the identity tests'
 * deploy tokens meet the store's own `pluginOwner`: claimed now, or moved to
 * them now (`owner_since` as the store's transfer sets it), or at `at`. A plugin made here has no
 * version; its skeleton is its id behind a prefix no real id has, so it
 * never stands in a real publish's lookalike check.
 */
const setTestOwner = async (env: Env, pluginId: string, userId: string | null, at = Date.now()): Promise<void> => {
  const db = env.STORE_DB!;
  if (userId === null) {
    await db.prepare("DELETE FROM plugins WHERE id = ? AND NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?)").bind(pluginId, pluginId).run();
    return;
  }
  await db
    .prepare(
      `INSERT INTO plugins (id, owner_id, owner_since, skeleton, created_at) VALUES (?1, ?2, ?3, ?4, ?3)
       ON CONFLICT (id) DO UPDATE SET owner_id = excluded.owner_id, owner_since = MAX(?3, plugins.created_at + 1, plugins.owner_since + 1)
       WHERE plugins.owner_id <> excluded.owner_id`
    )
    .bind(pluginId, userId, at, `__test:${pluginId}`)
    .run();
};

/**
 * The identity lane's principal probes, reached only by a request carrying
 * `x-test-principal` (every other store request goes to the real routes):
 * each answers who its principal says is calling, on the real paths:
 * publisher routes (`POST /store/v1/plugins/:id/versions` and its version
 * actions, `GET /store/v1/me/plugins`) with the body's hash as the route
 * reads it after authentication (a deploy token where the real routes take
 * one: publishing, its last use that plugin), admin
 * routes (`/store/v1/admin/*`), and a public one (`/store/v1/public`).
 */
const storeStandIn = async (request: Request, env: Env, url: URL): Promise<Response | undefined> => {
  const path = url.pathname;
  if (path.startsWith("/store/v1/admin/")) {
    const admin = await requireAdmin(request, env);
    return admin instanceof Response ? admin : json({ admin });
  }
  if (path === "/store/v1/public") return json({ user: await optionalUser(request, env), credentials: hasCredentials(request) });
  const publishing = /^\/store\/v1\/plugins\/([^/]+)\/versions$/.exec(path);
  if (path === "/store/v1/me/plugins" || /^\/store\/v1\/plugins\/[^/]+\/versions(\/[^/]+\/[a-z]+)?$/.test(path)) {
    const tokens = publishing && request.method === "POST" ? { usedFor: publishing[1]! } : undefined;
    const publisher = await authenticatePublisher(request, env, tokens);
    if (publisher instanceof Response) return publisher;
    // `x-test-stream: 1`: read it as a package write would (bodyStream), not buffered.
    if (request.headers.get("x-test-stream") === "1") {
      try {
        const bytes = new Uint8Array(await new Response(bodyStream(request)).arrayBuffer());
        return json({ publisher, body: await sha256Hex(bytes) });
      } catch (e) {
        return json({ publisher, body: e instanceof BodyRefused ? e.reason : "error" });
      }
    }
    const body = await readBody(request);
    return json({ publisher, body: typeof body === "string" ? body : await sha256Hex(body) });
  }
  return undefined;
};

/**
 * The production routing seam, in src/index.ts's order, minus the routes
 * that need loro's WASM (which cannot load in this tier): releases, admin + runner
 * pre-bearer routes, the hosted model list, bearer gate, runner policy, /cloud, /vault, preview,
 * /device (with Cloud auto-wake), /registry, /chat2 and /blob (with the
 * runner gates) — all through the production `forward`.
 */
export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);
    const release = await releaseRoute(request, env, url);
    if (release) return release;
    const modelList = await modelListRoute(request, env, url);
    if (modelList) return modelList;
    const admin = await handleAdminRoute(request, env, url);
    if (admin) return admin;
    const billing = await handleBillingPublicRoute(request, env, url);
    if (billing) return billing;
    const runner = await handleRunnerPublicRoute(request, env, url);
    if (runner) return runner;
    const users = await handleUserDirectoryRoute(request, env, url);
    if (users) return users;
    const webAuth = await handleWebAuthRoute(request, env, url);
    if (webAuth) return webAuth;
    const roles = await handleRoleAdminRoute(request, env, url);
    if (roles) return roles;
    const deployTokenAdmin = await handleDeployTokenAdminRoute(request, env, url, pluginOwner);
    if (deployTokenAdmin) return deployTokenAdmin;
    if (url.pathname === "/__test/owner" && request.method === "POST") {
      const { pluginId, userId, at } = (await request.json()) as { pluginId: string; userId: string | null; at?: number };
      await setTestOwner(env, pluginId, userId, at);
      return json({ ok: true });
    }
    const deployTokens = await handleDeployTokenRoute(request, env, url, pluginOwner);
    if (deployTokens) return withSessionCookie(request, deployTokens);
    if (request.headers.has("x-test-principal")) {
      const probe = await storeStandIn(request, env, url);
      if (probe) return withSessionCookie(request, probe);
    }
    const card = await publicCardGate(request, env, url, () => handlePublicCardRoute(request, env, ctx, url));
    if (card) return card;
    const store = await handleStoreRoute(request, env, url);
    if (store) return store;
    const auth = await authenticate(env, request);
    if (!auth) return new Response("Unauthorized", { status: 401 });
    const refused = runnerRefusal(auth, request.method, url);
    if (refused) return json({ error: "forbidden", message: refused }, 403);
    const username = await handleUsernameRoute(request, env, auth, url);
    if (username) return username;
    const onboarding = await handleOnboardingRoute(request, env, auth, url);
    if (onboarding) return onboarding;
    const pluginKeys = await handlePluginKeyRoute(request, env, auth, url);
    if (pluginKeys) return pluginKeys;
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
      let runnerEpoch: number | undefined;
      if (parts[2] === "ws" || parts[2] === "nudge") {
        const admission = await runnerDeviceGate(env, auth, deviceId);
        if (admission.refused) return admission.refused;
        runnerEpoch = admission.epoch;
      }
      if (parts[2] === "ws") {
        return dialDeviceRoom(env, auth, request, url, deviceId, runnerEpoch);
      }
      if (parts[2] === "nudge" && request.method === "POST") {
        autoWakeCloudDevice(env, ctx, auth, deviceId, env.CLOUD_EDGE_URL ?? url.origin);
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
    let chatRunner: RunnerDrop | undefined;
    if (parts[0] === "chat2" && parts[1] && id.test(parts[1])) {
      const admission = await runnerChatGate(env, auth, parts[1]);
      if (admission.refused) return admission.refused;
      if (admission.epoch !== undefined && auth.deviceId) chatRunner = { device: auth.deviceId, epoch: admission.epoch };
    }
    const chat = chatRoute(request, env, auth.userId, undefined, chatRunner);
    if (chat) return chat;
    if (parts[0] === "blob" && parts.length === 4 && parts[2] === "attachments" && id.test(parts[1]) && id.test(parts[3])) {
      return handleAttachmentBlob(request, env, auth, parts[1], parts[3]);
    }
    if (parts[0] === "blob" && parts[1] && parts[2] && parts.length === 3) {
      const admission = await runnerChatGate(env, auth, parts[1]);
      if (admission.refused) return admission.refused;
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
