import { describe, expect, it } from "vitest";
import {
  LOGIN_COOKIE,
  SESSION_COOKIE,
  codeChallenge,
  crossSiteRefusal,
  hostCookie,
  loginCookieValue,
  newToken,
  parseLoginCookie,
  readCookie,
  safeReturnTo,
  sessionTokenHash,
  webOrigin
} from "./web-cookies";

const ORIGIN = "https://zeron.sh";
const env = { WEB_ORIGIN: ORIGIN };

const request = (headers: Record<string, string>, method = "POST") => new Request(`${ORIGIN}/store/v1/x`, { method, headers });

describe("safeReturnTo", () => {
  it("keeps a path and query on the web origin", () => {
    expect(safeReturnTo("/plugins/git?tab=versions", ORIGIN)).toBe("/plugins/git?tab=versions");
    expect(safeReturnTo("/", ORIGIN)).toBe("/");
    expect(safeReturnTo("/a/../plugins", ORIGIN)).toBe("/plugins");
    // The fragment never reaches a server; it's dropped.
    expect(safeReturnTo("/plugins#readme", ORIGIN)).toBe("/plugins");
    // An escaped slash stays a path segment on this origin.
    expect(safeReturnTo("/%2F%2Fevil.example", ORIGIN)).toBe("/%2F%2Fevil.example");
  });

  it("sends anything that could leave the origin to /", () => {
    const attacks = [
      null,
      undefined,
      "",
      "https://evil.example/",
      "//evil.example",
      "//evil.example/plugins",
      "/\\evil.example",
      "\\\\evil.example",
      "/\\/evil.example",
      "javascript:alert(1)",
      "evil.example",
      " /plugins",
      "/plugins\n//evil.example",
      "/\tevil",
      "/\u0000",
      `/${"a".repeat(3000)}`,
      "https:evil.example",
      "/auth/web/logout",
      "/auth/web/login?return_to=//evil.example",
      "/auth/web"
    ];
    for (const attack of attacks) expect(safeReturnTo(attack, ORIGIN), String(attack)).toBe("/");
  });
});

describe("readCookie", () => {
  it("finds one cookie among others", () => {
    expect(readCookie(request({ cookie: `a=1; ${SESSION_COOKIE}=tok; b=2` }), SESSION_COOKIE)).toBe("tok");
    expect(readCookie(request({ cookie: "a=1" }), SESSION_COOKIE)).toBeUndefined();
    expect(readCookie(request({}), SESSION_COOKIE)).toBeUndefined();
  });

  it("refuses a cookie sent twice (a planted twin can't be told apart)", () => {
    expect(readCookie(request({ cookie: `${SESSION_COOKIE}=one; ${SESSION_COOKIE}=two` }), SESSION_COOKIE)).toBeUndefined();
  });

  it("matches the whole name, not a prefix or suffix", () => {
    expect(readCookie(request({ cookie: `x${SESSION_COOKIE}=tok` }), SESSION_COOKIE)).toBeUndefined();
    expect(readCookie(request({ cookie: `${SESSION_COOKIE}x=tok` }), SESSION_COOKIE)).toBeUndefined();
  });
});

describe("cookies", () => {
  it("are __Host- cookies: Secure, Path=/, no Domain, HttpOnly, Lax", () => {
    const cookie = hostCookie(SESSION_COOKIE, "v", 90_000);
    expect(cookie).toBe(`${SESSION_COOKIE}=v; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=90`);
    expect(cookie).not.toMatch(/domain/i);
    expect(SESSION_COOKIE.startsWith("__Host-")).toBe(true);
    expect(LOGIN_COOKIE.startsWith("__Host-")).toBe(true);
    expect(hostCookie(SESSION_COOKIE, "", 0)).toContain("Max-Age=0");
  });

  it("hash only real tokens", async () => {
    const token = newToken();
    expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(await sessionTokenHash(token)).toMatch(/^[0-9a-f]{64}$/);
    for (const bad of ["", "x", `${token}=`, `${token.slice(0, 42)}!`, 42, null]) expect(await sessionTokenHash(bad)).toBeUndefined();
  });

  it("round-trip a login attempt and refuse a tampered one", async () => {
    const state = newToken();
    const verifier = newToken();
    const value = loginCookieValue(state, verifier, "/plugins/git?x=ü");
    expect(parseLoginCookie(value)).toEqual({ state, verifier, returnTo: "/plugins/git?x=ü" });
    expect(parseLoginCookie(undefined)).toBeUndefined();
    expect(parseLoginCookie(`${value}.extra`)).toBeUndefined();
    expect(parseLoginCookie(`short.${verifier}.Lw`)).toBeUndefined();
    expect(parseLoginCookie(`${state}.${verifier}.!!`)).toBeUndefined();
    // PKCE S256, RFC 7636's own example.
    expect(await codeChallenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")).toBe("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
  });
});

describe("crossSiteRefusal", () => {
  const csrf = newToken();

  it("passes a same-origin request carrying the session's token", () => {
    expect(crossSiteRefusal(request({ origin: ORIGIN }), env, csrf, csrf)).toBeUndefined();
    expect(crossSiteRefusal(request({ origin: ORIGIN, "sec-fetch-site": "same-origin" }), env, csrf, csrf)).toBeUndefined();
  });

  it("refuses a missing or foreign Origin, or a cross-site fetch", () => {
    expect(crossSiteRefusal(request({}), env, csrf, csrf)).toBe("bad_origin");
    expect(crossSiteRefusal(request({ origin: "https://evil.example" }), env, csrf, csrf)).toBe("bad_origin");
    expect(crossSiteRefusal(request({ origin: "null" }), env, csrf, csrf)).toBe("bad_origin");
    expect(crossSiteRefusal(request({ origin: "https://edge.zeron.sh" }), env, csrf, csrf)).toBe("bad_origin");
    expect(crossSiteRefusal(request({ origin: "http://zeron.sh" }), env, csrf, csrf)).toBe("bad_origin");
    expect(crossSiteRefusal(request({ origin: ORIGIN, "sec-fetch-site": "same-site" }), env, csrf, csrf)).toBe("bad_origin");
    expect(crossSiteRefusal(request({ origin: ORIGIN, "sec-fetch-site": "cross-site" }), env, csrf, csrf)).toBe("bad_origin");
    expect(crossSiteRefusal(request({ origin: ORIGIN }), {}, csrf, csrf)).toBe("bad_origin");
  });

  it("refuses a missing or wrong CSRF token", () => {
    expect(crossSiteRefusal(request({ origin: ORIGIN }), env, csrf, null)).toBe("bad_csrf");
    expect(crossSiteRefusal(request({ origin: ORIGIN }), env, csrf, newToken())).toBe("bad_csrf");
    expect(crossSiteRefusal(request({ origin: ORIGIN }), env, csrf, "")).toBe("bad_csrf");
  });
});

describe("webOrigin", () => {
  it("takes only a bare https origin", () => {
    expect(webOrigin({ WEB_ORIGIN: ORIGIN })).toBe(ORIGIN);
    for (const bad of [undefined, "", "http://zeron.sh", "https://zeron.sh/", "https://zeron.sh/path", "zeron.sh"]) {
      expect(webOrigin({ WEB_ORIGIN: bad }), String(bad)).toBeUndefined();
    }
  });
});
