import { afterEach, describe, expect, it, vi } from "vitest";
import type { VaultCaller } from "../../src/api";
import { makeDevice } from "../support";
import { enroll, runnerCaller, setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const KEY = "sk-ant-api03-parents-test-key-4242";

/** Enroll a Cloud session sandbox under the logical Cloud device `parentId`. */
const enrollChild = async (caller: VaultCaller, parentId: string) => {
  const child = await makeDevice(`session-${crypto.randomUUID()}`);
  value(
    await vault().enrollDevice(caller, { deviceId: child.deviceId, kind: "cloud", publicKey: child.publicKey, parentId })
  );
  return child;
};

/** A user whose anthropic key is connected "for Cloud" (the logical id only). */
const connectedForCloud = async () => {
  const ctx = await setup();
  const cloudId = `cloud-${crypto.randomUUID()}`;
  value(await vault().putCredential(ctx.caller, "anthropic-key", { material: { key: KEY }, authorizedDevices: [cloudId] }));
  return { ...ctx, cloudId };
};

describe("parent devices", () => {
  it("a session device is authorized through its parent", async () => {
    const { userId, caller, device: laptop, cloudId } = await connectedForCloud();
    const child = await enrollChild(caller, cloudId);

    expect(value(await vault().grant(caller, await signedGrant(child, userId, "anthropic-key"))).accessToken).toBe(KEY);
    // The sandbox's own runner token works too.
    const runner = runnerCaller(userId, child.deviceId);
    expect(value(await vault().grant(runner, await signedGrant(child, userId, "anthropic-key"))).accessToken).toBe(KEY);

    // Not authorized: the laptop, and a session under some other parent.
    expect(await vault().grant(caller, await signedGrant(laptop, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "not_authorized"
    });
    const stray = await enrollChild(caller, "cloud-other");
    expect(await vault().grant(caller, await signedGrant(stray, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "not_authorized"
    });

    const status = value(await vault().status(caller));
    expect(status.devices.find((d) => d.deviceId === child.deviceId)).toMatchObject({ kind: "cloud", parentId: cloudId });
    expect(status.devices.find((d) => d.deviceId === laptop.deviceId)?.parentId).toBeUndefined();
  });

  it("revoking a parent that never enrolled a key refuses all its children, present and future", async () => {
    const { userId, caller, cloudId } = await connectedForCloud();
    const first = await enrollChild(caller, cloudId);
    const second = await enrollChild(caller, cloudId);
    value(await vault().grant(caller, await signedGrant(first, userId, "anthropic-key")));

    const status = value(await vault().revokeDevice(caller, cloudId));
    expect(status.devices.find((d) => d.deviceId === cloudId)?.revokedAt).toBeTypeOf("number");
    for (const child of [first, second]) {
      expect(status.devices.find((d) => d.deviceId === child.deviceId)?.revokedAt).toBeTypeOf("number");
      expect(await vault().grant(caller, await signedGrant(child, userId, "anthropic-key"))).toMatchObject({
        ok: false,
        error: "device_revoked"
      });
    }
    // A new session under the revoked parent cannot enroll.
    const late = await makeDevice("session-late");
    expect(
      await vault().enrollDevice(caller, { deviceId: late.deviceId, kind: "cloud", publicKey: late.publicKey, parentId: cloudId })
    ).toMatchObject({ ok: false, error: "device_revoked" });
  });

  it("revoking an enrolled parent refuses its children; revoking a child affects only that child", async () => {
    const ctx = await setup();
    const { userId, caller } = ctx;
    const parent = await enroll(caller, "cloud");
    value(await vault().putCredential(caller, "anthropic-key", { material: { key: KEY }, authorizedDevices: [parent.deviceId] }));
    const keep = await enrollChild(caller, parent.deviceId);
    const drop = await enrollChild(caller, parent.deviceId);

    value(await vault().revokeDevice(caller, drop.deviceId));
    expect(await vault().grant(caller, await signedGrant(drop, userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "device_revoked"
    });
    value(await vault().grant(caller, await signedGrant(parent, userId, "anthropic-key")));
    value(await vault().grant(caller, await signedGrant(keep, userId, "anthropic-key")));

    value(await vault().revokeDevice(caller, parent.deviceId));
    for (const device of [parent, keep]) {
      expect(await vault().grant(caller, await signedGrant(device, userId, "anthropic-key"))).toMatchObject({
        ok: false,
        error: "device_revoked"
      });
    }
  });

  it("parents are one level deep", async () => {
    const { caller, cloudId } = await connectedForCloud();
    const child = await enrollChild(caller, cloudId);
    const grandchild = await makeDevice("session-nested");
    expect(
      await vault().enrollDevice(caller, {
        deviceId: grandchild.deviceId,
        kind: "cloud",
        publicKey: grandchild.publicKey,
        parentId: child.deviceId
      })
    ).toMatchObject({ ok: false, error: "bad_request" });
    expect(
      await vault().enrollDevice(caller, { deviceId: grandchild.deviceId, kind: "cloud", publicKey: grandchild.publicKey, parentId: grandchild.deviceId })
    ).toMatchObject({ ok: false, error: "bad_request" });
    expect(
      await vault().enrollDevice(caller, { deviceId: grandchild.deviceId, kind: "cloud", publicKey: grandchild.publicKey, parentId: "bad\nid" })
    ).toMatchObject({ ok: false, error: "bad_request" });
  });
});
