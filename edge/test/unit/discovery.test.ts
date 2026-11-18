import { describe, expect, it } from "vitest";
import type { Env } from "../../src/env";
import { DISCOVERY_PATH, discoveryRoute } from "../../src/discovery";

const env = (vars: Partial<Env>): Env => ({ AUTH_MODE: "workos", WORKOS_CLIENT_ID: "client_abc", WORKOS_API_KEY: "sk_test", ...vars }) as Env;

const get = (vars: Partial<Env>, path = DISCOVERY_PATH, method = "GET") => {
  const url = new URL(`https://zeron.example.com${path}`);
  return discoveryRoute(new Request(url, { method }), env(vars), url);
};

describe("/.well-known/zeron", () => {
  it("publishes the WorkOS client an app signs in through", async () => {
    const res = get({})!;
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("application/json");
    expect(res.headers.get("cache-control")).toBe("public, max-age=300");
    expect(await res.json()).toEqual({ zeron: 1, auth: { mode: "workos", workosClientId: "client_abc" } });
  });

  it("names the server when its operator does", async () => {
    expect(await get({ SERVER_NAME: "  Acme  " })!.json()).toMatchObject({ name: "Acme" });
    expect(await get({ SERVER_NAME: "   " })!.json()).not.toHaveProperty("name");
  });

  it("says a dev edge is one, with no client and no secret needed", async () => {
    const res = get({ AUTH_MODE: "dev", WORKOS_CLIENT_ID: "", WORKOS_API_KEY: undefined })!;
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ zeron: 1, auth: { mode: "dev" } });
  });

  it("names the missing settings instead of handing out a sign-in that fails later", async () => {
    const noKey = get({ WORKOS_API_KEY: undefined })!;
    expect(noKey.status).toBe(503);
    expect(noKey.headers.get("cache-control")).toBe("no-store");
    expect(await noKey.json()).toEqual({ zeron: 1, error: "sign_in_not_configured", missing: ["WORKOS_API_KEY"] });

    const nothing = get({ WORKOS_CLIENT_ID: " ", WORKOS_API_KEY: undefined })!;
    expect(await nothing.json()).toMatchObject({ missing: ["WORKOS_CLIENT_ID", "WORKOS_API_KEY"] });
  });

  it("never leaks the API key", async () => {
    const text = await get({ WORKOS_API_KEY: "sk_live_secret" })!.text();
    expect(text).not.toContain("sk_live_secret");
  });

  it("answers HEAD without a body and leaves other paths and methods alone", async () => {
    const head = get({}, DISCOVERY_PATH, "HEAD")!;
    expect(head.status).toBe(200);
    expect(await head.text()).toBe("");
    expect(get({}, `${DISCOVERY_PATH}/`)!.status).toBe(200);
    expect(get({}, "/.well-known/zeron-other")).toBeUndefined();
    expect(get({}, "/health")).toBeUndefined();
    expect(get({}, DISCOVERY_PATH, "POST")).toBeUndefined();
  });
});
