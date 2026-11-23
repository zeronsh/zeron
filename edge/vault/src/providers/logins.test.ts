import { describe, expect, it, vi } from "vitest";
import { fakeJwt } from "../../test/support";
import { antigravityAdapter, GOOGLE_TOKEN_URL, parseAntigravityUpload, refreshAntigravity } from "./antigravity";
import { museAdapter, parseMuseUpload, withoutRefreshTokens } from "./muse";
import { nousAdapter, parseNousUpload, refreshNous } from "./nous";
import { classifyTokenReply, httpsUrlOn, refreshDueAhead } from "./oauth";
import type { FetchFn } from "./types";
import { HERMES_XAI_CLIENT_ID, parseXaiOauthUpload, refreshXaiOauth, xaiOauthAdapter, XAI_TOKEN_URL } from "./xai-oauth";

const NOW = Date.parse("2026-11-18T12:00:00Z");
const HOUR = 60 * 60_000;
const sec = (ms: number) => Math.floor(ms / 1000);
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const form = (init: RequestInit | undefined) => Object.fromEntries(new URLSearchParams(String(init?.body)));

describe("oauth helpers", () => {
  it("sends refresh tokens only to https hosts of the provider", () => {
    expect(httpsUrlOn("https://auth.x.ai/oauth2/token", ["x.ai"])).toBeDefined();
    for (const bad of ["http://auth.x.ai/t", "https://x.ai.evil.com/t", "https://evilx.ai/t", "https://auth.x.ai:8443/t", "https://u:p@auth.x.ai/t"]) {
      expect(httpsUrlOn(bad, ["x.ai"]), bad).toBeUndefined();
    }
  });

  it("refreshes ahead by the cap, or half a known life", () => {
    expect(refreshDueAhead(NOW + 3 * HOUR, 0, NOW, 2 * HOUR)).toBe(false);
    expect(refreshDueAhead(NOW + HOUR, 0, NOW, 2 * HOUR)).toBe(true);
    expect(refreshDueAhead(NOW + 40 * 60_000, NOW - 20 * 60_000, NOW, 2 * HOUR)).toBe(false);
  });

  it("classifies token replies", () => {
    expect(classifyTokenReply(400, { error: "refresh_token_reused" }).kind).toBe("reconnect");
    expect(classifyTokenReply(400, { error: "invalid_grant" }).kind).toBe("reconnect");
    expect(classifyTokenReply(500, undefined).kind).toBe("transient");
    expect(classifyTokenReply(200, {}).kind).toBe("transient");
  });
});

describe("xai-oauth (Hermes)", () => {
  const access = fakeJwt({ exp: sec(NOW + 6 * HOUR), sub: "xai-user-1" });
  const material = { xaiOauth: { tokens: { access_token: access, refresh_token: "rt-1", id_token: fakeJwt({ email: "ada@x.ai", sub: "xai-user-1" }), token_type: "Bearer" } } };

  it("parses Hermes' record, defaulting the endpoint and client", async () => {
    const parsed = parseXaiOauthUpload(material, NOW);
    if (!parsed.ok) throw new Error(parsed.message);
    expect(parsed.secret).toMatchObject({ tokenEndpoint: XAI_TOKEN_URL, clientId: HERMES_XAI_CLIENT_ID, expiresAt: NOW - (NOW % 1000) + 6 * HOUR });
    expect(xaiOauthAdapter.account(parsed.secret)).toBe("ada@x.ai");
    expect(await xaiOauthAdapter.identity!(parsed.secret, {})).toBe("xai:xai-user-1");
    const grant = xaiOauthAdapter.grant(parsed.secret, NOW);
    expect(grant.settings).toMatchObject({ token_endpoint: XAI_TOKEN_URL, token_type: "Bearer" });
    expect(JSON.stringify(grant)).not.toContain("rt-1");
    const evil = parseXaiOauthUpload({ xaiOauth: { ...material.xaiOauth, discovery: { token_endpoint: "https://evil.example/token" } } }, NOW);
    expect(evil.ok).toBe(false);
  });

  it("keeps grants two hours from expiry (Hermes refreshes an hour ahead itself)", async () => {
    const parsed = parseXaiOauthUpload(material, NOW);
    if (!parsed.ok) throw new Error(parsed.message);
    expect(xaiOauthAdapter.refreshDue(parsed.secret, NOW)).toBe(false);
    expect(xaiOauthAdapter.refreshDue(parsed.secret, NOW + 4.5 * HOUR)).toBe(true);
    const fetchFn = vi.fn<FetchFn>(async () => json(200, { access_token: fakeJwt({ exp: sec(NOW + 7 * HOUR) }), refresh_token: "rt-2" }));
    const done = await refreshXaiOauth(parsed.secret, { fetch: fetchFn, now: NOW });
    expect(fetchFn.mock.calls[0]![0]).toBe(XAI_TOKEN_URL);
    expect(form(fetchFn.mock.calls[0]![1])).toEqual({ grant_type: "refresh_token", client_id: HERMES_XAI_CLIENT_ID, refresh_token: "rt-1" });
    expect(done.kind === "ok" && done.secret.refreshToken).toBe("rt-2");
  });
});

describe("nous (Hermes)", () => {
  const record = {
    access_token: fakeJwt({ exp: sec(NOW + 3 * HOUR), email: "ada@nous.example", scope: "inference:invoke" }),
    refresh_token: "nous-rt-1",
    expires_at: new Date(NOW + 3 * HOUR).toISOString(),
    client_id: "hermes-cli",
    portal_base_url: "https://portal.nousresearch.com/",
    inference_base_url: "https://inference-api.nousresearch.com/v1",
    scope: "inference:invoke",
    agent_key: "never-leaves",
    token_type: "Bearer"
  };

  it("keeps the portal fields, never the agent key, and pins the portal", () => {
    const parsed = parseNousUpload({ nousAuth: record }, NOW);
    if (!parsed.ok) throw new Error(parsed.message);
    const grant = nousAdapter.grant(parsed.secret, NOW);
    expect(grant.settings).toEqual({
      portal_base_url: "https://portal.nousresearch.com",
      inference_base_url: "https://inference-api.nousresearch.com/v1",
      client_id: "hermes-cli",
      scope: "inference:invoke",
      token_type: "Bearer"
    });
    expect(JSON.stringify(grant)).not.toMatch(/never-leaves|nous-rt-1/);
    expect(parseNousUpload({ nousAuth: { ...record, portal_base_url: "https://evil.example" } }, NOW).ok).toBe(false);
  });

  it("refreshes with the refresh token in its header", async () => {
    const parsed = parseNousUpload({ nousAuth: record }, NOW);
    if (!parsed.ok) throw new Error(parsed.message);
    const fetchFn = vi.fn<FetchFn>(async () => json(200, { access_token: fakeJwt({ exp: sec(NOW + 4 * HOUR) }), refresh_token: "nous-rt-2", expires_in: 14400 }));
    const done = await refreshNous(parsed.secret, { fetch: fetchFn, now: NOW });
    const [url, init] = fetchFn.mock.calls[0]!;
    expect(url).toBe("https://portal.nousresearch.com/api/oauth/token");
    expect(form(init)).toEqual({ grant_type: "refresh_token", client_id: "hermes-cli" });
    expect(new Headers(init?.headers).get("x-nous-refresh-token")).toBe("nous-rt-1");
    expect(done.kind === "ok" && done.secret.refreshToken).toBe("nous-rt-2");
    const reused = await refreshNous(parsed.secret, { fetch: async () => json(400, { error: "refresh_token_reused" }), now: NOW });
    expect(reused.kind).toBe("reconnect");
  });
});

describe("antigravity", () => {
  const token = {
    client_id: "1071006060591-abc.apps.googleusercontent.com",
    client_secret: "GOCSPX-test",
    refresh_token: "1//google-rt",
    token_uri: "https://evil.example/token",
    scopes: ["https://www.googleapis.com/auth/cloud-platform"],
    project_id: "proj-1"
  };

  it("refreshes at Google whatever token_uri says, first grant included", async () => {
    const parsed = parseAntigravityUpload({ antigravityToken: token });
    if (!parsed.ok) throw new Error(parsed.message);
    expect(antigravityAdapter.refreshDue(parsed.secret, NOW)).toBe(true);
    const fetchFn = vi.fn<FetchFn>(async () => json(200, { access_token: "ya29.a", expires_in: 3599 }));
    const done = await refreshAntigravity(parsed.secret, { fetch: fetchFn, now: NOW });
    expect(fetchFn.mock.calls[0]![0]).toBe(GOOGLE_TOKEN_URL);
    expect(form(fetchFn.mock.calls[0]![1])).toMatchObject({ grant_type: "refresh_token", refresh_token: "1//google-rt", client_id: token.client_id });
    if (done.kind !== "ok") throw new Error(done.reason);
    expect(done.secret.refreshToken).toBe("1//google-rt");
    expect(antigravityAdapter.refreshDue(done.secret, NOW + 5 * 60_000)).toBe(false);
    expect(antigravityAdapter.refreshDue(done.secret, NOW + 11 * 60_000)).toBe(true);
    const grant = antigravityAdapter.grant(done.secret, NOW);
    expect(grant).toMatchObject({ accessToken: "ya29.a", settings: { client_id: token.client_id, project_id: "proj-1" } });
    expect(JSON.stringify(grant)).not.toMatch(/google-rt|GOCSPX/);
    expect(parseAntigravityUpload({ antigravityToken: { ...token, client_id: "someone-else" } }).ok).toBe(false);
  });

  it("accepts unified CLI antigravity-oauth-token uploads and refreshes via Worker OAuth client", async () => {
    const cliDoc = {
      token: {
        access_token: "ya29.initial",
        token_type: "Bearer",
        refresh_token: "1//cli-rt",
        expiry: new Date(NOW + 55 * 60_000).toISOString()
      },
      auth_method: "consumer"
    };
    const parsed = parseAntigravityUpload({ antigravityToken: cliDoc });
    if (!parsed.ok) throw new Error(parsed.message);
    expect(parsed.secret.refreshToken).toBe("1//cli-rt");
    expect(parsed.secret.accessToken).toBe("ya29.initial");
    expect(antigravityAdapter.refreshDue(parsed.secret, NOW)).toBe(false);
    expect(antigravityAdapter.refreshDue(parsed.secret, NOW + 10 * 60_000)).toBe(true);

    // Without configured Worker OAuth client secrets, returns a transient error.
    const fetchFn = vi.fn<FetchFn>(async () => json(200, { access_token: "ya29.b", expires_in: 3599 }));
    const unconfigured = await refreshAntigravity(parsed.secret, { fetch: fetchFn, now: NOW });
    expect(unconfigured.kind).toBe("transient");
    expect(fetchFn).not.toHaveBeenCalled();

    // With configured Worker OAuth client secrets, refreshes at Google cleanly.
    const done = await refreshAntigravity(parsed.secret, {
      fetch: fetchFn,
      now: NOW,
      antigravityClientId: "1071006060591-cli.apps.googleusercontent.com",
      antigravityClientSecret: "GOCSPX-worker"
    });
    expect(done.kind).toBe("ok");
    if (done.kind !== "ok") throw new Error(done.reason);
    expect(form(fetchFn.mock.calls[0]![1])).toMatchObject({
      grant_type: "refresh_token",
      refresh_token: "1//cli-rt",
      client_id: "1071006060591-cli.apps.googleusercontent.com",
      client_secret: "GOCSPX-worker"
    });
    expect(done.secret.accessToken).toBe("ya29.b");
    const grant = antigravityAdapter.grant(done.secret, NOW);
    expect(grant.accessToken).toBe("ya29.b");
    expect(JSON.stringify(grant)).not.toMatch(/cli-rt|GOCSPX/);
  });
});

describe("muse", () => {
  const doc = {
    schema_version: 2,
    providers: {
      meta: { mechanism: "oauth", access_token: "meta-at", refresh_token: "meta-rt", expires_at: sec(NOW + 30 * 24 * HOUR), user_email: "ada@meta.example" }
    }
  };

  it("hands the document back without refresh tokens, and expires with the login", async () => {
    const parsed = parseMuseUpload({ museAuth: doc }, NOW);
    if (!parsed.ok) throw new Error(parsed.message);
    expect(museAdapter.hardExpiresAt!(parsed.secret)).toBe(doc.providers.meta.expires_at * 1000);
    const grant = museAdapter.grant(parsed.secret, NOW);
    expect(grant.expiresAt).toBe(NOW + HOUR);
    const written = JSON.parse(grant.settings!.document!);
    expect(written.providers.meta).toMatchObject({ access_token: "meta-at", refresh_token: "" });
    expect(await museAdapter.identity!(parsed.secret, {})).toBe("meta:ada@meta.example");
    expect(withoutRefreshTokens({ a: [{ refresh_token: "x" }] })).toEqual({ a: [{ refresh_token: "" }] });
    expect(parseMuseUpload({ museAuth: { providers: {} } }, NOW).ok).toBe(false);
  });

  it("takes Muse 1.4's login as it writes it: no expiry, no refresh token", () => {
    const written = {
      schema_version: 1,
      providers: {
        meta: {
          access_token: "meta-at-opaque",
          obtained_via: "device_code",
          mechanism: "oauth",
          api_key: "mk-minted",
          api_base_url: "https://api.meta.ai",
          user_full_name: "Ada L",
          user_email: "ada@meta.example"
        }
      }
    };
    const parsed = parseMuseUpload({ museAuth: written }, NOW);
    if (!parsed.ok) throw new Error(parsed.message);
    expect(museAdapter.hardExpiresAt!(parsed.secret)).toBeUndefined();
    expect(museAdapter.account(parsed.secret)).toBe("ada@meta.example");
    const grant = museAdapter.grant(parsed.secret, NOW);
    expect(grant.expiresAt).toBe(NOW + HOUR);
    expect(JSON.parse(grant.settings!.document!)).toEqual(written);
  });
});
