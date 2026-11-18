import { describe, expect, it } from "vitest";
import { b64urlEncode, verifyEd25519 } from "../../cloud/crypto";
import {
  deployKeyProofMessage,
  deployKeyRoute,
  parseZeronKeyAuthorization,
  publishSigningString,
  sha256Hex
} from "./signing";

const newPair = async () => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  const sign = async (message: string) =>
    b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, new TextEncoder().encode(message))));
  return { key, sign };
};

describe("the ZeronKey authorization header", () => {
  it("parses the contract's form, in any order", async () => {
    const { key, sign } = await newPair();
    const sig = await sign("x");
    expect(parseZeronKeyAuthorization(`ZeronKey key=${key}, ts=1700000000000, sig=${sig}`)).toEqual({ key, ts: "1700000000000", sig });
    expect(parseZeronKeyAuthorization(`zeronkey sig=${sig},key=${key} ,ts=1`)).toEqual({ key, ts: "1", sig });
  });

  it("refuses anything else", async () => {
    const { key, sign } = await newPair();
    const sig = await sign("x");
    const bad = [
      null,
      "",
      `Bearer ${key}`,
      `ZeronKey key=${key}, ts=1`,
      `ZeronKey key=${key}, ts=1, sig=${sig}, extra=1`,
      `ZeronKey key=${key}, key=${key}, ts=1, sig=${sig}`,
      `ZeronKey key=${key}, ts=-1, sig=${sig}`,
      `ZeronKey key=${key}, ts=1.5, sig=${sig}`,
      `ZeronKey key=${key}, ts=12345678901234567, sig=${sig}`,
      `ZeronKey key=${key}=, ts=1, sig=${sig}`,
      `ZeronKey key=${key.slice(0, -1)}, ts=1, sig=${sig}`,
      `ZeronKey key=${key}, ts=1, sig=${sig.slice(0, -1)}`,
      `ZeronKey key="${key}", ts=1, sig=${sig}`,
      `ZeronKeykey=${key}, ts=1, sig=${sig}`
    ];
    for (const header of bad) expect(parseZeronKeyAuthorization(header), String(header)).toBeUndefined();
  });
});

describe("the signing string", () => {
  it("is the contract's, byte for byte", async () => {
    const body = await sha256Hex(new Uint8Array([1, 2, 3]));
    expect(body).toBe("039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81");
    expect(publishSigningString("POST", "/store/v1/plugins/git/versions", body, "1700000000000")).toBe(
      `zeron-plugin-publish:v1\nPOST\n/store/v1/plugins/git/versions\n${body}\n1700000000000`
    );
    expect(await sha256Hex("")).toBe("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
  });

  it("verifies with WebCrypto, and not for another path, body or time", async () => {
    const { key, sign } = await newPair();
    const body = await sha256Hex("package");
    const message = publishSigningString("POST", "/store/v1/plugins/git/versions", body, "1");
    const sig = await sign(message);
    expect(await verifyEd25519(key, message, sig)).toBe(true);
    expect(await verifyEd25519(key, publishSigningString("POST", "/store/v1/plugins/gh/versions", body, "1"), sig)).toBe(false);
    expect(await verifyEd25519(key, publishSigningString("POST", "/store/v1/plugins/git/versions", await sha256Hex("other"), "1"), sig)).toBe(false);
    expect(await verifyEd25519(key, publishSigningString("POST", "/store/v1/plugins/git/versions", body, "2"), sig)).toBe(false);
  });

  it("names the plugin in a deploy key's proof", () => {
    expect(deployKeyProofMessage("git", "KEY")).toBe("zeron-deploy-key:v1:git:KEY");
  });
});

describe("the routes a deploy key may use", () => {
  it("are its plugin's publishing routes, by POST", () => {
    expect(deployKeyRoute("POST", "/store/v1/plugins/git/versions")).toEqual({ pluginId: "git" });
    for (const action of ["withdraw", "yank", "unyank"]) {
      expect(deployKeyRoute("POST", `/store/v1/plugins/git/versions/1.2.0/${action}`)).toEqual({ pluginId: "git" });
    }
  });

  it("are nothing else", () => {
    const refused: [string, string][] = [
      ["GET", "/store/v1/plugins/git/versions"],
      ["PUT", "/store/v1/plugins/git/versions"],
      ["POST", "/store/v1/plugins/git/keys"],
      ["DELETE", "/store/v1/plugins/git/keys/abc"],
      ["POST", "/store/v1/plugins/git/installed"],
      ["POST", "/store/v1/plugins/git/reports"],
      ["POST", "/store/v1/me/plugins"],
      ["POST", "/store/v1/admin/submissions/git/1.0.0/approve"],
      ["POST", "/store/v1/plugins/git/versions/1.0.0/remove"],
      ["POST", "/store/v1/plugins/git/versions//yank"],
      ["POST", "/store/v1/plugins/git/versions/"],
      ["POST", "/store/v1/plugins/Git/versions"],
      ["POST", "/store/v1/plugins/git%2Fx/versions"],
      ["POST", "/store/v1/plugins/../versions"],
      ["POST", "//store/v1/plugins/git/versions"],
      ["POST", "/store/v2/plugins/git/versions"]
    ];
    for (const [method, path] of refused) expect(deployKeyRoute(method, path), `${method} ${path}`).toBeUndefined();
  });
});
