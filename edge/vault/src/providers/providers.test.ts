import { describe, expect, it, vi } from "vitest";
import { fakeJwt } from "../../test/support";
import {
  CODEX_CLIENT_ID,
  CODEX_TOKEN_URL,
  codexAdapter,
  codexRefreshDue,
  parseCodexRefresh,
  parseCodexUpload,
  refreshCodex,
  type CodexSecret
} from "./codex";
import {
  DEVICE_GRANT_TYPE,
  GITHUB_TOKEN_URL,
  githubAdapter,
  githubRefreshDue,
  parseDeviceCode,
  parseDevicePoll,
  parseGithubRefresh,
  parseGithubToken,
  refreshGithub,
  type GithubSecret
} from "./github";
import { anthropicKeyAdapter, openaiKeyAdapter } from "./keys";
import {
  CLAUDE_CLIENT_ID,
  CLAUDE_TOKEN_URL,
  claudeAdapter,
  claudeRefreshDue,
  parseClaudeRefresh,
  parseClaudeUpload,
  refreshClaude,
  type ClaudeSecret
} from "./claude";
import type { FetchFn } from "./types";

const NOW = Date.parse("2026-10-01T12:00:00Z");
const HOUR = 60 * 60_000;
const DAY = 24 * HOUR;
const sec = (ms: number) => Math.floor(ms / 1000);

const idToken = fakeJwt({
  email: "ada@example.com",
  "https://api.openai.com/auth": { chatgpt_account_id: "acct-from-claims" }
});

const codexSecret = (overrides: Partial<CodexSecret> = {}): CodexSecret => ({
  idToken,
  accessToken: fakeJwt({ exp: sec(NOW + 5 * DAY) }),
  refreshToken: "rt-1",
  accountId: "acct-1",
  lastRefresh: NOW - DAY,
  ...overrides
});

const jsonResponse = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

describe("codex", () => {
  it("parses auth.json and labels by id_token email", () => {
    const parsed = parseCodexUpload(
      {
        authJson: {
          OPENAI_API_KEY: null,
          tokens: { id_token: idToken, access_token: "at", refresh_token: "rt", account_id: "acct-1" },
          last_refresh: "2026-09-30T12:00:00Z"
        }
      },
      NOW
    );
    expect(parsed).toEqual({
      ok: true,
      secret: { idToken, accessToken: "at", refreshToken: "rt", accountId: "acct-1", lastRefresh: NOW - DAY }
    });
    if (parsed.ok) expect(codexAdapter.account(parsed.secret)).toBe("ada@example.com");
  });

  it("falls back to the id_token claim for account_id and to now for last_refresh", () => {
    const parsed = parseCodexUpload(
      { authJson: { tokens: { id_token: idToken, access_token: "at", refresh_token: "rt" } } },
      NOW
    );
    expect(parsed.ok && parsed.secret.accountId).toBe("acct-from-claims");
    expect(parsed.ok && parsed.secret.lastRefresh).toBe(NOW);
  });

  it("rejects uploads without a token set", () => {
    expect(parseCodexUpload({ key: "sk-x" }, NOW).ok).toBe(false);
    expect(parseCodexUpload({ authJson: { OPENAI_API_KEY: "sk-x" } }, NOW).ok).toBe(false);
    expect(parseCodexUpload({ authJson: { tokens: { access_token: "at" } } }, NOW).ok).toBe(false);
  });

  it("is due when the access token has under an hour left", () => {
    expect(codexRefreshDue(codexSecret({ accessToken: fakeJwt({ exp: sec(NOW + 2 * HOUR) }) }), NOW)).toBe(false);
    expect(codexRefreshDue(codexSecret({ accessToken: fakeJwt({ exp: sec(NOW + 30 * 60_000) }) }), NOW)).toBe(true);
    expect(codexRefreshDue(codexSecret({ accessToken: fakeJwt({ exp: sec(NOW - HOUR) }) }), NOW)).toBe(true);
  });

  it("without exp, is due only after 7 days since the last refresh", () => {
    expect(codexRefreshDue(codexSecret({ accessToken: "opaque", lastRefresh: NOW - 6 * DAY }), NOW)).toBe(false);
    expect(codexRefreshDue(codexSecret({ accessToken: "opaque", lastRefresh: NOW - 8 * DAY }), NOW)).toBe(true);
  });

  it("grants expire at the JWT exp, else in an hour", () => {
    const exp = sec(NOW + 5 * DAY) * 1000;
    expect(codexAdapter.grant(codexSecret(), NOW)).toMatchObject({ expiresAt: exp, accountId: "acct-1", idToken });
    expect(codexAdapter.grant(codexSecret({ accessToken: "opaque" }), NOW).expiresAt).toBe(NOW + HOUR);
  });

  it("merges a refresh response, keeping absent fields", () => {
    const previous = codexSecret();
    expect(parseCodexRefresh(200, { access_token: "at-2" }, previous, NOW)).toEqual({
      kind: "ok",
      secret: { ...previous, accessToken: "at-2", lastRefresh: NOW }
    });
    const rotated = parseCodexRefresh(200, { access_token: "at-2", refresh_token: "rt-2", id_token: idToken }, previous, NOW);
    expect(rotated.kind === "ok" && rotated.secret).toMatchObject({
      refreshToken: "rt-2",
      accountId: "acct-from-claims"
    });
  });

  it("classifies refresh failures", () => {
    const previous = codexSecret();
    expect(parseCodexRefresh(400, { error: "invalid_grant" }, previous, NOW).kind).toBe("reconnect");
    expect(parseCodexRefresh(401, { error: { code: "refresh_token_expired" } }, previous, NOW).kind).toBe("reconnect");
    expect(parseCodexRefresh(400, { error: { code: "refresh_token_reused" } }, previous, NOW).kind).toBe("reconnect");
    expect(parseCodexRefresh(401, undefined, previous, NOW).kind).toBe("reconnect");
    expect(parseCodexRefresh(503, undefined, previous, NOW).kind).toBe("transient");
    expect(parseCodexRefresh(429, { error: "rate_limited" }, previous, NOW).kind).toBe("transient");
    expect(parseCodexRefresh(200, {}, previous, NOW).kind).toBe("transient");
    expect(parseCodexRefresh(200, undefined, previous, NOW).kind).toBe("transient");
  });

  it("posts the documented refresh request", async () => {
    const fetchMock = vi.fn<FetchFn>(async () => jsonResponse(200, { access_token: "at-2" }));
    const outcome = await refreshCodex(codexSecret(), { fetch: fetchMock, now: NOW });
    expect(outcome.kind).toBe("ok");
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe(CODEX_TOKEN_URL);
    expect(init?.method).toBe("POST");
    expect(JSON.parse(init?.body as string)).toEqual({
      client_id: CODEX_CLIENT_ID,
      grant_type: "refresh_token",
      refresh_token: "rt-1",
      scope: "openid profile email"
    });
  });

  it("treats network errors as transient", async () => {
    const outcome = await refreshCodex(codexSecret(), {
      fetch: async () => {
        throw new TypeError("network down");
      },
      now: NOW
    });
    expect(outcome.kind).toBe("transient");
  });
});

describe("github", () => {
  const secret = (overrides: Partial<GithubSecret> = {}): GithubSecret => ({
    accessToken: "ghu_1",
    expiresAt: NOW + 8 * HOUR,
    refreshToken: "ghr_1",
    refreshTokenExpiresAt: NOW + 180 * DAY,
    login: "octocat",
    lastRefresh: NOW,
    ...overrides
  });

  it("parses token responses (expiring and non-expiring)", () => {
    expect(
      parseGithubToken(
        { access_token: "ghu_1", expires_in: 28800, refresh_token: "ghr_1", refresh_token_expires_in: 15897600 },
        NOW,
        "octocat"
      )
    ).toEqual(secret({ refreshTokenExpiresAt: NOW + 15897600 * 1000 }));
    expect(parseGithubToken({ access_token: "gho_1", token_type: "bearer" }, NOW, "octocat")).toEqual({
      accessToken: "gho_1",
      expiresAt: undefined,
      refreshToken: undefined,
      refreshTokenExpiresAt: undefined,
      login: "octocat",
      lastRefresh: NOW
    });
    expect(parseGithubToken({ error: "bad" }, NOW, undefined)).toBeUndefined();
  });

  it("refreshes only expiring tokens under 10 minutes from expiry", () => {
    expect(githubRefreshDue(secret({ expiresAt: NOW + 11 * 60_000 }), NOW)).toBe(false);
    expect(githubRefreshDue(secret({ expiresAt: NOW + 9 * 60_000 }), NOW)).toBe(true);
    expect(githubRefreshDue(secret({ expiresAt: undefined, refreshToken: undefined }), NOW)).toBe(false);
    expect(githubAdapter.refreshable(secret({ expiresAt: undefined, refreshToken: undefined }))).toBe(false);
  });

  it("non-expiring grants still expire hourly", () => {
    expect(githubAdapter.grant(secret({ expiresAt: undefined }), NOW)).toEqual({
      accessToken: "ghu_1",
      expiresAt: NOW + HOUR,
      account: "octocat"
    });
  });

  it("classifies refresh responses (errors arrive as HTTP 200)", () => {
    const previous = secret();
    expect(parseGithubRefresh(200, { error: "bad_refresh_token" }, previous, NOW).kind).toBe("reconnect");
    expect(parseGithubRefresh(200, { error: "incorrect_client_credentials" }, previous, NOW).kind).toBe("transient");
    expect(parseGithubRefresh(502, undefined, previous, NOW).kind).toBe("transient");
    const refreshed = parseGithubRefresh(200, { access_token: "ghu_2", expires_in: 28800 }, previous, NOW);
    expect(refreshed.kind === "ok" && refreshed.secret).toMatchObject({
      accessToken: "ghu_2",
      refreshToken: "ghr_1",
      login: "octocat"
    });
  });

  it("refreshes with the App's client credentials", async () => {
    const fetchMock = vi.fn<FetchFn>(async () =>
      jsonResponse(200, { access_token: "ghu_2", expires_in: 28800, refresh_token: "ghr_2" })
    );
    const outcome = await refreshGithub(secret(), {
      fetch: fetchMock,
      now: NOW,
      githubClientId: "Iv1.client",
      githubClientSecret: "shh"
    });
    expect(outcome.kind === "ok" && outcome.secret.refreshToken).toBe("ghr_2");
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe(GITHUB_TOKEN_URL);
    expect(Object.fromEntries(new URLSearchParams(init?.body as string))).toEqual({
      client_id: "Iv1.client",
      client_secret: "shh",
      grant_type: "refresh_token",
      refresh_token: "ghr_1"
    });
  });

  it("without App secrets a refresh is transient, never a reconnect", async () => {
    const outcome = await refreshGithub(secret(), { fetch: vi.fn<FetchFn>(), now: NOW });
    expect(outcome.kind).toBe("transient");
  });

  it("parses the device code response", () => {
    expect(
      parseDeviceCode(200, {
        device_code: "dc",
        user_code: "ABCD-1234",
        verification_uri: "https://github.com/login/device",
        expires_in: 899,
        interval: 5
      })
    ).toEqual({
      ok: true,
      start: {
        deviceCode: "dc",
        userCode: "ABCD-1234",
        verificationUri: "https://github.com/login/device",
        expiresInSecs: 899,
        intervalSecs: 5
      }
    });
    expect(parseDeviceCode(200, { error: "device_flow_disabled" })).toEqual({ ok: false, reason: "device_flow_disabled" });
  });

  it("parses device poll answers", () => {
    expect(parseDevicePoll(200, { error: "authorization_pending" }, 5)).toEqual({ kind: "pending" });
    expect(parseDevicePoll(200, { error: "slow_down" }, 5)).toEqual({ kind: "slow_down", intervalSecs: 10 });
    expect(parseDevicePoll(200, { error: "slow_down", interval: 15 }, 5)).toEqual({ kind: "slow_down", intervalSecs: 15 });
    expect(parseDevicePoll(200, { error: "expired_token" }, 5)).toEqual({ kind: "expired" });
    expect(parseDevicePoll(200, { error: "access_denied" }, 5)).toEqual({ kind: "denied" });
    expect(parseDevicePoll(200, { error: "incorrect_device_code" }, 5)).toEqual({
      kind: "failed",
      error: "incorrect_device_code"
    });
    expect(parseDevicePoll(503, undefined, 5).kind).toBe("transient");
    expect(parseDevicePoll(200, { access_token: "ghu_1" }, 5)).toEqual({ kind: "token", body: { access_token: "ghu_1" } });
    expect(DEVICE_GRANT_TYPE).toBe("urn:ietf:params:oauth:grant-type:device_code");
  });
});

describe("api keys", () => {
  it("parses, labels with the last 4 characters, never refreshes", () => {
    const parsed = anthropicKeyAdapter.parseUpload!({ key: "  sk-ant-api03-abcdef1234  " }, NOW);
    expect(parsed).toEqual({ ok: true, secret: { key: "sk-ant-api03-abcdef1234" } });
    if (!parsed.ok) return;
    expect(anthropicKeyAdapter.account(parsed.secret)).toBe("…1234");
    expect(anthropicKeyAdapter.refreshDue(parsed.secret, NOW)).toBe(false);
    expect(anthropicKeyAdapter.grant(parsed.secret, NOW)).toEqual({
      accessToken: "sk-ant-api03-abcdef1234",
      expiresAt: NOW + HOUR,
      account: "…1234"
    });
  });

  it("rejects malformed keys and the wrong material", () => {
    expect(openaiKeyAdapter.parseUpload!({ key: "short" }, NOW).ok).toBe(false);
    expect(openaiKeyAdapter.parseUpload!({ key: "sk-has space-in-it" }, NOW).ok).toBe(false);
    expect(openaiKeyAdapter.parseUpload!({ authJson: {} }, NOW).ok).toBe(false);
  });
});

describe("claude", () => {
  const oauth = (overrides: Partial<Extract<ClaudeSecret, { kind: "oauth" }>> = {}): Extract<ClaudeSecret, { kind: "oauth" }> => ({
    kind: "oauth",
    accessToken: "sk-ant-oat01-a",
    refreshToken: "sk-ant-ort01-r",
    expiresAt: NOW + HOUR,
    scopes: ["user:inference", "user:profile"],
    subscriptionType: "max",
    lastRefresh: NOW,
    ...overrides
  });

  it("parses the Claude Code credential blob", () => {
    const parsed = parseClaudeUpload(
      {
        claudeAiOauth: {
          accessToken: "sk-ant-oat01-a",
          refreshToken: "sk-ant-ort01-r",
          expiresAt: NOW + HOUR,
          scopes: ["user:inference", "user:profile"],
          subscriptionType: "max"
        }
      },
      NOW
    );
    expect(parsed).toEqual({ ok: true, secret: oauth() });
    if (parsed.ok) expect(claudeAdapter.account(parsed.secret)).toBe("max");
    // A label that is not a plain word is dropped, never echoed.
    const odd = parseClaudeUpload(
      { claudeAiOauth: { accessToken: "a", refreshToken: "r", expiresAt: NOW + HOUR, subscriptionType: "sk-ant-oat01 secret" } },
      NOW
    );
    expect(odd.ok && claudeAdapter.account(odd.secret)).toBe("Claude subscription");
  });

  it("parses setup tokens with a one-year default expiry", () => {
    expect(parseClaudeUpload({ oauthToken: "sk-ant-oat01-setup-0123456789" }, NOW)).toEqual({
      ok: true,
      secret: { kind: "setup-token", token: "sk-ant-oat01-setup-0123456789", expiresAt: NOW + 365 * DAY }
    });
    expect(parseClaudeUpload({ oauthToken: "sk-ant-oat01-setup-0123456789", expiresAt: NOW + DAY }, NOW)).toMatchObject({
      ok: true,
      secret: { expiresAt: NOW + DAY }
    });
    expect(parseClaudeUpload({ oauthToken: "sk-ant-oat01-setup-0123456789", expiresAt: NOW - 1 }, NOW).ok).toBe(false);
    expect(parseClaudeUpload({ oauthToken: "sk-ant-api03-not-oauth-0000" }, NOW).ok).toBe(false);
    expect(parseClaudeUpload({ key: "sk-ant-oat01-setup-0123456789" }, NOW).ok).toBe(false);
  });

  it("refreshes OAuth logins under 10 minutes from expiry; setup tokens never", () => {
    expect(claudeRefreshDue(oauth({ expiresAt: NOW + 11 * 60_000 }), NOW)).toBe(false);
    expect(claudeRefreshDue(oauth({ expiresAt: NOW + 9 * 60_000 }), NOW)).toBe(true);
    const setup: ClaudeSecret = { kind: "setup-token", token: "sk-ant-oat01-x", expiresAt: NOW + 60_000 };
    expect(claudeRefreshDue(setup, NOW)).toBe(false);
    expect(claudeAdapter.refreshable(setup)).toBe(false);
    expect(claudeAdapter.hardExpiresAt!(setup)).toBe(NOW + 60_000);
    expect(claudeAdapter.hardExpiresAt!(oauth())).toBeUndefined();
  });

  it("grants carry scopes and plan, never the refresh token", () => {
    const grant = claudeAdapter.grant(oauth(), NOW);
    expect(grant).toEqual({
      accessToken: "sk-ant-oat01-a",
      expiresAt: NOW + HOUR,
      scopes: ["user:inference", "user:profile"],
      subscriptionType: "max"
    });
    expect(JSON.stringify(grant)).not.toContain("sk-ant-ort01-r");
  });

  it("merges refresh answers: rotation, kept refresh token, scope, default lifetime", () => {
    const previous = oauth();
    expect(parseClaudeRefresh(200, { access_token: "a2", refresh_token: "r2", expires_in: 7200, scope: "user:inference" }, previous, NOW)).toEqual({
      kind: "ok",
      secret: { ...previous, accessToken: "a2", refreshToken: "r2", expiresAt: NOW + 2 * HOUR, scopes: ["user:inference"], lastRefresh: NOW }
    });
    expect(parseClaudeRefresh(200, { access_token: "a2" }, previous, NOW)).toEqual({
      kind: "ok",
      secret: { ...previous, accessToken: "a2", expiresAt: NOW + HOUR, lastRefresh: NOW }
    });
  });

  it("classifies refresh failures", () => {
    const previous = oauth();
    expect(parseClaudeRefresh(400, { error: "invalid_grant" }, previous, NOW).kind).toBe("reconnect");
    expect(parseClaudeRefresh(401, { error: "invalid_grant" }, previous, NOW).kind).toBe("reconnect");
    expect(parseClaudeRefresh(400, { type: "error", error: { type: "invalid_grant" } }, previous, NOW).kind).toBe("reconnect");
    expect(parseClaudeRefresh(401, undefined, previous, NOW).kind).toBe("reconnect");
    expect(parseClaudeRefresh(400, { error: "invalid_request" }, previous, NOW).kind).toBe("transient");
    expect(parseClaudeRefresh(529, undefined, previous, NOW).kind).toBe("transient");
    expect(parseClaudeRefresh(200, {}, previous, NOW).kind).toBe("transient");
  });

  it("posts the documented refresh request", async () => {
    const fetchMock = vi.fn<FetchFn>(async () => jsonResponse(200, { access_token: "a2", expires_in: 3600 }));
    const outcome = await refreshClaude(oauth(), { fetch: fetchMock, now: NOW });
    expect(outcome.kind).toBe("ok");
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe(CLAUDE_TOKEN_URL);
    expect(init?.method).toBe("POST");
    expect(JSON.parse(init?.body as string)).toEqual({
      grant_type: "refresh_token",
      refresh_token: "sk-ant-ort01-r",
      client_id: CLAUDE_CLIENT_ID
    });
  });
});
