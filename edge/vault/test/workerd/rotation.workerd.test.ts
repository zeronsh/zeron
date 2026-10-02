import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { VaultProviderId } from "../../src/api";
import { envelopeKid, loadKeyring, open, recordAad, sealJson } from "../../src/crypto";
import { toBase64 } from "../../src/encoding";
import { accountStub } from "../../src/names";
import { setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const KEY = "sk-ant-api03-rotation-test-key-7777";
const PROVIDER: VaultProviderId = "anthropic-key";

/** The configured keys, as the DOs see them. */
const currentOnly = () => loadKeyring({ VAULT_KEK: env.VAULT_KEK });
const previousOnly = () => loadKeyring({ VAULT_KEK: env.VAULT_KEK_PREVIOUS });
const retiredKey = () => loadKeyring({ VAULT_KEK: toBase64(crypto.getRandomValues(new Uint8Array(32))) });

const withKey = async () => {
  const ctx = await setup();
  value(await vault().putCredential(ctx.caller, PROVIDER, { material: { key: KEY }, authorizedDevices: [ctx.device.deviceId] }));
  return ctx;
};

const record = (userId: string) =>
  runInDurableObject(accountStub(env, userId, PROVIDER), (_instance, state) =>
    state.storage.sql.exec<{ generation: number; envelope: string }>("SELECT generation, envelope FROM record").toArray()[0]!
  );

const replaceEnvelope = (userId: string, envelope: string) =>
  runInDurableObject(accountStub(env, userId, PROVIDER), (_instance, state) => {
    state.storage.sql.exec("UPDATE record SET envelope = ? WHERE id = 1", envelope);
  });

describe("KEK rotation", () => {
  it("seals new records under VAULT_KEK", async () => {
    const { userId } = await withKey();
    expect(envelopeKid((await record(userId)).envelope)).toBe((await currentOnly()).current.kid);
  });

  it("opens a record sealed under VAULT_KEK_PREVIOUS and re-wraps it under VAULT_KEK before returning", async () => {
    const { userId, caller, device } = await withKey();
    const aad = recordAad(userId, PROVIDER, 1);
    const previous = await previousOnly();
    await replaceEnvelope(userId, await sealJson(previous, { key: KEY }, aad));
    expect(envelopeKid((await record(userId)).envelope)).toBe(previous.current.kid);

    const logs: string[] = [];
    vi.spyOn(console, "log").mockImplementation((line: unknown) => void logs.push(String(line)));
    const grant = value(await vault().grant(caller, await signedGrant(device, userId, PROVIDER)));
    expect(grant).toMatchObject({ accessToken: KEY, generation: 1 });

    // Persisted under the current key, same generation; it now opens with
    // VAULT_KEK alone, i.e. after VAULT_KEK_PREVIOUS is removed.
    const after = await record(userId);
    expect(after.generation).toBe(1);
    const current = await currentOnly();
    expect(envelopeKid(after.envelope)).toBe(current.current.kid);
    expect(await open(current, after.envelope, aad)).toMatchObject({ plaintext: JSON.stringify({ key: KEY }), rewrap: false });
    expect(logs.map((line) => JSON.parse(line))).toContainEqual({
      event: "vault.rewrap",
      provider: PROVIDER,
      from: previous.current.kid,
      to: current.current.kid
    });
  });

  it("a record sealed under a key no longer configured is unavailable, naming the kid", async () => {
    const { userId, caller, device } = await withKey();
    const retired = await retiredKey();
    await replaceEnvelope(userId, await sealJson(retired, { key: KEY }, recordAad(userId, PROVIDER, 1)));

    const result = await vault().grant(caller, await signedGrant(device, userId, PROVIDER));
    expect(result).toMatchObject({ ok: false, error: "unavailable", status: 503 });
    expect(!result.ok && result.message).toBe(`record sealed under key ${retired.current.kid}, which is no longer configured`);
    // Status reads no ciphertext, so it still answers; a fresh upload replaces the stranded record.
    expect(value(await vault().status(caller)).connections.find((c) => c.provider === PROVIDER)).toBeDefined();
    value(await vault().putCredential(caller, PROVIDER, { material: { key: KEY }, authorizedDevices: [device.deviceId] }));
    expect(value(await vault().grant(caller, await signedGrant(device, userId, PROVIDER))).generation).toBe(2);
  });

  it("AAD still binds a record to its user: a copied envelope does not open", async () => {
    const alice = await withKey();
    const bob = await withKey();
    await replaceEnvelope(bob.userId, (await record(alice.userId)).envelope);
    expect(await vault().grant(bob.caller, await signedGrant(bob.device, bob.userId, PROVIDER))).toMatchObject({
      ok: false,
      error: "unavailable",
      status: 500
    });
    // Alice's own record is untouched.
    value(await vault().grant(alice.caller, await signedGrant(alice.device, alice.userId, PROVIDER)));
  });
});
