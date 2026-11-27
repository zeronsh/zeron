/**
 * Outside agents (docs/mcp.md, "Outside agents"; mcp-oauth.ts, mcp-relay.ts):
 * an agent outside Zeron signs in with OAuth (public client, PKCE S256), the
 * person picks a device and allows it there, and the agent's MCP requests
 * reach that device's host as `mcp` frames — that device's and no other's,
 * live or not at all. The host here is a fake device: it makes grant ids
 * itself and answers calls only for the grants it keeps, as the engine does
 * (crates/engine/src/outside_agents.rs).
 */
import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { b64urlEncode, sha256B64url } from "../../src/cloud/crypto";
import { decodeDeviceFrame, encodeDeviceFrame } from "../../src/device-room";
import type { Env } from "../../src/env";
import { handleMcpRoute } from "../../src/mcp-oauth";
import { encodeHlc, type Op } from "../../src/registry-core";
import { call, newUser, userBearer, type TestUser } from "./cloud-helpers";

const ORIGIN = "https://edge.test";
const RESOURCE = `${ORIGIN}/mcp`;
const REDIRECT = "https://agent.example/callback";
const edgeEnv = { ...(env as unknown as Env), WORKOS_API_KEY: "sk_test" };
const rand = () => crypto.randomUUID().slice(0, 8);

const route = async (path: string, init: RequestInit = {}): Promise<Response> => {
  const url = new URL(path, ORIGIN);
  return (await handleMcpRoute(new Request(url, init), edgeEnv, url))!;
};

const form = (values: Record<string, string>) => ({
  method: "POST",
  headers: { "content-type": "application/x-www-form-urlencoded", origin: ORIGIN },
  body: new URLSearchParams(values).toString()
});

const setCookie = (res: Response, name: string): string | undefined => {
  for (const line of res.headers.getSetCookie()) {
    const [pair] = line.split(";");
    const at = pair!.indexOf("=");
    if (pair!.slice(0, at) === name) return pair!.slice(at + 1);
  }
  return undefined;
};

const register = (metadata: Record<string, unknown>) =>
  route("/oauth/register", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(metadata) });

const newClient = async (): Promise<string> => {
  const res = await register({ client_name: "Grok", redirect_uris: [REDIRECT] });
  expect(res.status).toBe(201);
  return ((await res.json()) as { client_id: string }).client_id;
};

const pkce = async () => {
  const verifier = b64urlEncode(crypto.getRandomValues(new Uint8Array(32)));
  return { verifier, challenge: await sha256B64url(verifier) };
};

const authorizeQuery = (clientId: string, challenge: string, extra: Record<string, string> = {}) =>
  new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: REDIRECT,
    code_challenge: challenge,
    code_challenge_method: "S256",
    resource: RESOURCE,
    state: "agent-state",
    ...extra
  });

let clock = 0;
const deviceRow = (id: string, name: string): Op => ({
  kind: "devices",
  id,
  op: "upsert",
  set: { name },
  hlc: encodeHlc(Date.now(), clock++, id)
});

/** A signed-up person (WorkOS) with `devices` in the registry. */
const person = async (...devices: string[]): Promise<TestUser> => {
  const u = newUser("oa");
  const seed = (path: string, body: unknown) => fetch(`https://api.workos.com/${path}`, { method: "POST", body: JSON.stringify(body) });
  await seed("__users", { id: u.userId, email: `${u.userId}@example.net` });
  await seed("__memberships", { user_id: u.userId, organization_id: u.orgId, role: "member" });
  const pushed = await call("POST", `/registry/${u.orgId}/push?device=${devices[0]}`, userBearer(u), {
    batch: `b-${rand()}`,
    ops: devices.map((d) => deviceRow(d, `Laptop ${d}`))
  });
  expect(pushed.status, await pushed.clone().text()).toBe(200);
  return u;
};

interface Frame {
  readonly s: string;
  readonly m: Record<string, unknown>;
}

/** A fake device's host socket: it makes its own grant ids, keeps them, and
 * answers a call only for a grant it keeps (401 otherwise). */
const device = async (u: TestUser, id: string, opts: { allow?: boolean } = {}) => {
  const res = await call("GET", `/device/${id}/ws?role=host`, userBearer(u), undefined, { upgrade: "websocket" });
  expect(res.status).toBe(101);
  const ws = res.webSocket!;
  ws.accept();
  ws.binaryType = "arraybuffer";
  const frames: Frame[] = [];
  const grants = new Set<string>();
  const send = (s: string, m: unknown) => ws.send(encodeDeviceFrame({ s, k: "mcp" }, new TextEncoder().encode(JSON.stringify(m))));
  ws.addEventListener("message", (e) => {
    const frame = decodeDeviceFrame(new Uint8Array(e.data as ArrayBuffer));
    if (frame.header.k !== "mcp") return;
    const m = JSON.parse(new TextDecoder().decode(frame.payload)) as Record<string, unknown>;
    frames.push({ s: frame.header.s, m });
    if (m.t === "ask") {
      if (opts.allow === false) return send(frame.header.s, { t: "answer", denied: true });
      const grant = `dev-${crypto.randomUUID()}`;
      grants.add(grant);
      send(frame.header.s, { t: "answer", grant });
    } else if (m.t === "call") {
      if (!grants.has(m.grant as string)) {
        send(frame.header.s, { t: "head", status: 401, headers: { "content-type": "application/json" } });
        return send(frame.header.s, { t: "end" });
      }
      const request = JSON.parse(m.body as string) as { id: number };
      send(frame.header.s, { t: "head", status: 200, headers: { "content-type": "application/json" } });
      send(frame.header.s, { t: "data", data: JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { tools: [] } }) });
      send(frame.header.s, { t: "end" });
    }
  });
  return {
    ws,
    frames,
    grants,
    calls: () => frames.filter((f) => f.m.t === "call"),
    revoke: (grant: string) => {
      grants.delete(grant);
      send("grants", { t: "revoked", grant });
    }
  };
};

const room = (id: string) => env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(`d2/${id}`));

/** Signed in on the consent pages: the session cookie. */
const signIn = async (u: TestUser, query: URLSearchParams): Promise<string> => {
  const first = await route(`/oauth/authorize?${query}`);
  expect(first.status).toBe(302);
  const workos = new URL(first.headers.get("location")!);
  expect(`${workos.origin}${workos.pathname}`).toBe("https://api.workos.com/user_management/authorize");
  expect(workos.searchParams.get("redirect_uri")).toBe(`${ORIGIN}/oauth/callback`);
  const login = setCookie(first, "__Secure-zeron-oauth-login")!;
  const back = await route(`/oauth/callback?${new URLSearchParams({ code: `code:${u.userId}`, state: workos.searchParams.get("state")! })}`, {
    headers: { cookie: `__Secure-zeron-oauth-login=${login}` }
  });
  expect(back.status).toBe(302);
  expect(back.headers.get("location")).toBe(`${ORIGIN}/oauth/authorize?${query}`);
  const session = setCookie(back, "__Secure-zeron-oauth")!;
  expect(back.headers.getSetCookie().find((c) => c.startsWith("__Secure-zeron-oauth="))).toMatch(/HttpOnly; Secure; SameSite=Lax/);
  return session;
};

/** The whole browser side: sign in, pick `deviceId`, wait for the device's
 * answer. The agent's redirect, as a URL. */
const authorize = async (u: TestUser, clientId: string, deviceId: string, challenge: string): Promise<URL> => {
  const query = authorizeQuery(clientId, challenge);
  const session = await signIn(u, query);
  const cookie = `__Secure-zeron-oauth=${session}`;
  const consent = await route(`/oauth/authorize?${query}`, { headers: { cookie } });
  expect(consent.status).toBe(200);
  const html = await consent.text();
  expect(html).toContain("Grok");
  expect(html).toContain(`value="${deviceId}"`);
  const formToken = html.match(/name="form_token" value="([^"]+)"/)![1]!;
  const decided = await route("/oauth/authorize", {
    ...form({ ...Object.fromEntries(query), form_token: formToken, device: deviceId, decision: "allow" }),
    headers: { ...form({}).headers, cookie }
  });
  expect(decided.status, await decided.clone().text()).toBe(303);
  const waitAt = decided.headers.get("location")!;
  let back: Response | undefined;
  await expect
    .poll(async () => {
      back = await route(waitAt, { headers: { cookie } });
      return back.status;
    })
    .toBe(302);
  const target = new URL(back!.headers.get("location")!);
  expect(target.searchParams.get("state")).toBe("agent-state");
  expect(target.searchParams.get("iss")).toBe(ORIGIN);
  return target;
};

const exchange = (clientId: string, code: string, verifier: string) =>
  route("/oauth/token", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code,
      client_id: clientId,
      redirect_uri: REDIRECT,
      code_verifier: verifier,
      resource: RESOURCE
    }).toString()
  });

/** An access token for `deviceId`, the whole way. */
const connect = async (u: TestUser, clientId: string, deviceId: string): Promise<string> => {
  const { verifier, challenge } = await pkce();
  const back = await authorize(u, clientId, deviceId, challenge);
  const res = await exchange(clientId, back.searchParams.get("code")!, verifier);
  expect(res.status, await res.clone().text()).toBe(200);
  const body = (await res.json()) as { access_token: string; token_type: string; scope: string };
  expect(body).toMatchObject({ token_type: "Bearer", scope: "mcp:agent" });
  return body.access_token;
};

const mcp = (token: string, id = 1) =>
  route("/mcp", {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      accept: "application/json, text/event-stream"
    },
    body: JSON.stringify({ jsonrpc: "2.0", id, method: "tools/list" })
  });

const expectUnauthorized = async (res: Response) => {
  expect(res.status).toBe(401);
  expect(res.headers.get("www-authenticate")).toBe(`Bearer resource_metadata="${ORIGIN}/.well-known/oauth-protected-resource/mcp"`);
  expect(await res.json()).toEqual({ error: "invalid_token" });
};

describe("outside agents: discovery and registration", () => {
  it("publishes an S256-only authorization server for the MCP resource", async () => {
    for (const path of ["/.well-known/oauth-protected-resource", "/.well-known/oauth-protected-resource/mcp"]) {
      const prm = (await (await route(path)).json()) as Record<string, unknown>;
      expect(prm).toMatchObject({ resource: RESOURCE, authorization_servers: [ORIGIN], scopes_supported: ["mcp:agent"] });
    }
    const as = (await (await route("/.well-known/oauth-authorization-server")).json()) as Record<string, unknown>;
    expect(as).toMatchObject({
      issuer: ORIGIN,
      authorization_endpoint: `${ORIGIN}/oauth/authorize`,
      token_endpoint: `${ORIGIN}/oauth/token`,
      registration_endpoint: `${ORIGIN}/oauth/register`,
      code_challenge_methods_supported: ["S256"],
      response_types_supported: ["code"],
      grant_types_supported: ["authorization_code"],
      token_endpoint_auth_methods_supported: ["none"]
    });
    // No token: where to sign in, through the real Worker too.
    await expectUnauthorized(await route("/mcp", { method: "POST", body: "{}" }));
    await expectUnauthorized(await call("POST", "/mcp", undefined, { jsonrpc: "2.0", id: 1, method: "tools/list" }));
    // A Zeron bearer isn't an outside agent's token.
    await expectUnauthorized(await call("POST", "/mcp", "Bearer someone@org1", {}));
    expect((await route("/mcp")).status).toBe(405);
    expect((await route("/mcp", { method: "DELETE" })).status).toBe(405);
  });

  it("registers public clients only, returning to https or loopback http", async () => {
    const refused = async (metadata: Record<string, unknown>, error: string) => {
      const res = await register({ client_name: "Grok", redirect_uris: [REDIRECT], ...metadata });
      expect(res.status).toBe(400);
      expect(((await res.json()) as { error: string }).error).toBe(error);
    };
    await refused({ client_secret: "s3cret" }, "invalid_client_metadata");
    await refused({ token_endpoint_auth_method: "client_secret_post" }, "invalid_client_metadata");
    await refused({ token_endpoint_auth_method: "client_secret_basic" }, "invalid_client_metadata");
    await refused({ redirect_uris: ["http://agent.example/callback"] }, "invalid_redirect_uri");
    await refused({ redirect_uris: ["http://192.168.1.5/callback"] }, "invalid_redirect_uri");
    await refused({ redirect_uris: ["https://*.agent.example/callback"] }, "invalid_redirect_uri");
    await refused({ redirect_uris: ["https://user:pw@agent.example/callback"] }, "invalid_redirect_uri");
    await refused({ redirect_uris: ["javascript:alert(1)"] }, "invalid_redirect_uri");
    await refused({ redirect_uris: [] }, "invalid_redirect_uri");

    for (const uri of [REDIRECT, "http://127.0.0.1:33418/callback", "http://localhost:8080/cb"]) {
      const res = await register({ client_name: "Grok", redirect_uris: [uri], token_endpoint_auth_method: "none" });
      expect(res.status).toBe(201);
      const body = (await res.json()) as Record<string, unknown>;
      expect(body.client_id).toMatch(/^zmcpcl_/);
      expect(body.token_endpoint_auth_method).toBe("none");
      expect(body.client_secret).toBeUndefined();
      expect(body.redirect_uris).toEqual([uri]);
    }
  });

  it("answers authorize without PKCE S256, or for another resource, with 400 and no redirect", async () => {
    const clientId = await newClient();
    const { challenge } = await pkce();
    const refused = async (query: URLSearchParams) => {
      const res = await route(`/oauth/authorize?${query}`);
      expect(res.status).toBe(400);
      expect(res.headers.get("location")).toBeNull();
    };
    const without = authorizeQuery(clientId, challenge);
    without.delete("code_challenge");
    without.delete("code_challenge_method");
    await refused(without);
    await refused(authorizeQuery(clientId, challenge, { code_challenge_method: "plain" }));
    await refused(authorizeQuery(clientId, "short"));
    await refused(authorizeQuery(clientId, challenge, { resource: "https://elsewhere.example/mcp" }));
    await refused(authorizeQuery(clientId, challenge, { redirect_uri: "https://attacker.example/callback" }));
    await refused(authorizeQuery(clientId, challenge, { scope: "mcp:agent admin" }));
    await refused(authorizeQuery("zmcpcl_AAAAAAAAAAAAAAAAAAAAAA", challenge));
    // All in order: off to sign in.
    expect((await route(`/oauth/authorize?${authorizeQuery(clientId, challenge)}`)).status).toBe(302);
  });
});

describe("outside agents: codes and tokens", () => {
  it("issues a code only once the device made the grant, and a token for its verifier", async () => {
    const laptop = `laptop-${rand()}`;
    const u = await person(laptop);
    const host = await device(u, laptop);
    const clientId = await newClient();
    const token = await connect(u, clientId, laptop);

    const asks = host.frames.filter((f) => f.m.t === "ask");
    expect(asks).toHaveLength(1);
    expect(asks[0]!.m).toEqual({ t: "ask", client: "Grok", redirectHost: "agent.example" });
    const [grant] = [...host.grants];
    // What the room keeps is the device's grant, and no secret as itself.
    await runInDurableObject(room(laptop), (_i, state) => {
      const sql = state.storage.sql;
      expect(sql.exec("SELECT id FROM mcp_grants").toArray().map((r) => r.id)).toEqual([grant]);
      const stored = JSON.stringify([...sql.exec("SELECT * FROM mcp_tokens").toArray(), ...sql.exec("SELECT * FROM mcp_codes").toArray()]);
      expect(stored).not.toContain(token);
      expect(stored).not.toContain(token.split(".")[2]);
    });

    const res = await mcp(token, 42);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ jsonrpc: "2.0", id: 42, result: { tools: [] } });
    const [relayed] = host.calls();
    expect(relayed!.m).toMatchObject({ t: "call", grant, body: JSON.stringify({ jsonrpc: "2.0", id: 42, method: "tools/list" }) });
    expect(JSON.stringify(relayed!.m)).not.toContain(token);
    host.ws.close();
  });

  it("refuses a wrong verifier, a reused code (revoking its token) and an expired code", async () => {
    const laptop = `laptop-${rand()}`;
    const u = await person(laptop);
    const host = await device(u, laptop);
    const clientId = await newClient();
    const invalid = async (res: Response) => {
      expect(res.status).toBe(400);
      const body = (await res.json()) as Record<string, unknown>;
      expect(body.error).toBe("invalid_grant");
      expect(body.access_token).toBeUndefined();
    };

    // Wrong verifier: refused, and the code is burnt for the right one too.
    const first = await pkce();
    const code1 = (await authorize(u, clientId, laptop, first.challenge)).searchParams.get("code")!;
    await invalid(await exchange(clientId, code1, (await pkce()).verifier));
    await invalid(await exchange(clientId, code1, first.verifier));

    // Reused: the second redemption fails and takes the first's token.
    const second = await pkce();
    const code2 = (await authorize(u, clientId, laptop, second.challenge)).searchParams.get("code")!;
    const ok = await exchange(clientId, code2, second.verifier);
    expect(ok.status).toBe(200);
    const token = ((await ok.json()) as { access_token: string }).access_token;
    expect((await mcp(token)).status).toBe(200);
    await invalid(await exchange(clientId, code2, second.verifier));
    await expectUnauthorized(await mcp(token));

    // Another client, or another redirect URI, can't redeem it either.
    const third = await pkce();
    const code3 = (await authorize(u, clientId, laptop, third.challenge)).searchParams.get("code")!;
    await invalid(await exchange(await newClient(), code3, third.verifier));

    // Expired.
    const fourth = await pkce();
    const code4 = (await authorize(u, clientId, laptop, fourth.challenge)).searchParams.get("code")!;
    await runInDurableObject(room(laptop), (_i, state) => {
      state.storage.sql.exec("UPDATE mcp_codes SET expires_at = ?", Date.now() - 1);
    });
    await invalid(await exchange(clientId, code4, fourth.verifier));

    // A code that isn't one at all.
    await invalid(await exchange(clientId, `zmcpc1.${laptop}.${b64urlEncode(new Uint8Array(32))}`, fourth.verifier));
    host.ws.close();
  });

  it("sends a no back as access_denied and keeps no grant", async () => {
    const laptop = `laptop-${rand()}`;
    const u = await person(laptop);
    const host = await device(u, laptop, { allow: false });
    const clientId = await newClient();
    const back = await authorize(u, clientId, laptop, (await pkce()).challenge);
    expect(back.searchParams.get("error")).toBe("access_denied");
    expect(back.searchParams.get("code")).toBeNull();
    await runInDurableObject(room(laptop), (_i, state) => {
      expect(state.storage.sql.exec("SELECT COUNT(*) AS n FROM mcp_grants").one().n).toBe(0);
      expect(state.storage.sql.exec("SELECT COUNT(*) AS n FROM mcp_codes").one().n).toBe(0);
    });
    host.ws.close();
  });
});

describe("outside agents: calls", () => {
  it("never forwards a token for device A to device B", async () => {
    const a = `laptop-${rand()}`;
    const b = `desktop-${rand()}`;
    const u = await person(a, b);
    const hostA = await device(u, a);
    const hostB = await device(u, b);
    const token = await connect(u, await newClient(), a);

    expect((await mcp(token)).status).toBe(200);
    expect(hostA.calls()).toHaveLength(1);
    // A's secret under B's id: B's room has no such token.
    const [, , secret] = token.split(".");
    await expectUnauthorized(await mcp(`zmcp1.${b}.${secret}`));
    await expectUnauthorized(await mcp(`zmcp1.${b}`));
    expect(hostB.frames).toEqual([]);
    hostA.ws.close();
    hostB.ws.close();
  });

  it("answers 401 once the device revoked the grant, or for one it never kept", async () => {
    const laptop = `laptop-${rand()}`;
    const u = await person(laptop);
    const host = await device(u, laptop);
    const clientId = await newClient();

    // Revoked on the device: the room drops it, and no call goes there.
    const revoked = await connect(u, clientId, laptop);
    expect((await mcp(revoked)).status).toBe(200);
    host.revoke([...host.grants][0]!);
    await expect.poll(() => runInDurableObject(room(laptop), (_i, s) => s.storage.sql.exec("SELECT COUNT(*) AS n FROM mcp_grants").one().n)).toBe(0);
    const before = host.calls().length;
    await expectUnauthorized(await mcp(revoked));
    expect(host.calls()).toHaveLength(before);

    // Gone from the device while the room still had it (a revoke the room
    // missed): the device answers 401, and the room drops it then.
    const missed = await connect(u, clientId, laptop);
    host.grants.clear();
    await expectUnauthorized(await mcp(missed));
    const afterRefusal = host.calls().length;
    await expectUnauthorized(await mcp(missed));
    expect(host.calls()).toHaveLength(afterRefusal);

    // Unknown token.
    await expectUnauthorized(await mcp(`zmcp1.${laptop}.${b64urlEncode(crypto.getRandomValues(new Uint8Array(32)))}`));
    host.ws.close();
  });

  it("answers a JSON-RPC error while the device is offline, and queues nothing", async () => {
    const laptop = `laptop-${rand()}`;
    const u = await person(laptop);
    const host = await device(u, laptop);
    const token = await connect(u, await newClient(), laptop);
    host.ws.close();
    await expect
      .poll(async () => ((await (await call("GET", `/device/${laptop}/status`, userBearer(u))).json()) as { hostConnected: boolean }).hostConnected)
      .toBe(false);

    const res = await mcp(token, 9);
    expect(res.status).toBe(503);
    const body = (await res.json()) as { jsonrpc: string; id: number; error: { code: number; message: string } };
    expect(body).toMatchObject({ jsonrpc: "2.0", id: 9, error: { code: -32000 } });
    expect(body.error.message).toMatch(/offline/);

    // Back online: the earlier call never arrives.
    const again = await device(u, laptop);
    for (const grant of host.grants) again.grants.add(grant);
    await new Promise((r) => setTimeout(r, 100));
    expect(again.calls()).toEqual([]);
    // And the next call goes through, live.
    expect((await mcp(token, 10)).status).toBe(200);
    expect(again.calls()).toHaveLength(1);
    again.ws.close();
  });

  it("asks nothing of an offline device, or of someone else's", async () => {
    const laptop = `laptop-${rand()}`;
    const u = await person(laptop);
    const host = await device(u, laptop);
    host.ws.close();
    await expect
      .poll(async () => ((await (await call("GET", `/device/${laptop}/status`, userBearer(u))).json()) as { hostConnected: boolean }).hostConnected)
      .toBe(false);
    const clientId = await newClient();
    const { challenge } = await pkce();
    const query = authorizeQuery(clientId, challenge);
    const cookie = `__Secure-zeron-oauth=${await signIn(u, query)}`;
    const html = await (await route(`/oauth/authorize?${query}`, { headers: { cookie } })).text();
    const formToken = html.match(/name="form_token" value="([^"]+)"/)![1]!;
    const decide = (deviceId: string, extra: Record<string, string> = {}) =>
      route("/oauth/authorize", {
        ...form({ ...Object.fromEntries(query), form_token: formToken, device: deviceId, decision: "allow", ...extra }),
        headers: { ...form({}).headers, cookie }
      });
    expect((await decide(laptop)).status).toBe(409);

    // Someone else's device: its room refuses.
    const other = `laptop-${rand()}`;
    const stranger = await person(other);
    const theirs = await device(stranger, other);
    expect((await decide(other)).status).toBe(403);
    expect(theirs.frames).toEqual([]);
    // A forged form token, or another origin: refused before any room.
    expect((await decide(laptop, { form_token: "x".repeat(43) })).status).toBe(403);
    const crossSite = await route("/oauth/authorize", {
      ...form({ ...Object.fromEntries(query), form_token: formToken, device: other, decision: "allow" }),
      headers: { ...form({}).headers, cookie, origin: "https://attacker.example" }
    });
    expect(crossSite.status).toBe(403);
    // Deny: straight back, nothing asked.
    const denied = await decide(other, { decision: "deny" });
    expect(denied.status).toBe(302);
    expect(new URL(denied.headers.get("location")!).searchParams.get("error")).toBe("access_denied");
    expect(theirs.frames).toEqual([]);
    theirs.ws.close();
  });
});
