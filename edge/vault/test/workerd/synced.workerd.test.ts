import { env } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import { devicesStub } from "../../src/names";
import { makeDevice } from "../support";
import { codexMaterial, codexTokens, DAY, enroll, setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const KEY_A = "sk-ant-api03-synced-key-aaaa-0001";
const KEY_B = "sk-ant-api03-synced-key-bbbb-0002";

const putKey = (ctx: Awaited<ReturnType<typeof setup>>, key: string, extra: { activate?: boolean; devices?: string[] } = {}) =>
  vault().putCredential(ctx.caller, "anthropic-key", {
    material: { key },
    authorizedDevices: extra.devices ?? [ctx.device.deviceId],
    ...(extra.activate !== undefined ? { activate: extra.activate } : {})
  });

const slots = async (ctx: Awaited<ReturnType<typeof setup>>) => value(await vault().accounts(ctx.caller, "anthropic-key"));

describe("grants by slot", () => {
  it("a slot other than the active one is granted from that account", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    value(await putKey(ctx, KEY_B)); // B is active
    const [a, b] = await slots(ctx);
    expect(b!.active).toBe(true);

    const request = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    const bySlot = value(await vault().grant(ctx.caller, { ...request, slot: a!.slot }));
    expect(bySlot).toMatchObject({ accessToken: KEY_A, slot: a!.slot });
    // Without a slot, still the active account (and the grant says which);
    // naming the active slot agrees.
    const active = value(await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "anthropic-key")));
    expect(active).toMatchObject({ accessToken: KEY_B, slot: b!.slot });
    const named = { ...(await signedGrant(ctx.device, ctx.userId, "anthropic-key")), slot: b!.slot };
    expect(value(await vault().grant(ctx.caller, named)).accessToken).toBe(KEY_B);
  });

  it("works when no account is active", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    const [a] = await slots(ctx);
    value(await vault().activateAccount(ctx.caller, "anthropic-key", null));
    const request = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    expect(await vault().grant(ctx.caller, request)).toMatchObject({ ok: false, error: "not_found" });
    const bySlot = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    expect(value(await vault().grant(ctx.caller, { ...bySlot, slot: a!.slot })).accessToken).toBe(KEY_A);
  });

  it("an unknown slot is not_found", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    const request = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    expect(await vault().grant(ctx.caller, { ...request, slot: "nope" })).toMatchObject({
      ok: false,
      error: "not_found",
      status: 404
    });
  });

  it("a malformed slot is a bad request", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    const request = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    expect(await vault().grant(ctx.caller, { ...request, slot: "bad slot!" })).toMatchObject({ ok: false, error: "bad_request" });
  });

  it("the slot's own authorizedDevices apply, not the active account's", async () => {
    const ctx = await setup();
    const other = await enroll(ctx.caller);
    value(await putKey(ctx, KEY_A, { devices: [other.deviceId] }));
    value(await putKey(ctx, KEY_B)); // active; lists only ctx.device
    const [a] = await slots(ctx);
    const request = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    expect(await vault().grant(ctx.caller, { ...request, slot: a!.slot })).toMatchObject({ ok: false, error: "not_authorized" });
    const ok = await signedGrant(other, ctx.userId, "anthropic-key");
    expect(value(await vault().grant(ctx.caller, { ...ok, slot: a!.slot })).accessToken).toBe(KEY_A);
  });

  it("github grants cannot name a slot", async () => {
    const ctx = await setup();
    const request = await signedGrant(ctx.device, ctx.userId, "github", undefined, "octocat/app");
    expect(await vault().grant(ctx.caller, { ...request, slot: "any" })).toMatchObject({ ok: false, error: "bad_request" });
  });

  it("the slot rides outside the signature and lands in the audit detail", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    const [a] = await slots(ctx);
    // Signed exactly as before (no slot in the message); adding one still verifies.
    const request = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    value(await vault().grant(ctx.caller, { ...request, slot: a!.slot }));
    const audit = value(await devicesStub(env, ctx.userId).auditLog(ctx.userId));
    expect(audit.find((entry) => entry.event === "grant")?.detail).toContain(`slot=${a!.slot}`);
  });
});

describe("authorizedDevices \"*\"", () => {
  it("authorizes an enrolled laptop that isn't listed, and a Cloud child", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A, { devices: ["*"] }));
    const laptop = await enroll(ctx.caller);
    expect(value(await vault().grant(ctx.caller, await signedGrant(laptop, ctx.userId, "anthropic-key"))).accessToken).toBe(KEY_A);

    const child = await makeDevice(`session-${crypto.randomUUID()}`);
    value(
      await vault().enrollDevice(ctx.caller, {
        deviceId: child.deviceId,
        kind: "cloud",
        publicKey: child.publicKey,
        parentId: "cloud-xyz"
      })
    );
    expect(value(await vault().grant(ctx.caller, await signedGrant(child, ctx.userId, "anthropic-key"))).accessToken).toBe(KEY_A);
  });

  it("does not authorize a device that is unknown or revoked", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A, { devices: ["*"] }));
    const stranger = await makeDevice("never-enrolled");
    expect(await vault().grant(ctx.caller, await signedGrant(stranger, ctx.userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "device_unknown"
    });
    const laptop = await enroll(ctx.caller);
    value(await vault().grant(ctx.caller, await signedGrant(laptop, ctx.userId, "anthropic-key")));
    value(await vault().revokeDevice(ctx.caller, laptop.deviceId));
    expect(await vault().grant(ctx.caller, await signedGrant(laptop, ctx.userId, "anthropic-key"))).toMatchObject({
      ok: false,
      error: "device_revoked"
    });
  });

  it("is accepted by authorize and the GitHub device flow, and rejected when malformed", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    value(await vault().authorize(ctx.caller, "anthropic-key", ["*"]));
    const laptop = await enroll(ctx.caller);
    expect(value(await vault().grant(ctx.caller, await signedGrant(laptop, ctx.userId, "anthropic-key"))).accessToken).toBe(KEY_A);
    expect(await vault().authorize(ctx.caller, "anthropic-key", ["bad id!"])).toMatchObject({ ok: false, error: "bad_request" });
    expect(await putKey(ctx, KEY_B, { devices: ["*", "bad id!"] })).toMatchObject({ ok: false, error: "bad_request" });
    expect(await vault().githubDeviceStart(ctx.caller, ["*", "bad id!"])).toMatchObject({ ok: false, error: "bad_request" });
  });
});

describe("non-activating uploads", () => {
  it("activate:false keeps the current active account", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    value(await putKey(ctx, KEY_B, { activate: false }));
    const accounts = await slots(ctx);
    expect(accounts.map((a) => [a.active])).toEqual([[true], [false]]);
    expect(value(await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "anthropic-key"))).accessToken).toBe(KEY_A);
    // The inactive one is still stored and grantable by slot.
    const request = await signedGrant(ctx.device, ctx.userId, "anthropic-key");
    expect(value(await vault().grant(ctx.caller, { ...request, slot: accounts[1]!.slot })).accessToken).toBe(KEY_B);
  });

  it("activate:false activates when the provider has no active account", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A, { activate: false }));
    expect((await slots(ctx)).map((a) => a.active)).toEqual([true]);

    // Also after the user cleared the active account.
    value(await vault().activateAccount(ctx.caller, "anthropic-key", null));
    value(await putKey(ctx, KEY_B, { activate: false }));
    const accounts = await slots(ctx);
    expect(accounts.filter((a) => a.active)).toHaveLength(1);
    expect(accounts.find((a) => a.active)?.slot).toBe(accounts[1]!.slot);
  });

  it("activate:false activates when the active account was forgotten", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    value(await putKey(ctx, KEY_B, { activate: false }));
    const [a, b] = await slots(ctx);
    value(await vault().forgetAccount(ctx.caller, "anthropic-key", a!.slot));
    value(await putKey(ctx, "sk-ant-api03-synced-key-cccc-0003", { activate: false }));
    const accounts = await slots(ctx);
    expect(accounts).toHaveLength(2);
    // Forgetting left none active; the next upload takes the role.
    expect(accounts.find((x) => x.active)?.slot).not.toBe(b!.slot);
  });

  it("re-uploading an existing account with activate:false does not change which is active", async () => {
    const ctx = await setup();
    value(await putKey(ctx, KEY_A));
    value(await putKey(ctx, KEY_B)); // B active
    const before = await slots(ctx);
    value(await putKey(ctx, KEY_A, { activate: false }));
    const after = await slots(ctx);
    expect(after).toHaveLength(2);
    expect(after.find((a) => a.active)?.slot).toBe(before.find((a) => a.active)?.slot);
    // And a plain re-upload still activates it.
    value(await putKey(ctx, KEY_A));
    const last = await slots(ctx);
    expect(last.find((a) => a.active)?.slot).toBe(before.find((a) => !a.active)?.slot);
  });

  it("activate must be a boolean", async () => {
    const ctx = await setup();
    const bad = await vault().putCredential(ctx.caller, "anthropic-key", {
      material: { key: KEY_A },
      authorizedDevices: [ctx.device.deviceId],
      activate: "no" as unknown as boolean
    });
    expect(bad).toMatchObject({ ok: false, error: "bad_request" });
    expect(await slots(ctx)).toHaveLength(0);
  });
});

describe("upload reply", () => {
  it("names the slot: stable for the same identity, new for another", async () => {
    const ctx = await setup();
    const first = value(await putKey(ctx, KEY_A));
    expect(first.slot).toBeTypeOf("string");
    expect((await slots(ctx)).map((a) => a.slot)).toEqual([first.slot]);

    const again = value(await putKey(ctx, KEY_A, { activate: false }));
    expect(again.slot).toBe(first.slot);
    const second = value(await putKey(ctx, KEY_B));
    expect(second.slot).not.toBe(first.slot);
    expect((await slots(ctx)).find((a) => a.active)?.slot).toBe(second.slot);
    // The reply is still the status view.
    expect(second.connections.find((c) => c.provider === "anthropic-key")).toMatchObject({ accounts: 2 });
  });
});

describe("reserved device id", () => {
  it("\"*\" cannot be enrolled as a device or a parent", async () => {
    const ctx = await setup();
    const star = await makeDevice("*");
    expect(
      await vault().enrollDevice(ctx.caller, { deviceId: "*", kind: "laptop", publicKey: star.publicKey })
    ).toMatchObject({ ok: false, error: "bad_request" });
    const child = await makeDevice(`session-${crypto.randomUUID()}`);
    expect(
      await vault().enrollDevice(ctx.caller, { deviceId: child.deviceId, kind: "cloud", publicKey: child.publicKey, parentId: "*" })
    ).toMatchObject({ ok: false, error: "bad_request" });
    const status = value(await vault().status(ctx.caller));
    expect(status.devices.map((d) => d.deviceId)).toEqual([ctx.device.deviceId]);
  });
});

describe("profile on re-upload", () => {
  it("an upload without a profile keeps the stored one; a new profile replaces it", async () => {
    const ctx = await setup();
    const tokens = codexTokens(Date.now() + 5 * DAY);
    const upload = (profile?: { email?: string; plan?: string }) =>
      vault().putCredential(ctx.caller, "codex", {
        material: codexMaterial(tokens),
        authorizedDevices: [ctx.device.deviceId],
        ...(profile ? { profile } : {})
      });
    const view = async () => {
      const accounts = value(await vault().accounts(ctx.caller, "codex"));
      expect(accounts).toHaveLength(1);
      return accounts[0]!;
    };

    value(await upload({ email: "ada@example.com", plan: "plus" }));
    expect(await view()).toMatchObject({ email: "ada@example.com", plan: "plus" });

    value(await upload());
    expect(await view()).toMatchObject({ email: "ada@example.com", plan: "plus" });
    // An empty profile object cleans to nothing and keeps it too.
    value(await upload({}));
    expect(await view()).toMatchObject({ email: "ada@example.com", plan: "plus" });

    value(await upload({ email: "ada@example.com", plan: "pro" }));
    expect(await view()).toMatchObject({ email: "ada@example.com", plan: "pro" });
  });
});
