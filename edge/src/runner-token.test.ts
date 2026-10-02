import { exportJWK, generateKeyPair } from "jose";
import { describe, expect, it } from "vitest";
import { verifyToken } from "./auth";
import type { Env } from "./env";
import { b64urlEncode, runnerTokenMessage, verifyEd25519 } from "./cloud/crypto";
import { devRunnerBearer, mintRunnerToken, parseDevRunnerBearer } from "./runner-token";

const keyPair = async () => {
  const { privateKey, publicKey } = await generateKeyPair("ES256", { extractable: true });
  return {
    RUNNER_JWT_PRIVATE_KEY: JSON.stringify(await exportJWK(privateKey)),
    RUNNER_JWT_PUBLIC_KEY: JSON.stringify(await exportJWK(publicKey))
  };
};

const id = { userId: "user_1", orgId: "org_1", deviceId: "cloud-abc" };
const baseEnv = { AUTH_MODE: "workos", WORKOS_CLIENT_ID: "client_x" } as Env;

describe("runner tokens", () => {
  it("mints an ES256 JWT that authenticate() reads as a runner", async () => {
    const keys = await keyPair();
    const env = { ...baseEnv, ...keys } as Env;
    const now = Date.now();
    const token = await mintRunnerToken(env, id, now);
    expect(token?.expiresAt).toBe(now + 3600_000);
    expect(await verifyToken(env, token!.accessToken)).toEqual({ ...id, kind: "runner" });
  });

  it("rejects a runner JWT signed by another key, and never falls through to dev mode", async () => {
    const env = { ...baseEnv, ...(await keyPair()) } as Env;
    const other = { ...baseEnv, ...(await keyPair()), AUTH_MODE: "dev" } as Env;
    const forged = await mintRunnerToken(other, id);
    expect(await verifyToken(env, forged!.accessToken)).toBeUndefined();
    expect(await verifyToken({ ...env, AUTH_MODE: "dev" } as Env, forged!.accessToken)).toBeUndefined();
  });

  it("refuses to mint in production without a key, mints the dev bearer in dev mode", async () => {
    expect(await mintRunnerToken(baseEnv, id)).toBeUndefined();
    const dev = await mintRunnerToken({ AUTH_MODE: "dev" }, id);
    expect(dev?.accessToken).toBe("runner:user_1@org_1:cloud-abc");
    expect(parseDevRunnerBearer(devRunnerBearer(id))).toEqual(id);
  });

  it("accepts the dev runner bearer only in dev mode", async () => {
    const devEnv = { ...baseEnv, AUTH_MODE: "dev" } as Env;
    expect(await verifyToken(devEnv, "runner:user_1@org_1:cloud-abc")).toEqual({ ...id, kind: "runner" });
    expect(await verifyToken(devEnv, "runner:broken")).toBeUndefined();
    expect(await verifyToken(devEnv, "alice@org_1")).toEqual({ userId: "alice", orgId: "org_1", kind: "user" });
    expect(await verifyToken(baseEnv, "runner:user_1@org_1:cloud-abc")).toBeUndefined();
  });
});

describe("runner signatures", () => {
  it("verifies the Ed25519 signature over the documented message", async () => {
    const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const publicKey = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
    const message = runnerTokenMessage("org_1", "user_1", "cloud-abc", 1_800_000_000_000);
    expect(message).toBe("zeron-runner-token\norg_1\nuser_1\ncloud-abc\n1800000000000");
    const sig = b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, new TextEncoder().encode(message))));
    expect(await verifyEd25519(publicKey, message, sig)).toBe(true);
    expect(await verifyEd25519(publicKey, `${message}1`, sig)).toBe(false);
    expect(await verifyEd25519(publicKey, message, "not-a-sig")).toBe(false);
    expect(await verifyEd25519("short", message, sig)).toBe(false);
  });
});
