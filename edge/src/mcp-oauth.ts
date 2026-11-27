/**
 * Outside agents (docs/mcp.md, "Outside agents"): Zeron as a remote MCP
 * server for an agent outside it (Grok, say), signed in with OAuth 2.1 in
 * the person's browser and let in on one of their devices.
 *
 *   GET  /.well-known/oauth-protected-resource[/mcp] — RFC 9728
 *   GET  /.well-known/oauth-authorization-server     — RFC 8414
 *   POST /oauth/register       — RFC 7591; public clients only (no secret)
 *   GET  /oauth/authorize      — PKCE S256; signs the person in, then asks
 *                                which device
 *   GET  /oauth/callback       — WorkOS's return from that sign-in
 *   POST /oauth/authorize      — the person's answer: a device, or no
 *   GET  /oauth/authorize/wait — until the device answers, then back to
 *                                the agent with a code (or `access_denied`)
 *   POST /oauth/token          — authorization_code + code_verifier
 *   POST /mcp                  — the agent's MCP requests, to its device
 *
 * All before the bearer gate: none of it takes a Zeron bearer. Who the
 * person is comes from WorkOS (the sign-in's code, exchanged here), kept in
 * a short session cookie that only these pages read (`Path=/oauth`,
 * HttpOnly, Secure, SameSite=Lax), never from the request. Their device's
 * room checks the device is theirs, sends the ask there, and records the
 * grant only once the device made it (mcp-relay.ts); the room keeps codes
 * and tokens, so a token reaches its own device's room and nowhere else.
 * A token reaches what an agent in Zeron reaches (the device's agent tool
 * set, speaking for no chat), never the person's bearer API. Nothing here
 * logs a code, verifier, token, cookie or id.
 */
import { escapeHtml } from "./auth-routes";
import { b64urlDecode, b64urlEncode, constantTimeEqual, sha256B64url } from "./cloud/crypto";
import { AUTH_USER_HEADER, type Env } from "./env";
import { codeDevice, MCP_SCOPE, tokenDevice, type AskRequest, type AskStatus } from "./mcp-relay";
import { userDirectoryStub } from "./user-directory";
import { exchange, listOrgs, WorkOsAuthFailed } from "./workos";

/** How long a consent-page sign-in lasts. */
export const SESSION_TTL_MS = 30 * 60_000;
/** How long a sign-in started from `/oauth/authorize` may take. */
const LOGIN_TTL_MS = 10 * 60_000;
const SESSION_COOKIE = "__Secure-zeron-oauth";
const LOGIN_COOKIE = "__Secure-zeron-oauth-login";
/** The largest MCP request relayed (tool arguments are small). */
export const MAX_MCP_BODY = 1 << 20;
const MAX_FORM = 16 << 10;
const MAX_REDIRECT_URIS = 10;
const MAX_URI = 2048;
const MAX_STATE = 1024;
const MAX_CLIENT_NAME = 100;
/** Orgs whose rooms are read for the person's devices. */
const MAX_ORGS = 10;
const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
const CHALLENGE_RE = /^[A-Za-z0-9_-]{43}$/;
const CLIENT_ID_RE = /^zmcpcl_[A-Za-z0-9_-]{22}$/;
const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]"]);

const json = (value: unknown, status = 200, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json", ...headers } });

/** RFC 6749 §5.2 / RFC 7591 §3.2.2 errors. */
const oauthError = (error: string, description: string, status = 400): Response =>
  json({ error, error_description: description }, status, { "cache-control": "no-store" });

const resourceOf = (url: URL): string => `${url.origin}/mcp`;
const resourceMetadataOf = (url: URL): string => `${url.origin}/.well-known/oauth-protected-resource/mcp`;

/** The same answer for a token that's missing, unknown, revoked or expired. */
const unauthorized = (url: URL): Response =>
  json({ error: "invalid_token" }, 401, { "www-authenticate": `Bearer resource_metadata="${resourceMetadataOf(url)}"` });

const protectedResource = (url: URL) => ({
  resource: resourceOf(url),
  authorization_servers: [url.origin],
  scopes_supported: [MCP_SCOPE],
  bearer_methods_supported: ["header"],
  resource_name: "Zeron"
});

const authorizationServer = (url: URL) => ({
  issuer: url.origin,
  authorization_endpoint: `${url.origin}/oauth/authorize`,
  token_endpoint: `${url.origin}/oauth/token`,
  registration_endpoint: `${url.origin}/oauth/register`,
  scopes_supported: [MCP_SCOPE],
  response_types_supported: ["code"],
  grant_types_supported: ["authorization_code"],
  code_challenge_methods_supported: ["S256"],
  token_endpoint_auth_methods_supported: ["none"],
  authorization_response_iss_parameter_supported: true
});

/** Trimmed, without control characters, at most `max` characters. */
const clean = (text: string, max: number): string =>
  [...text.replace(/\p{Cc}/gu, "").trim()].slice(0, max).join("");

/** A redirect URI a public client may register: https, or http on a
 * loopback address (RFC 8252 §7.3). No wildcards, credentials or fragment. */
export const redirectUriAllowed = (raw: unknown): boolean => {
  if (typeof raw !== "string" || raw.length > MAX_URI || raw.includes("*")) return false;
  let uri: URL;
  try {
    uri = new URL(raw);
  } catch {
    return false;
  }
  if (uri.username || uri.password || uri.hash || raw.includes("#")) return false;
  if (uri.protocol === "https:") return uri.hostname !== "";
  return uri.protocol === "http:" && LOOPBACK_HOSTS.has(uri.hostname);
};

/** RFC 7591 registration of a public client, or why not. */
export const parseRegistration = (
  body: unknown
): { readonly name: string; readonly redirectUris: string[] } | { readonly error: string } => {
  if (!body || typeof body !== "object" || Array.isArray(body)) return { error: "expected a JSON object" };
  const meta = body as Record<string, unknown>;
  if (meta.client_secret !== undefined) return { error: "public clients only: no client_secret" };
  if (meta.token_endpoint_auth_method !== undefined && meta.token_endpoint_auth_method !== "none") {
    return { error: "public clients only: token_endpoint_auth_method must be none" };
  }
  const uris = meta.redirect_uris;
  if (!Array.isArray(uris) || uris.length === 0 || uris.length > MAX_REDIRECT_URIS) {
    return { error: `redirect_uris: 1 to ${MAX_REDIRECT_URIS} required` };
  }
  if (!uris.every(redirectUriAllowed)) return { error: "redirect_uris: https, or http on 127.0.0.1 or localhost only" };
  const grants = meta.grant_types;
  if (grants !== undefined && (!Array.isArray(grants) || !grants.includes("authorization_code"))) {
    return { error: "grant_types must include authorization_code" };
  }
  const responses = meta.response_types;
  if (responses !== undefined && (!Array.isArray(responses) || !responses.every((r) => r === "code"))) {
    return { error: "response_types: code only" };
  }
  const named = typeof meta.client_name === "string" ? clean(meta.client_name, MAX_CLIENT_NAME) : "";
  return { name: named || new URL(uris[0] as string).host, redirectUris: uris as string[] };
};

interface Authorize {
  readonly clientId: string;
  readonly redirectUri: string;
  readonly codeChallenge: string;
  readonly state: string | null;
}

/** `/oauth/authorize`'s parameters, checked against the client and this
 * server. The redirect URI is checked first: no error goes to one that
 * isn't the client's. */
const parseAuthorize = async (
  env: Env,
  url: URL,
  params: URLSearchParams
): Promise<{ readonly ask: Authorize; readonly clientName: string } | { readonly error: string }> => {
  const clientId = params.get("client_id") ?? "";
  const directory = userDirectoryStub(env);
  const client = CLIENT_ID_RE.test(clientId) && directory ? await directory.mcpClient(clientId) : undefined;
  if (!client) return { error: "Unknown client. Add the server to the agent again." };
  const redirectUri = params.get("redirect_uri") ?? "";
  if (!client.redirectUris.includes(redirectUri)) return { error: "That return address isn't one the client registered." };
  if (params.get("response_type") !== "code") return { error: "Only response_type=code is supported." };
  const codeChallenge = params.get("code_challenge") ?? "";
  if (params.get("code_challenge_method") !== "S256" || !CHALLENGE_RE.test(codeChallenge)) {
    return { error: "PKCE is required: code_challenge with code_challenge_method=S256." };
  }
  if (params.get("resource") !== resourceOf(url)) return { error: `resource must be ${resourceOf(url)}.` };
  const scope = params.get("scope");
  if (scope !== null && scope.split(" ").some((s) => s !== "" && s !== MCP_SCOPE)) {
    return { error: `The only scope is ${MCP_SCOPE}.` };
  }
  const state = params.get("state");
  if (state !== null && state.length > MAX_STATE) return { error: "state is too long." };
  return { ask: { clientId, redirectUri, codeChallenge, state }, clientName: client.name };
};

const AUTHORIZE_PARAMS = [
  "response_type",
  "client_id",
  "redirect_uri",
  "code_challenge",
  "code_challenge_method",
  "resource",
  "scope",
  "state"
] as const;

const authorizeParams = (from: URLSearchParams | FormData): URLSearchParams => {
  const params = new URLSearchParams();
  for (const name of AUTHORIZE_PARAMS) {
    const value = from.get(name);
    if (typeof value === "string") params.set(name, value);
  }
  return params;
};

/** Back to the client, with `iss` (RFC 9207). */
const backToClient = (url: URL, redirectUri: string, state: string | null, values: Record<string, string>): Response => {
  const target = new URL(redirectUri);
  for (const [name, value] of Object.entries(values)) target.searchParams.set(name, value);
  if (state !== null) target.searchParams.set("state", state);
  target.searchParams.set("iss", url.origin);
  return new Response(null, { status: 302, headers: { location: target.toString(), "cache-control": "no-store" } });
};

const cookie = (request: Request, name: string): string | undefined => {
  for (const part of (request.headers.get("cookie") ?? "").split(";")) {
    const [key, ...value] = part.trim().split("=");
    if (key === name) return value.join("=");
  }
  return undefined;
};

const setCookie = (name: string, value: string, maxAgeMs: number): string =>
  `${name}=${value}; Path=/oauth; HttpOnly; Secure; SameSite=Lax; Max-Age=${Math.floor(maxAgeMs / 1000)}`;

const randomSecret = (): string => b64urlEncode(crypto.getRandomValues(new Uint8Array(32)));

/** The signed-in person on these pages, from their session cookie. */
const sessionOf = async (env: Env, request: Request): Promise<{ readonly userId: string; readonly formToken: string } | undefined> => {
  const value = cookie(request, SESSION_COOKIE);
  const directory = userDirectoryStub(env);
  if (!value || !/^[A-Za-z0-9_-]{43}$/.test(value) || !directory) return undefined;
  return directory.oauthSession(await sha256B64url(value));
};

/** One of these pages; `refresh` reloads it (the waiting page, no script). */
const page = (title: string, body: string, status = 200, refresh?: string): Response =>
  new Response(
    `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="robots" content="noindex" />
${refresh ? `<meta http-equiv="refresh" content="${escapeHtml(refresh)}" />` : ""}
<title>Zeron — ${escapeHtml(title)}</title>
<style>
  body { margin: 0; min-height: 100vh; display: grid; place-items: center;
         background: #0a0a0a; color: #ededed;
         font: 15px/1.6 ui-sans-serif, system-ui, sans-serif; }
  main { max-width: 30rem; padding: 2rem; }
  h1 { font-size: 1.05rem; font-weight: 600; margin: 0 0 0.75rem; }
  p { color: #a1a1a1; margin: 0.5rem 0; }
  b { color: #ededed; font-weight: 500; }
  fieldset { border: 1px solid #2e2e2e; border-radius: 8px; margin: 1rem 0; padding: 0.5rem 0.9rem; }
  label { display: block; padding: 0.3rem 0; cursor: pointer; }
  .actions { display: flex; gap: 0.5rem; margin-top: 1rem; }
  button { padding: 0.45rem 1rem; border-radius: 8px; border: 1px solid #2e2e2e;
           font: 500 13px ui-sans-serif, system-ui, sans-serif; cursor: pointer;
           background: #171717; color: #ededed; }
  button.primary { background: #ededed; color: #0a0a0a; }
</style>
</head>
<body><main>${body}</main></body>
</html>`,
    {
      status,
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
        "x-frame-options": "DENY",
        "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
        "referrer-policy": "no-referrer"
      }
    }
  );

const failed = (message: string, status = 400): Response =>
  page("can't connect", `<h1>This agent can't connect</h1><p>${escapeHtml(message)}</p><p>Start again from the agent.</p>`, status);

interface Device {
  readonly id: string;
  readonly name: string;
  readonly platform: string | null;
}

/** The person's devices the registry knows, across their orgs (Cloud
 * machines aside: there's no window there to answer in). */
const devicesOf = async (env: Env, apiKey: string, userId: string): Promise<Device[]> => {
  const orgs = (await listOrgs(apiKey, userId)).slice(0, MAX_ORGS);
  const found = new Map<string, Device>();
  for (const org of orgs) {
    if (!ID_RE.test(org.organizationId)) continue;
    const room = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${org.organizationId}/${userId}`));
    const res = await room.fetch("https://registry/devices", { headers: { [AUTH_USER_HEADER]: userId } });
    if (!res.ok) continue;
    for (const device of ((await res.json()) as { devices: Device[] }).devices) {
      if (device.platform !== "cloud" && ID_RE.test(device.id)) found.set(device.id, device);
    }
  }
  return [...found.values()].sort((a, b) => a.name.localeCompare(b.name));
};

const deviceRoom = (env: Env, device: string) => env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(`d2/${device}`));

const consentPage = (params: URLSearchParams, clientName: string, redirectUri: string, formToken: string, devices: Device[]): Response => {
  const hidden = [...params.entries(), ["form_token", formToken]]
    .map(([name, value]) => `<input type="hidden" name="${escapeHtml(name)}" value="${escapeHtml(value)}" />`)
    .join("\n");
  const choices = devices
    .map(
      (device, i) =>
        `<label><input type="radio" name="device" value="${escapeHtml(device.id)}"${i === 0 ? " checked" : ""} required /> ${escapeHtml(device.name)}</label>`
    )
    .join("\n");
  const pick = devices.length
    ? `<p>Pick the device whose Zeron it may use. You'll allow it on that device too.</p><fieldset>${choices}</fieldset>`
    : "<p>None of your devices is signed in to Zeron. Open Zeron on one, then start again from the agent.</p>";
  return page(
    "connect an agent",
    `<h1>Let <b>${escapeHtml(clientName)}</b> use Zeron?</h1>
<p>It returns to <b>${escapeHtml(new URL(redirectUri).host)}</b>. It can read the chats on that device and message them, so an agent you already run keeps the access you gave it. Chats it starts are read-only and supervised, and it can't run a shell itself. Revoke it any time, in Settings → MCP.</p>
<form method="post" action="/oauth/authorize">
${hidden}
${pick}
<div class="actions">
${devices.length ? '<button class="primary" name="decision" value="allow">Continue</button>' : ""}
<button name="decision" value="deny" formnovalidate>Deny</button>
</div>
</form>`
  );
};

const waitPage = (url: URL): Response =>
  page(
    "waiting for your device",
    `<h1>Allow it on your device</h1><p>Zeron on that device is asking you now. This page moves on once you answer there.</p>`,
    200,
    `2;url=${url.pathname}${url.search}`
  );

/** Handle an outside-agent route; undefined means "not one". */
export const handleMcpRoute = async (request: Request, env: Env, url: URL): Promise<Response | undefined> => {
  const path = url.pathname;
  if (path === "/.well-known/oauth-protected-resource" || path === "/.well-known/oauth-protected-resource/mcp") {
    return request.method === "GET" ? json(protectedResource(url)) : json({ error: "method_not_allowed" }, 405, { allow: "GET" });
  }
  if (path === "/.well-known/oauth-authorization-server") {
    return request.method === "GET" ? json(authorizationServer(url)) : json({ error: "method_not_allowed" }, 405, { allow: "GET" });
  }
  if (path === "/mcp") {
    if (request.method !== "POST") return new Response(null, { status: 405, headers: { allow: "POST" } });
    return mcpCall(request, env, url);
  }
  if (!path.startsWith("/oauth/")) return undefined;
  if (path === "/oauth/register" && request.method === "POST") return register(request, env);
  if (path === "/oauth/token" && request.method === "POST") return token(request, env, url);
  if (path === "/oauth/authorize" && request.method === "GET") return authorize(request, env, url);
  if (path === "/oauth/authorize" && request.method === "POST") return decide(request, env, url);
  if (path === "/oauth/authorize/wait" && request.method === "GET") return wait(request, env, url);
  if (path === "/oauth/callback" && request.method === "GET") return callback(request, env, url);
  return json({ error: "not_found" }, 404);
};

const register = async (request: Request, env: Env): Promise<Response> => {
  const directory = userDirectoryStub(env);
  if (!directory) return oauthError("temporarily_unavailable", "registration is unavailable", 503);
  const text = await request.text();
  if (text.length > MAX_FORM) return oauthError("invalid_client_metadata", "too large");
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return oauthError("invalid_client_metadata", "expected a JSON object");
  }
  const parsed = parseRegistration(body);
  if ("error" in parsed) {
    const field = parsed.error.startsWith("redirect_uris") ? "invalid_redirect_uri" : "invalid_client_metadata";
    return oauthError(field, parsed.error);
  }
  const clientId = `zmcpcl_${b64urlEncode(crypto.getRandomValues(new Uint8Array(16)))}`;
  if (!(await directory.registerMcpClient(clientId, parsed.name, parsed.redirectUris))) {
    return oauthError("temporarily_unavailable", "too many clients; try again later", 503);
  }
  return json(
    {
      client_id: clientId,
      client_id_issued_at: Math.floor(Date.now() / 1000),
      client_name: parsed.name,
      redirect_uris: parsed.redirectUris,
      grant_types: ["authorization_code"],
      response_types: ["code"],
      token_endpoint_auth_method: "none",
      scope: MCP_SCOPE
    },
    201,
    { "cache-control": "no-store" }
  );
};

const authorize = async (request: Request, env: Env, url: URL): Promise<Response> => {
  const params = authorizeParams(url.searchParams);
  const parsed = await parseAuthorize(env, url, params);
  if ("error" in parsed) return failed(parsed.error);
  const apiKey = env.WORKOS_API_KEY;
  if (!apiKey) return failed("Signing in isn't set up on this server.", 501);
  const session = await sessionOf(env, request);
  if (!session) {
    // Sign in first (WorkOS), then back here with the same parameters.
    const nonce = randomSecret();
    const signIn = new URL("https://api.workos.com/user_management/authorize");
    signIn.searchParams.set("response_type", "code");
    signIn.searchParams.set("client_id", env.WORKOS_CLIENT_ID);
    signIn.searchParams.set("redirect_uri", `${url.origin}/oauth/callback`);
    signIn.searchParams.set("provider", "authkit");
    signIn.searchParams.set("state", nonce);
    const resume = b64urlEncode(new TextEncoder().encode(params.toString()));
    return new Response(null, {
      status: 302,
      headers: {
        location: signIn.toString(),
        "set-cookie": setCookie(LOGIN_COOKIE, `${nonce}.${resume}`, LOGIN_TTL_MS),
        "cache-control": "no-store"
      }
    });
  }
  let devices: Device[];
  try {
    devices = await devicesOf(env, apiKey, session.userId);
  } catch (e) {
    console.warn("oauth/authorize: listing devices failed", e instanceof WorkOsAuthFailed ? e.status ?? "no status" : "error");
    return failed("Your devices couldn't be listed just now.", 502);
  }
  return consentPage(params, parsed.clientName, parsed.ask.redirectUri, session.formToken, devices);
};

const callback = async (request: Request, env: Env, url: URL): Promise<Response> => {
  const apiKey = env.WORKOS_API_KEY;
  const directory = userDirectoryStub(env);
  if (!apiKey || !directory) return failed("Signing in isn't set up on this server.", 501);
  const [nonce, resume] = (cookie(request, LOGIN_COOKIE) ?? "").split(".");
  const state = url.searchParams.get("state") ?? "";
  const code = url.searchParams.get("code");
  if (!nonce || !resume || !code || !constantTimeEqual(state, nonce)) return failed("This sign-in didn't start here, or took too long.");
  const params = b64urlDecode(resume);
  if (!params) return failed("This sign-in didn't start here, or took too long.");
  let userId: string;
  try {
    userId = (await exchange(env, apiKey, code)).user.id;
  } catch (e) {
    console.warn("oauth/callback: sign-in exchange failed", e instanceof WorkOsAuthFailed ? e.status ?? "no status" : "error");
    return failed("Signing in failed.", 401);
  }
  const session = randomSecret();
  await directory.openOAuthSession(await sha256B64url(session), userId, randomSecret(), SESSION_TTL_MS);
  const headers = new Headers({
    location: `${url.origin}/oauth/authorize?${new TextDecoder().decode(params)}`,
    "cache-control": "no-store"
  });
  headers.append("set-cookie", setCookie(SESSION_COOKIE, session, SESSION_TTL_MS));
  headers.append("set-cookie", setCookie(LOGIN_COOKIE, "", 0));
  return new Response(null, { status: 302, headers });
};

const decide = async (request: Request, env: Env, url: URL): Promise<Response> => {
  const sent = request.headers.get("origin");
  if (sent !== null && sent !== url.origin) return failed("This answer didn't come from this page.", 403);
  const session = await sessionOf(env, request);
  if (!session) return failed("Your sign-in here expired.", 401);
  if (Number(request.headers.get("content-length") ?? "0") > MAX_FORM) return failed("Too large.", 413);
  const form = await request.formData();
  const formToken = form.get("form_token");
  if (typeof formToken !== "string" || !constantTimeEqual(formToken, session.formToken)) {
    return failed("This answer didn't come from this page.", 403);
  }
  const params = authorizeParams(form);
  const parsed = await parseAuthorize(env, url, params);
  if ("error" in parsed) return failed(parsed.error);
  const { ask, clientName } = parsed;
  if (form.get("decision") !== "allow") {
    return backToClient(url, ask.redirectUri, ask.state, { error: "access_denied" });
  }
  const device = form.get("device");
  if (typeof device !== "string" || !ID_RE.test(device)) return failed("Pick a device.");
  const body: AskRequest = { ...ask, clientName };
  const res = await deviceRoom(env, device).fetch("https://device/mcp/ask", {
    method: "POST",
    headers: { [AUTH_USER_HEADER]: session.userId, "content-type": "application/json" },
    body: JSON.stringify(body)
  });
  if (res.status === 409) return failed("That device is offline. Open Zeron on it, then start again from the agent.", 409);
  if (res.status === 429) return failed("That device has too many requests waiting. Answer those first.", 429);
  if (!res.ok) return failed("That isn't one of your devices.", 403);
  const { ask: id } = (await res.json()) as { ask: string };
  await userDirectoryStub(env)?.useMcpClient(ask.clientId);
  const next = new URL(`${url.origin}/oauth/authorize/wait`);
  next.searchParams.set("device", device);
  next.searchParams.set("ask", id);
  return new Response(null, { status: 303, headers: { location: next.toString(), "cache-control": "no-store" } });
};

const wait = async (request: Request, env: Env, url: URL): Promise<Response> => {
  const session = await sessionOf(env, request);
  if (!session) return failed("Your sign-in here expired.", 401);
  const device = url.searchParams.get("device") ?? "";
  const ask = url.searchParams.get("ask") ?? "";
  if (!ID_RE.test(device) || !/^[0-9a-f-]{36}$/.test(ask)) return failed("This link is broken.");
  const query = new URLSearchParams({ id: ask, device });
  const res = await deviceRoom(env, device).fetch(`https://device/mcp/ask?${query}`, {
    headers: { [AUTH_USER_HEADER]: session.userId }
  });
  if (!res.ok) return failed("That isn't one of your devices.", 403);
  const status = (await res.json()) as AskStatus;
  switch (status.status) {
    case "allowed":
      return backToClient(url, status.redirectUri, status.state, { code: status.code });
    case "denied":
      return backToClient(url, status.redirectUri, status.state, { error: "access_denied" });
    case "waiting":
      return waitPage(url);
    case "issued":
      return page("done", "<h1>Done</h1><p>The agent is connected. You can close this tab.</p>");
    default:
      return failed("Nobody answered on the device in time.", 410);
  }
};

const token = async (request: Request, env: Env, url: URL): Promise<Response> => {
  if (Number(request.headers.get("content-length") ?? "0") > MAX_FORM) return oauthError("invalid_request", "too large");
  const raw = await request.arrayBuffer();
  if (raw.byteLength > MAX_FORM) return oauthError("invalid_request", "too large");
  const form = new URLSearchParams(new TextDecoder().decode(raw));
  if (form.get("grant_type") !== "authorization_code") return oauthError("unsupported_grant_type", "authorization_code only");
  const code = form.get("code");
  const clientId = form.get("client_id");
  const redirectUri = form.get("redirect_uri");
  const codeVerifier = form.get("code_verifier");
  if (!code || !clientId || !redirectUri || !codeVerifier) {
    return oauthError("invalid_request", "code, client_id, redirect_uri and code_verifier are required");
  }
  const resource = form.get("resource");
  if (resource !== null && resource !== resourceOf(url)) return oauthError("invalid_target", `resource must be ${resourceOf(url)}`);
  const directory = userDirectoryStub(env);
  if (!directory) return oauthError("temporarily_unavailable", "unavailable", 503);
  if (!CLIENT_ID_RE.test(clientId) || !(await directory.mcpClient(clientId))) return oauthError("invalid_client", "unknown client");
  const device = codeDevice(code);
  if (!device) return oauthError("invalid_grant", "invalid, used or expired code");
  const res = await deviceRoom(env, device).fetch("https://device/mcp/token", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code, device, clientId, redirectUri, codeVerifier })
  });
  if (!res.ok) return oauthError("invalid_grant", "invalid, used or expired code");
  const issued = (await res.json()) as { token: string; expiresIn: number };
  return json(
    { access_token: issued.token, token_type: "Bearer", expires_in: issued.expiresIn, scope: MCP_SCOPE },
    200,
    { "cache-control": "no-store", pragma: "no-cache" }
  );
};

const mcpCall = async (request: Request, env: Env, url: URL): Promise<Response> => {
  const bearer = request.headers.get("authorization")?.match(/^Bearer (\S+)$/)?.[1];
  const device = tokenDevice(bearer);
  if (!bearer || !device) return unauthorized(url);
  if (Number(request.headers.get("content-length") ?? "0") > MAX_MCP_BODY) {
    return json({ jsonrpc: "2.0", id: null, error: { code: -32600, message: "request too large" } }, 413);
  }
  const body = await request.text();
  if (body.length > MAX_MCP_BODY) {
    return json({ jsonrpc: "2.0", id: null, error: { code: -32600, message: "request too large" } }, 413);
  }
  const headers = new Headers({ authorization: `Bearer ${bearer}` });
  for (const name of ["accept", "content-type", "mcp-protocol-version", "mcp-method", "mcp-name"]) {
    const value = request.headers.get(name);
    if (value !== null) headers.set(name, value);
  }
  const res = await deviceRoom(env, device).fetch("https://device/mcp/call", { method: "POST", headers, body });
  return res.status === 401 ? unauthorized(url) : res;
};
