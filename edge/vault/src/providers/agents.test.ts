import { describe, expect, it, vi } from "vitest";
import { copilotAdapter, parseCopilotUpload } from "./copilot";
import { devinAdapter, isDevinUrl, parseDevinUpload } from "./devin";
import { grokAdapter, grokRefreshDue, isXaiIssuer, parseGrokRefresh, parseGrokUpload, refreshGrok, type GrokSecret } from "./grok";
import { keyAdapter } from "./keys";
import type { FetchFn } from "./types";

const NOW = Date.parse("2026-11-18T12:00:00Z");
const HOUR = 60 * 60_000;

const grokEntry = (overrides: Record<string, unknown> = {}) => ({
  key: "xai-access-1",
  refresh_token: "xai-refresh-1",
  expires_at: new Date(NOW + HOUR).toISOString(),
  oidc_issuer: "https://auth.x.ai",
  oidc_client_id: "client-1",
  user_id: "user-1",
  email: "ada@x.ai",
  first_name: "Ada",
  subscription_tier: "SUBSCRIPTION_TIER_SUPER_GROK_HEAVY",
  ...overrides
});

const grokSecret = (overrides: Partial<GrokSecret> = {}): GrokSecret => {
  const parsed = parseGrokUpload({ grokAuth: grokEntry(), scope: "https://auth.x.ai::client-1" }, NOW);
  if (!parsed.ok) throw new Error(parsed.message);
  return { ...parsed.secret, ...overrides };
};

const jsonResponse = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

describe("api keys", () => {
  it("every vendor key is a plain key; only Cursor's may expire", () => {
    const openrouter = keyAdapter("openrouter-key");
    const parsed = openrouter.parseUpload!({ key: "  sk-or-v1-abcdefgh1234  " }, NOW);
    expect(parsed).toEqual({ ok: true, secret: { key: "sk-or-v1-abcdefgh1234" } });
    expect(openrouter.parseUpload!({ key: "sk-or-v1-abcdefgh1234", expiresAt: NOW + HOUR }, NOW).ok).toBe(false);
    expect(openrouter.parseUpload!({ key: "has spaces in it" }, NOW).ok).toBe(false);
    expect(openrouter.parseUpload!({ copilotToken: "x" } as never, NOW).ok).toBe(false);
  });

  it("a Cursor sign-in key expires: grants never outlive it, and it hard-expires", () => {
    const cursor = keyAdapter("cursor-key");
    const parsed = cursor.parseUpload!({ key: "crsr_abcdefgh12345678", expiresAt: NOW + 10 * 60_000 }, NOW);
    if (!parsed.ok) throw new Error(parsed.message);
    expect(cursor.hardExpiresAt!(parsed.secret)).toBe(NOW + 10 * 60_000);
    expect(cursor.grant(parsed.secret, NOW).expiresAt).toBe(NOW + 10 * 60_000);
    expect(cursor.grant({ key: "crsr_abcdefgh12345678" }, NOW).expiresAt).toBe(NOW + HOUR);
    expect(cursor.parseUpload!({ key: "crsr_abcdefgh12345678", expiresAt: NOW - 1 }, NOW).ok).toBe(false);
    expect(cursor.parseUpload!({ key: "crsr_abcdefgh12345678", expiresAt: 1234 }, NOW).ok).toBe(false);
  });
});

describe("copilot", () => {
  it("keeps the token with its GitHub login, the login being the identity", async () => {
    const parsed = parseCopilotUpload({ copilotToken: "gho_abcdefghijklmnopqrstuvwxyz0123", login: "Octo-Cat" });
    if (!parsed.ok) throw new Error(parsed.message);
    expect(copilotAdapter.account(parsed.secret)).toBe("Octo-Cat");
    expect(await copilotAdapter.identity!(parsed.secret, {})).toBe("login:octo-cat");
    expect(copilotAdapter.grant(parsed.secret, NOW)).toEqual({
      accessToken: "gho_abcdefghijklmnopqrstuvwxyz0123",
      expiresAt: NOW + HOUR,
      account: "Octo-Cat"
    });
    expect(parseCopilotUpload({ copilotToken: "gho_abcdefghijklmnopqrstuvwxyz0123", login: "-bad" }).ok).toBe(false);
    expect(parseCopilotUpload({ copilotToken: "short", login: "octocat" }).ok).toBe(false);
  });
});

describe("devin", () => {
  it("hands out the key as the token and the rest of the file as settings", async () => {
    const parsed = parseDevinUpload({
      devinCredentials: {
        windsurf_api_key: "devin-key-0123456789",
        api_server_url: "https://server.codeium.com",
        auth_kind: "browser",
        ignored_number: 3
      }
    });
    if (!parsed.ok) throw new Error(parsed.message);
    const grant = devinAdapter.grant(parsed.secret, NOW);
    expect(grant.accessToken).toBe("devin-key-0123456789");
    expect(grant.settings).toEqual({ api_server_url: "https://server.codeium.com", auth_kind: "browser" });
    expect(await devinAdapter.identity!(parsed.secret, {})).toMatch(/^key:[0-9a-f]{16}$/);
  });

  it("refuses a server that isn't Devin's: the key would be sent there", () => {
    const upload = (url: string) =>
      parseDevinUpload({ devinCredentials: { windsurf_api_key: "devin-key-0123456789", api_server_url: url } });
    expect(upload("https://evil.example").ok).toBe(false);
    expect(upload("http://server.codeium.com").ok).toBe(false);
    expect(upload("https://devin.ai.evil.example").ok).toBe(false);
    expect(upload("https://api.devin.ai").ok).toBe(true);
    expect(isDevinUrl("https://windsurf.com:8443")).toBe(false);
    expect(parseDevinUpload({ devinCredentials: { api_server_url: "https://api.devin.ai" } }).ok).toBe(false);
  });
});

describe("grok", () => {
  it("pins the refresh token to xAI", () => {
    expect(isXaiIssuer("https://auth.x.ai")).toBe(true);
    expect(isXaiIssuer("https://auth.x.ai/")).toBe(true);
    for (const bad of ["http://auth.x.ai", "https://auth.x.ai.evil", "https://evilx.ai", "https://auth.x.ai/path", "https://auth.x.ai:444"]) {
      expect(isXaiIssuer(bad), bad).toBe(false);
    }
    const evil = parseGrokUpload({ grokAuth: grokEntry({ oidc_issuer: "https://evil.example" }), scope: "https://evil.example::c" }, NOW);
    expect(evil.ok).toBe(false);
  });

  it("grants the access token with the entry's non-secret fields", async () => {
    const secret = grokSecret();
    const grant = grokAdapter.grant(secret, NOW);
    expect(grant.accessToken).toBe("xai-access-1");
    expect(grant.expiresAt).toBe(NOW + HOUR);
    expect(grant.account).toBe("ada@x.ai");
    expect(grant.settings).toMatchObject({
      scope: "https://auth.x.ai::client-1",
      oidc_issuer: "https://auth.x.ai",
      oidc_client_id: "client-1",
      user_id: "user-1",
      subscription_tier: "SUBSCRIPTION_TIER_SUPER_GROK_HEAVY"
    });
    expect(Object.values(grant.settings!)).not.toContain("xai-refresh-1");
    expect(await grokAdapter.identity!(secret, {})).toBe("grok:user-1");
  });

  it("refreshes ahead of expiry: half the token's life, at most 30 minutes", () => {
    const secret = grokSecret();
    expect(grokRefreshDue(secret, NOW)).toBe(false);
    expect(grokRefreshDue(secret, NOW + 31 * 60_000)).toBe(true);
    // A refreshed token's life is known: half of a short one.
    const short = grokSecret({ issuedAt: NOW, expiresAt: NOW + 10 * 60_000 });
    expect(grokRefreshDue(short, NOW + 4 * 60_000)).toBe(false);
    expect(grokRefreshDue(short, NOW + 6 * 60_000)).toBe(true);
    // An upload's isn't (it may be nearly spent): the fixed margin.
    expect(grokSecret().issuedAt).toBe(0);
    expect(grokRefreshDue(grokSecret({ expiresAt: NOW + 20 * 60_000 }), NOW)).toBe(true);
    const created = parseGrokUpload(
      { grokAuth: grokEntry({ create_time: new Date(NOW - 50 * 60_000).toISOString() }), scope: "https://auth.x.ai::client-1" },
      NOW
    );
    expect(created.ok && created.secret.issuedAt).toBe(NOW - 50 * 60_000);
    // An upload that named no expiry is refreshed at its first grant.
    expect(grokRefreshDue(grokSecret({ expiresAt: 0 }), NOW)).toBe(true);
  });

  it("refreshes at the issuer with the entry's client id, keeping a rotated refresh token", async () => {
    const fetchFn = vi.fn<FetchFn>(async () => jsonResponse(200, { access_token: "xai-access-2", refresh_token: "xai-refresh-2", expires_in: 1800 }));
    const outcome = await refreshGrok(grokSecret(), { fetch: fetchFn, now: NOW });
    expect(fetchFn).toHaveBeenCalledOnce();
    const [url, init] = fetchFn.mock.calls[0]!;
    expect(url).toBe("https://auth.x.ai/oauth2/token");
    expect(Object.fromEntries(new URLSearchParams(String(init?.body)))).toEqual({
      grant_type: "refresh_token",
      refresh_token: "xai-refresh-1",
      client_id: "client-1"
    });
    expect(outcome).toMatchObject({
      kind: "ok",
      secret: { accessToken: "xai-access-2", refreshToken: "xai-refresh-2", expiresAt: NOW + 1800_000, issuedAt: NOW }
    });
  });

  it("classifies refusals: a dead refresh token needs a reconnect, a 5xx is retried", () => {
    const previous = grokSecret();
    expect(parseGrokRefresh(400, { error: "invalid_grant" }, previous, NOW).kind).toBe("reconnect");
    expect(parseGrokRefresh(401, undefined, previous, NOW).kind).toBe("reconnect");
    expect(parseGrokRefresh(503, undefined, previous, NOW).kind).toBe("transient");
    expect(parseGrokRefresh(200, { token_type: "bearer" }, previous, NOW).kind).toBe("transient");
    const kept = parseGrokRefresh(200, { access_token: "a2" }, previous, NOW);
    expect(kept.kind === "ok" && kept.secret.refreshToken).toBe("xai-refresh-1");
  });
});
