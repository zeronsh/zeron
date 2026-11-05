import { exports } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import { makeDevice } from "../support";
import { enroll, nextTs, runnerCaller, setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const KEY = "sk-ant-api03-grant-test-key-0001";

/** A user with one laptop authorized for an anthropic key. */
const withKey = async () => {
  const ctx = await setup();
  value(
    await vault().putCredential(ctx.caller, "anthropic-key", {
      material: { key: KEY },
      authorizedDevices: [ctx.device.deviceId]
    })
  );
  return ctx;
};

describe("grants", () => {
  it("enrolled device with a valid signature gets a grant", async () => {
    const { userId, caller, device } = await withKey();
    const before = Date.now();
    const grant = value(await vault().grant(caller, await signedGrant(device, userId, "anthropic-key")));
    expect(grant).toMatchObject({ provider: "anthropic-key", accessToken: KEY, generation: 1, account: "…0001" });
    expect(grant.expiresAt).toBeGreaterThanOrEqual(before + 60 * 60_000);
  });

  it("works end to end through the VaultApi entrypoint (real RPC)", async () => {
    const { userId, caller, device } = await withKey();
    const grant = await exports.VaultApi.grant(caller, await signedGrant(device, userId, "anthropic-key"));
    expect(grant.ok && grant.value.accessToken).toBe(KEY);
  });

  it("rejects a bad signature", async () => {
    const { userId, caller, device } = await withKey();
    const imposter = await makeDevice(device.deviceId);
    const ts = nextTs();
    const result = await vault().grant(caller, {
      provider: "anthropic-key",
      deviceId: device.deviceId,
      ts,
      sig: await imposter.grantSig(userId, "anthropic-key", ts)
    });
    expect(result).toMatchObject({ ok: false, error: "bad_signature", status: 401 });
    // A signature for another provider (or user) does not transfer either.
    const other = await signedGrant(device, userId, "openai-key");
    const swapped = await vault().grant(caller, { ...other, provider: "anthropic-key" });
    expect(swapped).toMatchObject({ ok: false, error: "bad_signature" });
    const garbage = await vault().grant(caller, { ...other, provider: "anthropic-key", sig: "!!!" });
    expect(garbage).toMatchObject({ ok: false, error: "bad_signature" });
  });

  it("rejects a ts outside the ±120 s window", async () => {
    const { userId, caller, device } = await withKey();
    const old = await signedGrant(device, userId, "anthropic-key", Date.now() - 121_000);
    expect(await vault().grant(caller, old)).toMatchObject({ ok: false, error: "stale", status: 401 });
    const future = await signedGrant(device, userId, "anthropic-key", Date.now() + 10 * 60_000);
    expect(await vault().grant(caller, future)).toMatchObject({ ok: false, error: "stale" });
  });

  it("rejects a replayed ts (and any ts at or below the fence)", async () => {
    const { userId, caller, device } = await withKey();
    const request = await signedGrant(device, userId, "anthropic-key");
    value(await vault().grant(caller, request));
    expect(await vault().grant(caller, request)).toMatchObject({ ok: false, error: "stale" });
    const older = await signedGrant(device, userId, "anthropic-key", request.ts - 1);
    expect(await vault().grant(caller, older)).toMatchObject({ ok: false, error: "stale" });
    value(await vault().grant(caller, await signedGrant(device, userId, "anthropic-key")));
  });

  it("rejects unknown and revoked devices; a revoked key stays revoked", async () => {
    const { userId, caller, device } = await withKey();
    const stranger = await makeDevice("never-enrolled");
    expect(await vault().grant(caller, await signedGrant(stranger, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "device_unknown"
    });

    const status = value(await vault().revokeDevice(caller, device.deviceId));
    expect(status.devices.find((d) => d.deviceId === device.deviceId)?.revokedAt).toBeTypeOf("number");
    expect(await vault().grant(caller, await signedGrant(device, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "device_revoked",
      status: 403
    });
    // An idempotent re-enroll with the SAME key must not undo the revoke…
    expect(
      await vault().enrollDevice(caller, { deviceId: device.deviceId, kind: "laptop", publicKey: device.publicKey })
    ).toMatchObject({ ok: false, error: "device_revoked" });
    // …nor smuggle the revoked key back in under another id…
    expect(
      await vault().enrollDevice(caller, { deviceId: "fresh-id", kind: "laptop", publicKey: device.publicKey })
    ).toMatchObject({ ok: false, error: "device_revoked" });
    // …but a freshly generated key may take the id over again.
    const rotated = await makeDevice(device.deviceId);
    value(await vault().enrollDevice(caller, { deviceId: device.deviceId, kind: "laptop", publicKey: rotated.publicKey }));
    value(await vault().grant(caller, await signedGrant(rotated, userId, "anthropic-key")));
    expect(await vault().revokeDevice(caller, "no-such-device")).toMatchObject({ ok: false, error: "not_found" });
  });

  it("rejects an enrolled device that is not authorized for the credential", async () => {
    const { userId, caller } = await withKey();
    const other = await enroll(caller);
    expect(await vault().grant(caller, await signedGrant(other, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "not_authorized",
      status: 403
    });
    value(await vault().authorize(caller, "anthropic-key", [other.deviceId]));
    value(await vault().grant(caller, await signedGrant(other, userId, "anthropic-key")));
  });

  it("answers not_found for a provider with no credential", async () => {
    const { userId, caller, device } = await setup();
    expect(await vault().grant(caller, await signedGrant(device, userId, "codex"))).toMatchObject({
      ok: false,
      error: "not_found",
      status: 404
    });
  });

  it("rejects malformed enrollments and uploads", async () => {
    const { caller, device } = await setup();
    expect(
      await vault().enrollDevice(caller, { deviceId: "x", kind: "laptop", publicKey: "AAAA" })
    ).toMatchObject({ ok: false, error: "bad_request" });
    expect(
      await vault().enrollDevice(caller, { deviceId: "bad\nid", kind: "laptop", publicKey: device.publicKey })
    ).toMatchObject({ ok: false, error: "bad_request" });
    expect(
      await vault().putCredential(caller, "github", { material: { key: "ghp_whatever_123" }, authorizedDevices: [] })
    ).toMatchObject({ ok: false, error: "bad_request" });
    expect(
      await vault().putCredential(caller, "codex", { material: { key: "sk-not-codex" }, authorizedDevices: [] })
    ).toMatchObject({ ok: false, error: "bad_request" });
  });
});

describe("runner callers", () => {
  it("may not call admin methods", async () => {
    const { userId, caller } = await setup();
    const cloud = await enroll(caller, "cloud");
    const runner = runnerCaller(userId, cloud.deviceId);
    const refusals = await Promise.all([
      vault().status(runner),
      vault().putCredential(runner, "anthropic-key", { material: { key: KEY }, authorizedDevices: [cloud.deviceId] }),
      vault().authorize(runner, "anthropic-key", [cloud.deviceId]),
      vault().disconnect(runner, "anthropic-key"),
      vault().enrollDevice(runner, { deviceId: "cloud-2", kind: "cloud", publicKey: cloud.publicKey }),
      vault().revokeDevice(runner, cloud.deviceId),
      vault().setDisabled(runner, false),
      vault().githubDeviceStart(runner, [cloud.deviceId]),
      vault().githubDevicePoll(runner, "A".repeat(22))
    ]);
    for (const result of refusals) expect(result).toMatchObject({ ok: false, error: "forbidden", status: 403 });
  });

  it("may grant only for their own device", async () => {
    const { userId, caller, device: laptop } = await withKey();
    const cloud = await enroll(caller, "cloud");
    value(await vault().authorize(caller, "anthropic-key", [laptop.deviceId, cloud.deviceId]));
    const runner = runnerCaller(userId, cloud.deviceId);

    // Even with the laptop's valid signature, a runner cannot draw its grant.
    const forLaptop = await signedGrant(laptop, userId, "anthropic-key");
    expect(await vault().grant(runner, forLaptop)).toMatchObject({ ok: false, error: "forbidden", status: 403 });

    const own = value(await vault().grant(runner, await signedGrant(cloud, userId, "anthropic-key")));
    expect(own.accessToken).toBe(KEY);
  });

  it("must carry a device id", async () => {
    const { userId, device } = await withKey();
    const result = await vault().grant(
      { userId, orgId: "org_test", kind: "runner" },
      await signedGrant(device, userId, "anthropic-key")
    );
    expect(result).toMatchObject({ ok: false, error: "bad_request" });
  });
});

describe("kill switches", () => {
  it("global VAULT_DISABLED=1 refuses everything with 503", async () => {
    const { userId, caller, device } = await withKey();
    const disabled = vault({ VAULT_DISABLED: "1" });
    expect(await disabled.status(caller)).toMatchObject({ ok: false, error: "disabled", status: 503 });
    expect(await disabled.grant(caller, await signedGrant(device, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "disabled",
      status: 503
    });
    expect(await disabled.setDisabled(caller, false)).toMatchObject({ ok: false, error: "disabled", status: 503 });
  });

  it("per-user disable refuses grants and reads until switched back", async () => {
    const { userId, caller, device } = await withKey();
    value(await vault().setDisabled(caller, true));
    expect(await vault().grant(caller, await signedGrant(device, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "disabled",
      status: 403
    });
    expect(await vault().status(caller)).toMatchObject({ ok: false, error: "disabled", status: 403 });
    expect(
      await vault().putCredential(caller, "openai-key", { material: { key: "sk-proj-abcdefgh" }, authorizedDevices: [] })
    ).toMatchObject({ ok: false, error: "disabled" });
    expect(await vault().githubDeviceStart(caller, [])).toMatchObject({ ok: false, error: "disabled" });

    // Access-reducing calls stay available while disabled.
    const other = await makeDevice("other-laptop");
    expect(await vault().enrollDevice(caller, { deviceId: other.deviceId, kind: "laptop", publicKey: other.publicKey })).toMatchObject({
      ok: false,
      error: "disabled"
    });
    value(await vault().disconnect(caller, "openai-key"));

    // Another user is unaffected.
    const stranger = await withKey();
    value(await vault().grant(stranger.caller, await signedGrant(stranger.device, stranger.userId, "anthropic-key")));

    value(await vault().setDisabled(caller, false));
    value(await vault().grant(caller, await signedGrant(device, userId, "anthropic-key")));
  });
});
