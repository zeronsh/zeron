import { describe, expect, it } from "vitest";
import { exportPKCS8, generateKeyPair, jwtVerify } from "jose";
import { isDeadToken, providerToken } from "./apns";

describe("APNs provider token", () => {
  it("is an ES256 JWT with the key id and team, reused within its window", async () => {
    const { privateKey, publicKey } = await generateKeyPair("ES256", { extractable: true });
    const cfg = { keyP8: await exportPKCS8(privateKey), keyId: "ABC123DEFG", teamId: "5XY3M483YQ", topic: "sh.zeron.ios" };
    const now = Date.now();
    const jwt = await providerToken(cfg, now);
    const { payload, protectedHeader } = await jwtVerify(jwt, publicKey);
    expect(protectedHeader).toMatchObject({ alg: "ES256", kid: "ABC123DEFG" });
    expect(payload.iss).toBe("5XY3M483YQ");
    expect(await providerToken(cfg, now + 10 * 60 * 1000)).toBe(jwt);
    expect(await providerToken(cfg, now + 55 * 60 * 1000)).not.toBe(jwt);
  });

  it("drops tokens APNs has retired", () => {
    expect(isDeadToken({ status: 410, reason: "Unregistered" })).toBe(true);
    expect(isDeadToken({ status: 400, reason: "BadDeviceToken" })).toBe(true);
    expect(isDeadToken({ status: 429, reason: "TooManyRequests" })).toBe(false);
  });
});
