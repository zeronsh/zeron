import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { b64urlEncode } from "../../src/cloud/crypto";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import { MAX_LIVE_STEP_UPS, STEP_UP_TTL_MS, newStepUpToken, stepUpHash } from "../../src/step-up";
import { PLUGIN_KEYS_PER_DAY } from "../../src/user-directory";

const newUserId = () => `user_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;
const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));

/** A WorkOS user in the directory with `username`, as after sign-up and a claim. */
const member = async (username: string): Promise<string> => {
  const userId = newUserId();
  const stamp = "2026-10-31T10:00:00.000Z";
  await directory().upsert({ id: userId, email: `${userId}@example.net`, first_name: "Ada", last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp });
  const res = await SELF.fetch("https://edge.test/username", {
    method: "POST",
    headers: { authorization: `Bearer ${userId}`, "content-type": "application/json" },
    body: JSON.stringify({ username })
  });
  expect(res.status).toBe(200);
  return userId;
};

interface Signer {
  readonly key: string;
  sign(message: string): Promise<string>;
}

const newSigner = async (): Promise<Signer> => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  return {
    key,
    sign: async (message) =>
      b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, new TextEncoder().encode(message))))
  };
};

const call = (bearer: string | undefined, method: string, path: string, body?: unknown) =>
  SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: {
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body)
  });

/** A fresh sign-in's step-up ticket (step-up.ts), as `/auth/exchange` mints. */
const stepUp = async (userId: string): Promise<string> => {
  const token = newStepUpToken();
  await directory().issueStepUp(userId, (await stepUpHash(token))!, STEP_UP_TTL_MS, MAX_LIVE_STEP_UPS);
  return token;
};

/** Register `signer`'s key for `userId`, with a ticket just minted for them
 * (edge-security.workerd.test.ts covers registering without one). Runner
 * bearers get none: they never register. */
const register = async (userId: string, signer: Signer, deviceId?: string) =>
  call(userId, "POST", "/plugin-keys", {
    key: signer.key,
    ...(deviceId ? { deviceId } : {}),
    proof: await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key),
    ...(userId.startsWith("runner:") ? {} : { stepUp: await stepUp(userId) })
  });

const lookup = (userId: string, key: string) => call(userId, "GET", `/plugin-keys/${key}`);

describe("/plugin-keys", () => {
  it("registers a key and tells its owner and others who it belongs to", async () => {
    const owner = await member("keyowner");
    const signer = await newSigner();
    const res = await register(owner, signer, "device-1");
    expect(res.status).toBe(201);
    expect(await res.json()).toEqual({ key: signer.key, registered: true });

    const mine = await lookup(owner, signer.key);
    expect(mine.status).toBe(200);
    expect(await mine.json()).toMatchObject({ key: signer.key, username: "keyowner", mine: true, revoked: false, verified: true });

    const other = newUserId();
    const theirs = await lookup(other, signer.key);
    expect(theirs.status).toBe(200);
    const text = await theirs.text();
    expect(JSON.parse(text)).toMatchObject({ key: signer.key, username: "keyowner", mine: false, revoked: false, verified: true });
    expect(text).not.toContain(owner);

    // The same registration again is a no-op that succeeds.
    expect((await register(owner, signer, "device-1")).status).toBe(201);
  });

  it("answers a null username, unverified, to an owner without one; unknown to others", async () => {
    const owner = newUserId();
    const signer = await newSigner();
    expect((await register(owner, signer)).status).toBe(201);
    expect(await (await lookup(owner, signer.key)).json()).toMatchObject({ key: signer.key, username: null, mine: true, verified: false });
    expect((await lookup(newUserId(), signer.key)).status).toBe(404);
  });

  it("answers 400 for a bad key, device or proof, registering nothing", async () => {
    const userId = newUserId();
    const signer = await newSigner();
    const other = await newSigner();
    const proof = await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key);
    const cases: [unknown, string][] = [
      // Signed by another key, or over another message.
      [{ key: signer.key, proof: await other.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key) }, "invalid_proof"],
      [{ key: signer.key, proof: await signer.sign(signer.key) }, "invalid_proof"],
      [{ key: signer.key, proof: proof.slice(0, -4) }, "invalid_proof"],
      [{ key: signer.key }, "invalid_proof"],
      [{ key: signer.key.slice(0, -2), proof }, "invalid_key"],
      [{ key: `${signer.key}=`, proof }, "invalid_key"],
      [{ key: 42, proof }, "invalid_key"],
      [{ proof }, "invalid_key"],
      [{ key: signer.key, proof, deviceId: "has space" }, "invalid_device"],
      [{ key: signer.key, proof, deviceId: 7 }, "invalid_device"],
      ["{not json", "invalid_key"]
    ];
    for (const [body, error] of cases) {
      const res = await call(userId, "POST", "/plugin-keys", body);
      expect(res.status, JSON.stringify(body)).toBe(400);
      expect(((await res.json()) as { error: string }).error).toBe(error);
    }
    expect((await lookup(userId, signer.key)).status).toBe(404);
    expect(((await (await call(userId, "GET", "/plugin-keys")).json()) as { keys: unknown[] }).keys).toEqual([]);
  });

  it("answers 409 key_taken for someone else's key", async () => {
    const owner = newUserId();
    const signer = await newSigner();
    expect((await register(owner, signer)).status).toBe(201);
    const thief = newUserId();
    const res = await register(thief, signer);
    expect(res.status).toBe(409);
    expect(((await res.json()) as { error: string }).error).toBe("key_taken");
    expect(((await (await lookup(owner, signer.key)).json()) as { mine: boolean }).mine).toBe(true);
    expect(((await (await call(thief, "GET", "/plugin-keys")).json()) as { keys: unknown[] }).keys).toEqual([]);
  });

  it("lists the caller's keys, oldest first", async () => {
    const userId = newUserId();
    const first = await newSigner();
    const second = await newSigner();
    expect((await register(userId, first, "laptop")).status).toBe(201);
    await new Promise((r) => setTimeout(r, 5));
    expect((await register(userId, second)).status).toBe(201);
    // Another user's key isn't listed.
    expect((await register(newUserId(), await newSigner())).status).toBe(201);

    const { keys } = (await (await call(userId, "GET", "/plugin-keys")).json()) as {
      keys: { key: string; deviceId: string | null; createdAt: number; revokedAt: number | null }[];
    };
    expect(keys.map(({ key, deviceId, revokedAt }) => ({ key, deviceId, revokedAt }))).toEqual([
      { key: first.key, deviceId: "laptop", revokedAt: null },
      { key: second.key, deviceId: null, revokedAt: null }
    ]);
    for (const { createdAt } of keys) expect(Math.abs(Date.now() - createdAt)).toBeLessThan(60_000);
  });

  it("revokes the caller's key, and only theirs", async () => {
    const owner = newUserId();
    const signer = await newSigner();
    expect((await register(owner, signer)).status).toBe(201);

    // Not someone else's.
    const other = newUserId();
    const refused = await call(other, "DELETE", `/plugin-keys/${signer.key}`);
    expect(refused.status).toBe(404);
    expect(((await refused.json()) as { error: string }).error).toBe("unknown_key");
    expect(((await (await lookup(owner, signer.key)).json()) as { revoked: boolean }).revoked).toBe(false);

    const res = await call(owner, "DELETE", `/plugin-keys/${signer.key}`);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ revoked: true });
    expect(await (await lookup(other, signer.key)).json()).toMatchObject({ key: signer.key, username: null, mine: false, revoked: true, verified: false });
    expect(((await (await lookup(owner, signer.key)).json()) as { revoked: boolean }).revoked).toBe(true);

    // Revoking again keeps the first time; the list shows it.
    const listed = async () =>
      ((await (await call(owner, "GET", "/plugin-keys")).json()) as { keys: { revokedAt: number | null }[] }).keys[0]!.revokedAt;
    const revokedAt = await listed();
    expect(revokedAt).toEqual(expect.any(Number));
    await new Promise((r) => setTimeout(r, 5));
    expect((await call(owner, "DELETE", `/plugin-keys/${signer.key}`)).status).toBe(200);
    expect(await listed()).toBe(revokedAt);

    // A revoked key can't be registered again, by anyone.
    const again = await register(owner, signer);
    expect(again.status).toBe(409);
    expect(((await again.json()) as { error: string }).error).toBe("key_revoked");
    expect((await register(other, signer)).status).toBe(409);
  });

  it("answers 404 unknown_key for a key no one registered, or a malformed one", async () => {
    const userId = newUserId();
    for (const key of [(await newSigner()).key, "AAAA", "not-a-key"]) {
      const res = await lookup(userId, key);
      expect(res.status).toBe(404);
      expect(((await res.json()) as { error: string }).error).toBe("unknown_key");
      expect((await call(userId, "DELETE", `/plugin-keys/${key}`)).status).toBe(404);
    }
  });

  it(`registers at most ${PLUGIN_KEYS_PER_DAY} new keys a user a day`, async () => {
    const userId = newUserId();
    const signers = await Promise.all(Array.from({ length: PLUGIN_KEYS_PER_DAY + 1 }, newSigner));
    for (const signer of signers.slice(0, PLUGIN_KEYS_PER_DAY)) expect((await register(userId, signer)).status).toBe(201);
    expect((await register(userId, signers.at(-1)!)).status).toBe(429);
    // A key already theirs still answers.
    expect((await register(userId, signers[0]!)).status).toBe(201);
  });

  it("revokes a deleted user's keys and refuses them new ones", async () => {
    const owner = await member("goneowner");
    const signer = await newSigner();
    expect((await register(owner, signer)).status).toBe(201);
    const stamp = "2026-10-31T11:00:00.000Z";
    await directory().remove({ id: owner, email: "", first_name: null, last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp }, stamp);
    expect(await (await lookup(newUserId(), signer.key)).json()).toMatchObject({ key: signer.key, username: null, mine: false, revoked: true });
    expect((await register(owner, await newSigner())).status).toBe(403);
  });

  it("needs a bearer, and lets runner tokens look a key up but nothing else", async () => {
    const userId = newUserId();
    const signer = await newSigner();
    expect((await call(undefined, "GET", "/plugin-keys/AAAA")).status).toBe(401);
    expect((await call(undefined, "GET", "/plugin-keys")).status).toBe(401);
    expect((await call(undefined, "POST", "/plugin-keys", { key: signer.key })).status).toBe(401);
    const runner = `runner:${userId}@org1:cloud-1`;
    expect((await register(runner, signer)).status).toBe(403);
    expect((await call(runner, "GET", "/plugin-keys")).status).toBe(403);
    expect((await call(runner, "DELETE", `/plugin-keys/${signer.key}`)).status).toBe(403);
    expect((await lookup(userId, signer.key)).status).toBe(404);
    // A Cloud machine tells its plugins' authors: one key, looked up as its
    // account (mine), never registered from it.
    const owner = await member("runnerowner");
    const ownersKey = await newSigner();
    expect((await register(owner, ownersKey)).status).toBe(201);
    const ownersRunner = `runner:${owner}@org1:cloud-1`;
    expect(await (await lookup(ownersRunner, ownersKey.key)).json()).toMatchObject({
      key: ownersKey.key,
      username: "runnerowner",
      mine: true,
      revoked: false
    });
    expect((await call(userId, "PUT", "/plugin-keys")).status).toBe(405);
  });
});
