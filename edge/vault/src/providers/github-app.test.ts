import { describe, expect, it } from "vitest";
import { toBase64 } from "../encoding";
import { appJwt, installUrl, isRepoFullName, pkcs8Der } from "./github-app";

const pem = (label: string, der: Uint8Array) =>
  `-----BEGIN ${label}-----\n${toBase64(der).replace(/.{64}/g, "$&\n")}\n-----END ${label}-----\n`;

const rsaPair = async () =>
  (await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"]
  )) as CryptoKeyPair;

describe("GitHub App keys", () => {
  it("wraps GitHub's PKCS#1 download into the exact PKCS#8 WebCrypto exports, and passes PKCS#8 through", async () => {
    const pair = await rsaPair();
    const pkcs8 = new Uint8Array((await crypto.subtle.exportKey("pkcs8", pair.privateKey)) as ArrayBuffer);
    // PrivateKeyInfo = 30 82 LL LL | 02 01 00 | AlgorithmIdentifier (15) | 04 82 LL LL | RSAPrivateKey
    const pkcs1 = pkcs8.subarray(26);
    expect(pkcs8Der(pem("RSA PRIVATE KEY", pkcs1))).toEqual(pkcs8);
    expect(pkcs8Der(pem("PRIVATE KEY", pkcs8))).toEqual(pkcs8);
    // Squeezed onto one env-file line with literal \n escapes.
    expect(pkcs8Der(pem("RSA PRIVATE KEY", pkcs1).replace(/\n/g, "\\n"))).toEqual(pkcs8);
    expect(() => pkcs8Der(pem("EC PRIVATE KEY", pkcs1))).toThrow();
  });

  it("signs an RS256 App JWT that verifies, issued by the client id, under GitHub's 10-minute cap", async () => {
    const pair = await rsaPair();
    const pkcs8 = new Uint8Array((await crypto.subtle.exportKey("pkcs8", pair.privateKey)) as ArrayBuffer);
    const now = Date.UTC(2026, 9, 1, 12);
    const jwt = await appJwt("Iv23.client", pem("RSA PRIVATE KEY", pkcs8.subarray(26)), now);
    const [header, claims, signature] = jwt.split(".") as [string, string, string];
    const decode = (part: string) =>
      Uint8Array.from(atob(part.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
    const valid = await crypto.subtle.verify(
      "RSASSA-PKCS1-v1_5",
      pair.publicKey,
      decode(signature),
      new TextEncoder().encode(`${header}.${claims}`)
    );
    expect(valid).toBe(true);
    const payload = JSON.parse(new TextDecoder().decode(decode(claims)));
    expect(payload).toEqual({ iss: "Iv23.client", iat: now / 1000 - 60, exp: now / 1000 + 540 });
  });
});

describe("repository names", () => {
  it("accepts owner/name only", () => {
    for (const ok of ["octocat/Hello-World", "acme-inc/app.js", "a/b", "Org1/repo_name"]) {
      expect(isRepoFullName(ok)).toBe(true);
    }
    for (const bad of ["octocat", "/app", "octocat/", "-x/app", "a/b/c", "a b/c", "a/..%2f", "", 42]) {
      expect(isRepoFullName(bad)).toBe(false);
    }
  });

  it("builds the install link from the slug", () => {
    expect(installUrl("zeron")).toBe("https://github.com/apps/zeron/installations/new");
  });
});
