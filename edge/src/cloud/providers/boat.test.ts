import { describe, expect, it } from "vitest";
import { SandboxError, classifyFailure, sandboxUserMessage } from "../sandbox-provider";
import { BoatProvider, boatErrorKind, boatFailure, boatState } from "./boat";

type Call = { url: string; init: RequestInit };

const fakeBoat = (reply: (call: Call) => Response) => {
  const calls: Call[] = [];
  const boat = new BoatProvider({
    apiKey: "k",
    base: "https://boat.test/api/v1/",
    fetch: async (url, init) => {
      const call = { url, init: init ?? {} };
      calls.push(call);
      return reply(call);
    }
  });
  return { boat, calls };
};

const jsonReply = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const boatErr = (status: number, code: string, extra: Record<string, unknown> = {}) =>
  jsonReply(status, { ok: false, type: "sandbox.error", status, code, message: `${code} happened`, ...extra });

describe("Boat provider wire format", () => {
  it("creates noEnv with the idempotency key and maps machine → type", async () => {
    const { boat, calls } = fakeBoat(() =>
      jsonReply(202, {
        ok: true,
        type: "sandbox.created",
        status: "provisioning",
        sandbox: { id: "bx_23456789", state: "provisioning", createdAt: "2026-10-01T12:00:00Z" }
      })
    );
    const created = await boat.create({ idempotencyKey: "cloud-x-g1", env: { A: "1" }, machine: "default", ttlSeconds: 7200 });
    expect(created).toEqual({ sandboxId: "bx_23456789", createdAt: Date.parse("2026-10-01T12:00:00Z") });
    expect(calls[0]!.url).toBe("https://boat.test/api/v1/sandboxes");
    const headers = new Headers(calls[0]!.init.headers);
    expect(headers.get("idempotency-key")).toBe("cloud-x-g1");
    expect(headers.get("authorization")).toBe("Bearer k");
    expect(JSON.parse(calls[0]!.init.body as string)).toEqual({ type: "default", ttlSeconds: 7200, noEnv: true, env: { A: "1" } });
  });

  it("deletes only with the confirm header and returns the operation id", async () => {
    const { boat, calls } = fakeBoat(() =>
      jsonReply(202, { ok: true, type: "sandbox.deleting", operation: { id: "bdop_1", status: "pending" } })
    );
    expect(await boat.delete("bx_23456789")).toEqual({ operationId: "bdop_1" });
    expect(calls[0]!.init.method).toBe("DELETE");
    expect(new Headers(calls[0]!.init.headers).get("x-ascii-confirm-delete")).toBe("bx_23456789");
  });

  it("treats a resume already under way as done, and any other refusal as a refusal", async () => {
    const refusal = (code: string) =>
      fakeBoat(() => jsonReply(409, { ok: false, code, message: code, error: { code, message: code } })).boat;
    await expect(refusal("sandbox_resuming").resume("bx_1", { ttlSeconds: null })).resolves.toBeUndefined();
    await expect(refusal("resume_failed").resume("bx_1", { ttlSeconds: null })).rejects.toMatchObject({ kind: "conflict" });
  });

  it("calls a command sent to a machine still starting worth retrying", () => {
    expect(boatErrorKind(409, "sandbox_starting")).toBe("retryable");
    expect(boatErrorKind(502, "sandbox_direct_failed")).toBe("retryable");
  });

  it("never forces a stop and resumes with the TTL", async () => {
    const { boat, calls } = fakeBoat(() => jsonReply(202, { ok: true, sandbox: { id: "bx_1", state: "archiving" } }));
    await boat.stop("bx_1");
    await boat.resume("bx_1", { ttlSeconds: 7200 });
    expect(JSON.parse(calls[0]!.init.body as string)).toEqual({});
    expect(JSON.parse(calls[1]!.init.body as string)).toEqual({ ttlSeconds: 7200 });
  });

  it("resumes onto another size only when asked", async () => {
    const { boat, calls } = fakeBoat(() => jsonReply(202, { ok: true, sandbox: { id: "bx_1", state: "provisioning" } }));
    await boat.resume("bx_1", { ttlSeconds: null, machine: "xlarge" });
    await boat.resume("bx_1", { ttlSeconds: null });
    expect(JSON.parse(calls[0]!.init.body as string)).toEqual({ type: "xlarge", ttlSeconds: null });
    expect(JSON.parse(calls[1]!.init.body as string)).toEqual({ ttlSeconds: null });
  });

  it("types a resume its disk doesn't fit, with the figure, never retried", async () => {
    const message = "This Sandbox needs 118.3 GiB of disk but the largest 'large' machine holds 111.8 GiB. Resume or fork it as a bigger type (--type).";
    // Whatever code and hint it arrives with.
    for (const reply of [
      boatErr(409, "sandbox_too_big", { message }),
      boatErr(400, "bad_request", { message }),
      boatErr(503, "no_ready_machine", { message, retryable: true })
    ]) {
      const { boat } = fakeBoat(() => reply);
      const error = await boat.resume("bx_1", { ttlSeconds: 7200 }).catch((e: unknown) => e);
      expect(error).toMatchObject({ failure: { kind: "disk_too_small", neededGiB: 118.3 } });
      expect(classifyFailure(error, "wake").retryable).toBe(false);
    }
  });

  it("types the documented refusals without a figure", async () => {
    expect(boatFailure("type_too_small", "Too much data.")).toEqual({ kind: "disk_too_small" });
    // Real Boat, a 400 on a resume to a smaller type: decimal GB with the base
    // image in. The same machine read "needs 51.8 GiB" on its own size.
    expect(
      boatFailure(
        "type_too_small",
        "This sandbox uses 77.7 GB of disk, which does not fit a 'default' machine (76.0 GB). Delete data in the sandbox and let it snapshot again, or resume it as 'large' or larger."
      )
    ).toEqual({ kind: "disk_too_small", neededGiB: 51.9 });
    expect(boatFailure("snapshot_data_expired", "gone")).toEqual({ kind: "data_expired" });
    const { boat } = fakeBoat(() => boatErr(409, "snapshot_data_expired"));
    await expect(boat.resume("bx_1", { ttlSeconds: null })).rejects.toMatchObject({ failure: { kind: "data_expired" } });
  });

  it("leaves every other error untyped", async () => {
    expect(boatFailure("rate_limited", "Slow down")).toBeUndefined();
    expect(boatFailure(undefined, null)).toBeUndefined();
    expect(boatFailure(undefined, "needs a bigger plan")).toBeUndefined();
    const { boat } = fakeBoat(() => boatErr(429, "rate_limited"));
    const error = await boat.resume("bx_1", { ttlSeconds: null }).catch((e: unknown) => e);
    expect(error).toMatchObject({ kind: "retryable" });
    expect((error as SandboxError).failure).toBeUndefined();
  });

  it("reads expiry, the last stop and hydration off a sandbox", async () => {
    const reading = (sandbox: Record<string, unknown>) => fakeBoat(() => jsonReply(200, { ok: true, sandbox: { id: "bx_1", ...sandbox } })).boat.get("bx_1");
    expect(await reading({ state: "archived", stoppedAt: "2026-10-01T00:00:00Z", dataExpiredAt: null, hydrated: true })).toEqual({
      state: "stopped",
      stoppedAt: Date.parse("2026-10-01T00:00:00Z"),
      hydrated: true
    });
    expect(await reading({ state: "archived", dataExpiredAt: "2026-10-15T00:00:00Z" })).toEqual({ state: "stopped", failure: { kind: "data_expired" } });
    // As real machines have reported it.
    expect(
      await reading({ state: "archived", healthReason: "This sandbox's data was deleted 14 days after it stopped. Save a saved snapshot to keep data: boat snapshot bx_1 name" })
    ).toEqual({ state: "stopped", failure: { kind: "data_expired" } });
    expect(await reading({ state: "idle", hydrated: false })).toEqual({ state: "ready", hydrated: false });
    expect(await reading({ state: "error", error: "This Sandbox needs 5 GiB of disk" })).toEqual({
      state: "error",
      error: "This Sandbox needs 5 GiB of disk",
      failure: { kind: "disk_too_small", neededGiB: 5 }
    });
    expect((new BoatProvider({ apiKey: "k" })).capabilities.stoppedRetentionMs).toBe(14 * 24 * 60 * 60_000);
  });

  it("reads sandbox.state (not the action reply's status) and treats 404 as deleted", async () => {
    const { boat } = fakeBoat((call) =>
      call.url.endsWith("bx_gone")
        ? boatErr(404, "not_found")
        : jsonReply(200, { ok: true, type: "sandbox.info", status: "whatever", sandbox: { id: "bx_1", state: "archived" } })
    );
    expect(await boat.get("bx_1")).toEqual({ state: "stopped" });
    expect(await boat.get("bx_gone")).toEqual({ state: "deleted" });
  });

  it("normalizes a scoped key's missing action into a config error the user can act on", async () => {
    const { boat } = fakeBoat(() =>
      boatErr(403, "api_key_action_forbidden", {
        message: "This API key cannot perform sandbox.resume.",
        error: { code: "api_key_action_forbidden", details: { action: "sandbox.resume" } }
      })
    );
    const error = await boat.resume("bx_1", { ttlSeconds: 7200 }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SandboxError);
    expect(error).toMatchObject({ kind: "config", providerCode: "api_key_action_forbidden", provider: "boat", action: "resume" });
    expect(sandboxUserMessage(error, "wake Cloud")).toBe(
      "Cloud isn't configured to wake sandboxes (the Boat API key lacks sandbox.resume)."
    );
    expect(classifyFailure(error, "wake Cloud").retryable).toBe(false);
  });

  it("honors an explicit retryable hint and maps network failures to retryable", async () => {
    const hinted = fakeBoat(() => boatErr(503, "sandbox_unavailable", { retryable: false }));
    await expect(hinted.boat.stop("bx_1")).rejects.toMatchObject({ kind: "conflict" });
    const down = new BoatProvider({ apiKey: "k", fetch: async () => { throw new TypeError("connect ECONNREFUSED"); } });
    await expect(down.get("bx_1")).rejects.toMatchObject({ kind: "retryable", providerCode: "network" });
  });

  it("queries usage with ISO windows and pages through list", async () => {
    const { boat, calls } = fakeBoat((call) => {
      if (call.url.includes("/usage")) {
        return jsonReply(200, { ok: true, type: "sandbox.usage", sandboxId: "bx_1", sandboxType: "large", seconds: 4980, dollars: 0.0498, running: false });
      }
      const cursor = new URL(call.url).searchParams.get("cursor");
      return cursor
        ? jsonReply(200, { ok: true, sandboxes: [{ id: "bx_2", state: "archived" }], pageInfo: { nextCursor: null, hasMore: false, limit: 200 } })
        : jsonReply(200, { ok: true, sandboxes: [{ id: "bx_1", state: "idle", createdAt: "2026-09-01T00:00:00Z" }], pageInfo: { nextCursor: "c2", hasMore: true, limit: 200 } });
    });
    const since = Date.UTC(2026, 8, 1);
    const until = Date.UTC(2026, 9, 1);
    expect(await boat.usage("bx_1", { since, until })).toEqual({ seconds: 4980, dollars: 0.0498, machine: "large", running: false });
    const query = new URL(calls[0]!.url).searchParams;
    expect(query.get("since")).toBe("2026-09-01T00:00:00.000Z");
    expect(query.get("until")).toBe("2026-10-01T00:00:00.000Z");
    const listed = [];
    for await (const s of boat.list()) listed.push(s);
    expect(listed).toEqual([
      { sandboxId: "bx_1", state: "ready", createdAt: Date.parse("2026-09-01T00:00:00Z") },
      { sandboxId: "bx_2", state: "stopped" }
    ]);
  });
});

describe("Boat vocabulary", () => {
  it.each([
    ["init", "provisioning"],
    ["provisioning", "provisioning"],
    ["provisioned", "provisioning"],
    ["cloning", "provisioning"],
    ["ready", "ready"],
    ["idle", "ready"],
    ["running", "ready"],
    ["archiving", "stopping"],
    ["archived", "stopped"],
    ["error", "error"],
    ["cancelled", "error"]
  ] as const)("state %s → %s", (state, normalized) => {
    expect(boatState(state)).toBe(normalized);
  });

  it.each([
    [0, "network", "retryable"],
    [503, "out_of_capacity", "retryable"],
    [502, "boat_direct_failed", "retryable"],
    [409, "idempotency_in_progress", "retryable"],
    [409, "boat_starting", "retryable"],
    [400, "machine_not_running", "retryable"],
    [429, "rate_limited", "retryable"],
    [409, "idempotency_key_reused", "conflict"],
    [409, "sandbox_not_ready", "conflict"],
    [404, "not_found", "notFound"],
    [403, "api_key_action_forbidden", "config"],
    [401, "unauthorized", "config"],
    [402, "billing_required", "config"],
    [400, "trial_auto_stop_required", "config"],
    [429, "limit_reached", "fatal"],
    [400, "invalid_json", "fatal"]
  ] as const)("%i %s → %s", (status, code, kind) => {
    expect(boatErrorKind(status, code)).toBe(kind);
  });
});
