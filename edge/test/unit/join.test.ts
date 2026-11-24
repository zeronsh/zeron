import { describe, expect, it } from "vitest";
import type { Env } from "../../src/env";
import { NAME_MAX, tidyName } from "../../src/discovery";
import { inviteLink, joinRoute, typedAddress } from "../../src/join";

const env = (vars: Partial<Env>): Env => ({ AUTH_MODE: "workos", WORKOS_CLIENT_ID: "client_abc", WORKOS_API_KEY: "sk_test", ...vars }) as Env;

const get = (vars: Partial<Env>, at = "https://zeron.acme.com/", method = "GET") => {
  const url = new URL(at);
  return joinRoute(new Request(url, { method }), env(vars), url);
};

describe("/ on a self-hosted edge", () => {
  it("invites by name, the way on the app's invite link", async () => {
    const res = get({ SERVER_NAME: "Acme" })!;
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("text/html; charset=utf-8");
    const html = await res.text();
    expect(html).toContain("<title>Join Acme on Zeron</title>");
    expect(html).toContain('href="zeron://server?address=zeron.acme.com"');
    expect(html).toContain('<code id="address">zeron.acme.com</code>');
    expect(html).toContain("https://zeron.sh/#downloads");
  });

  it("goes by its address when unnamed", async () => {
    const html = await get({})!.text();
    expect(html).toContain("<title>Join zeron.acme.com on Zeron</title>");
    expect(html).toContain("This is a self-hosted Zeron server.");
  });

  it("hands out the address as it would be typed: the host over https, the origin over http", () => {
    expect(typedAddress(new URL("https://zeron.acme.com/"))).toBe("zeron.acme.com");
    expect(typedAddress(new URL("https://zeron.acme.com:8443/"))).toBe("zeron.acme.com:8443");
    expect(typedAddress(new URL("http://192.168.1.20:8787/"))).toBe("http://192.168.1.20:8787");
    expect(inviteLink("http://192.168.1.20:8787")).toBe("zeron://server?address=http%3A%2F%2F192.168.1.20%3A8787");
  });

  it("escapes the operator's name", async () => {
    const html = await get({ SERVER_NAME: `<img src=x onerror="alert(1)">` })!.text();
    expect(html).not.toContain("<img src=x");
    expect(html).toContain("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  });

  it("runs only its own script and style, and is never stored", () => {
    const res = get({})!;
    const csp = res.headers.get("content-security-policy")!;
    const nonce = /'nonce-([0-9a-f]+)'/.exec(csp)![1];
    expect(csp).toContain("default-src 'none'");
    expect(csp).toContain("frame-ancestors 'none'");
    expect(csp).not.toContain("unsafe-inline");
    expect(nonce).toHaveLength(32);
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(res.headers.get("x-content-type-options")).toBe("nosniff");
  });

  it("offers no way in until sign-in is set up, and never names the API key", async () => {
    const res = get({ WORKOS_API_KEY: undefined, SERVER_NAME: "Acme" })!;
    expect(res.status).toBe(503);
    const html = await res.text();
    expect(html).toContain("Acme isn't ready yet");
    expect(html).not.toContain("zeron://");
    expect(html).not.toContain("WORKOS_API_KEY");
    expect(await get({ WORKOS_API_KEY: "sk_live_secret" })!.text()).not.toContain("sk_live_secret");
  });

  it("says a development edge is one, and offers no way in", async () => {
    const html = await get({ AUTH_MODE: "dev" })!.text();
    expect(html).toContain("is a development server");
    expect(html).not.toContain("zeron://");
  });

  it("leaves Zeron's own hosts, other paths and other methods alone", async () => {
    expect(get({}, "https://edge.zeron.sh/")).toBeUndefined();
    expect(get({}, "https://zeron.sh/")).toBeUndefined();
    expect(get({}, "https://zeron.acme.com/health")).toBeUndefined();
    expect(get({}, "https://zeron.acme.com/", "POST")).toBeUndefined();
    const head = get({}, "https://zeron.acme.com/", "HEAD")!;
    expect(head.status).toBe(200);
    expect(await head.text()).toBe("");
  });
});

describe("tidyName", () => {
  it("keeps a name on one line, the way the apps show it", () => {
    expect(tidyName("  Acme\n  Corp\t")).toBe("Acme Corp");
    expect(tidyName("‮emoc.emca")).toBe("emoc.emca");
    expect(tidyName(" ⁦ ")).toBeUndefined();
    expect(tidyName(undefined)).toBeUndefined();
    const long = tidyName("a".repeat(NAME_MAX + 5))!;
    expect([...long]).toHaveLength(NAME_MAX + 1);
    expect(long.endsWith("…")).toBe(true);
  });
});
