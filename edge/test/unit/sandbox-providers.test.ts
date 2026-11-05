/**
 * The SandboxProvider contract (src/cloud/sandbox-provider.ts), run against
 * every provider: Boat (over the same fake Boat HTTP API the workerd tier
 * uses) and the in-memory fake. A new provider joins this list.
 */
import { describe, expect, it } from "vitest";
import { BoatProvider } from "../../src/cloud/providers/boat";
import { FakeProvider } from "../../src/cloud/providers/fake";
import { SandboxError, type SandboxProvider, type SandboxState } from "../../src/cloud/sandbox-provider";
import { createBoatFake } from "../workerd/boat-fake";

const providers: [string, () => SandboxProvider][] = [
  [
    "boat",
    () => {
      const api = createBoatFake();
      return new BoatProvider({
        apiKey: "test-boat-key",
        base: "https://boat.test/api/v1",
        fetch: async (input, init) => (await api(new Request(input, init) as never)) as unknown as Response
      });
    }
  ],
  ["fake", () => new FakeProvider()]
];

const settle = async (provider: SandboxProvider, id: string, want: SandboxState) => {
  let state: SandboxState = "provisioning";
  for (let i = 0; i < 5 && state !== want; i++) state = (await provider.get(id)).state;
  return state;
};

describe.each(providers)("%s provider contract", (_name, make) => {
  it("creates idempotently, runs, stops, resumes, meters and deletes", async () => {
    const provider = make();
    const request = { idempotencyKey: "dev-g1", env: { ZERON_X: "1" }, machine: "default" as const, ttlSeconds: 7200 };
    const { sandboxId } = await provider.create(request);
    expect((await provider.create(request)).sandboxId).toBe(sandboxId);
    await expect(provider.create({ ...request, env: { ZERON_X: "2" } })).rejects.toMatchObject({ kind: "conflict" });

    expect(await settle(provider, sandboxId, "ready")).toBe("ready");
    expect((await provider.exec(sandboxId, { command: "true", timeoutSeconds: 10 })).exitCode).toBe(0);
    await provider.extendTtl(sandboxId, 3600);

    const window = { since: Date.now() - 60_000, until: Date.now() + 1 };
    const live = await provider.usage(sandboxId, window);
    expect(live.running).toBe(true);
    expect(live.machine).toBe("default");
    expect(live.seconds).toBeGreaterThanOrEqual(0);

    await provider.stop(sandboxId);
    expect(await settle(provider, sandboxId, "stopped")).toBe("stopped");
    expect((await provider.usage(sandboxId, window)).running).toBe(false);

    await provider.resume(sandboxId, { ttlSeconds: 7200 });
    expect(await settle(provider, sandboxId, "ready")).toBe("ready");

    const listed: string[] = [];
    for await (const s of provider.list()) listed.push(s.sandboxId);
    expect(listed).toContain(sandboxId);

    await provider.stop(sandboxId);
    await settle(provider, sandboxId, "stopped");
    const deleted = await provider.delete(sandboxId);
    expect(deleted.operationId).toEqual(expect.any(String));
    expect((await provider.get(sandboxId)).state).toBe("deleted");
    const gone = await provider.usage(sandboxId, window).catch((e: unknown) => e);
    expect(gone).toBeInstanceOf(SandboxError);
    expect(gone).toMatchObject({ kind: "notFound" });
  });

  it("reports unknown sandboxes as deleted / notFound", async () => {
    const provider = make();
    expect((await provider.get("bx_zzzzzzzz")).state).toBe("deleted");
    await expect(provider.stop("bx_zzzzzzzz")).rejects.toMatchObject({ kind: "notFound" });
  });
});
