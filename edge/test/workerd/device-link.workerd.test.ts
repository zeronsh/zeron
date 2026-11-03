/**
 * Which device a relayed request comes from (device-link.ts, R14): the edge
 * stamps `fromDevice` on a client's frames only when the dial proved its
 * device with its registered plugin key (or the bearer is a runner's), and
 * never relays a `fromDevice` a client wrote itself.
 */
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import type { Verified } from "../../src/auth";
import { b64urlEncode } from "../../src/cloud/crypto";
import { callerDevice, deviceLinkMessage, MAX_SKEW_MS } from "../../src/device-link";
import { decodeDeviceFrame, encodeDeviceFrame, type DeviceFrameHeader } from "../../src/device-room";
import type { Env } from "../../src/env";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import { MAX_LIVE_STEP_UPS, STEP_UP_TTL_MS, newStepUpToken, stepUpHash } from "../../src/step-up";

const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));
const rand = () => crypto.randomUUID().replaceAll("-", "").slice(0, 12);

interface Signer {
  readonly key: string;
  sign(message: string): Promise<string>;
}
const newSigner = async (): Promise<Signer> => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  return {
    key,
    sign: async (m) => b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, new TextEncoder().encode(m))))
  };
};

/** `signer`'s key registered for `userId` from `deviceId` (with a ticket). */
const registered = async (userId: string, deviceId: string): Promise<Signer> => {
  const signer = await newSigner();
  const token = newStepUpToken();
  await directory().issueStepUp(userId, (await stepUpHash(token))!, STEP_UP_TTL_MS, MAX_LIVE_STEP_UPS);
  const res = await SELF.fetch("https://edge.test/plugin-keys", {
    method: "POST",
    headers: { authorization: `Bearer ${userId}`, "content-type": "application/json" },
    body: JSON.stringify({ key: signer.key, deviceId, proof: await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key), stepUp: token })
  });
  expect(res.status).toBe(201);
  return signer;
};

const dial = (bearer: string, target: string, query: string) =>
  SELF.fetch(`https://edge.test/device/${target}/ws?${query}`, { headers: { authorization: `Bearer ${bearer}`, upgrade: "websocket" } });

/** The proof query a device makes for dialing `target` as `connId`. */
const proofQuery = async (signer: Signer, device: string, target: string, connId: string, ts = Date.now(), signedTarget = target) =>
  `callerDevice=${device}&callerKey=${signer.key}&callerTs=${ts}&callerProof=${await signer.sign(deviceLinkMessage(signedTarget, connId, ts))}`;

/** A host for `target` that records the headers of what reaches it. */
const host = async (owner: string, target: string) => {
  const res = await dial(owner, target, "role=host");
  expect(res.status).toBe(101);
  const ws = res.webSocket!;
  ws.accept();
  ws.binaryType = "arraybuffer";
  const seen: DeviceFrameHeader[] = [];
  ws.addEventListener("message", (e) => seen.push(decodeDeviceFrame(new Uint8Array(e.data as ArrayBuffer)).header));
  return { ws, seen };
};

/** Dial as a client, send one frame whose own header claims `forged`, and
 * answer the header the host got. */
const relayed = async (bearer: string, target: string, query: string, h: Awaited<ReturnType<typeof host>>, connId: string) => {
  const res = await dial(bearer, target, `role=client&connId=${connId}&${query}`);
  expect(res.status).toBe(101);
  const ws = res.webSocket!;
  ws.accept();
  ws.send(encodeDeviceFrame({ s: `s-${connId}`, k: "rpc", from: "someone-else", fromDevice: "forged" } as DeviceFrameHeader, new Uint8Array([1])));
  await expect.poll(() => h.seen.find((f) => f.s === `s-${connId}`)).toBeDefined();
  ws.close();
  return { header: h.seen.find((f) => f.s === `s-${connId}`)!, stamped: res.headers.get("x-zeron-caller-device") };
};

describe("relay device stamp", () => {
  it("stamps a dial that proves its device, and never relays a device a client wrote", async () => {
    const owner = `user_${rand()}`;
    const target = `desk-${rand()}`;
    const laptop = await registered(owner, "laptop-1");
    const h = await host(owner, target);

    const good = await relayed(owner, target, await proofQuery(laptop, "laptop-1", target, "c-good"), h, "c-good");
    expect(good.stamped).toBe("laptop-1");
    expect(good.header).toEqual({ s: "s-c-good", k: "rpc", from: "c-good", fromDevice: "laptop-1" });

    // No proof: connected, but nothing stamped, and the forged field dropped.
    const bare = await relayed(owner, target, "callerDevice=laptop-1", h, "c-bare");
    expect(bare.stamped).toBeNull();
    expect(bare.header).toEqual({ s: "s-c-bare", k: "rpc", from: "c-bare" });
    h.ws.close();
  });

  it("leaves a dial unstamped when its proof doesn't hold", async () => {
    const owner = `user_${rand()}`;
    const target = `desk-${rand()}`;
    const laptop = await registered(owner, "laptop-1");
    const stranger = `user_${rand()}`;
    const theirs = await registered(stranger, "laptop-1");
    const revoked = await registered(owner, "old-laptop");
    await SELF.fetch(`https://edge.test/plugin-keys/${revoked.key}`, { method: "DELETE", headers: { authorization: `Bearer ${owner}` } });
    const h = await host(owner, target);
    const cases: [string, string, string][] = [
      ["another device's id", "c1", await proofQuery(laptop, "phone-2", target, "c1")],
      ["signed for another room", "c2", await proofQuery(laptop, "laptop-1", target, "c2", Date.now(), "elsewhere")],
      ["signed for another conn", "c3", await proofQuery(laptop, "laptop-1", target, "other-conn")],
      ["stale", "c4", await proofQuery(laptop, "laptop-1", target, "c4", Date.now() - MAX_SKEW_MS - 60_000)],
      ["someone else's key", "c5", await proofQuery(theirs, "laptop-1", target, "c5")],
      ["a revoked key", "c6", await proofQuery(revoked, "old-laptop", target, "c6")],
      ["a bad signature", "c7", (await proofQuery(laptop, "laptop-1", target, "c7")).replace(/callerProof=[A-Za-z0-9_-]+/, `callerProof=${"A".repeat(86)}`)]
    ];
    for (const [label, connId, query] of cases) {
      const { header, stamped } = await relayed(owner, target, query, h, connId);
      expect(stamped, label).toBeNull();
      expect(header.fromDevice, label).toBeUndefined();
    }
    h.ws.close();
  });

  it("can't reach another user's device room, proof or not", async () => {
    const owner = `user_${rand()}`;
    const target = `desk-${rand()}`;
    const h = await host(owner, target);
    const intruder = `user_${rand()}`;
    const theirKey = await registered(intruder, "laptop-x");
    expect((await dial(intruder, target, `role=client&connId=cx&${await proofQuery(theirKey, "laptop-x", target, "cx")}`)).status).toBe(403);
    h.ws.close();
  });

  it("takes a runner's device from its verified token", async () => {
    const url = new URL("https://edge.test/device/d/ws?callerDevice=forged");
    const runner: Verified = { userId: "user_r", kind: "runner", deviceId: "cloud-7" };
    expect(await callerDevice(env as unknown as Env, runner, url, "d", "c1")).toBe("cloud-7");
    expect(await callerDevice(env as unknown as Env, { userId: "user_r", kind: "user" }, url, "d", "c1")).toBeUndefined();
  });
});
