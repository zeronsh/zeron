import { describe, expect, it } from "vitest";
import { SandboxError, type ExecResult, type SandboxProvider, type SandboxState } from "./sandbox-provider";
import { ENGINE_START_GRACE_MS, FAST_WAKE_DEADLINE_MS, fastWake, parseEngineTiming, parseWakeStage, slowWakeMs, wakePollMs, wakeTiming } from "./wake";

/** A provider on a scripted clock: every state read and timer moves it. */
const scripted = (script: {
  states: (SandboxState | Error)[];
  resume?: () => Promise<void>;
  exec?: () => Promise<ExecResult>;
  execAfterMs?: number;
}) => {
  let clock = 1_000;
  const calls: string[] = [];
  const reads = [...script.states];
  const provider = {
    name: "scripted",
    capabilities: { ttlRequired: false, maxTtlSeconds: null },
    get: async () => {
      calls.push("get");
      const next = reads.length > 1 ? reads.shift()! : reads[0]!;
      if (next instanceof Error) throw next;
      return { state: next };
    },
    resume: async () => {
      calls.push("resume");
      await script.resume?.();
    },
    exec: async () => {
      calls.push("exec");
      return (script.exec ?? (() => new Promise<ExecResult>(() => undefined)))();
    }
  } as unknown as SandboxProvider;
  return {
    provider,
    calls,
    options: {
      ttlSeconds: null,
      startCommand: "start",
      now: () => clock,
      sleep: async (ms: number) => {
        clock += ms;
        await Promise.resolve();
      }
    },
    advance: (ms: number) => {
      clock += ms;
    },
    time: () => clock
  };
};

const ran: ExecResult = { exitCode: 0, stdout: "", stderr: "" };

describe("fastWake", () => {
  it("polls in hundreds of milliseconds, not the seconds a Workflow sleep costs", () => {
    expect(wakePollMs(0)).toBe(250);
    expect(wakePollMs(9_999)).toBe(250);
    expect(wakePollMs(10_000)).toBe(500);
    expect(wakePollMs(30_000)).toBe(1_000);
  });

  it("resumes, starts the engine without waiting for ready, and sees ready within a poll", async () => {
    const s = scripted({ states: ["stopped", "provisioning", "provisioning", "ready"] });
    const wake = await fastWake(s.provider, "bx", s.options);
    expect(s.calls.slice(0, 3)).toEqual(["get", "resume", "exec"]);
    expect(wake.state).toBe("ready");
    // Three polls: 750 ms after the resume, where a Workflow sleep charged 2 s a poll.
    expect(wake.stages.ready! - wake.stages.resumed!).toBeLessThanOrEqual(750 + ENGINE_START_GRACE_MS);
    expect(wake.stages.ready! - wake.stages.resumed!).toBe(750);
  });

  it("calls a machine up as soon as a command ran on it, whatever its state reads", async () => {
    const s = scripted({ states: ["stopped", "provisioning"], exec: async () => ran });
    const wake = await fastWake(s.provider, "bx", s.options);
    expect(wake.state).toBe("ready");
    expect(wake.stages.engineExit).toBe(0);
    expect(wake.stages.engineStarted).toBeDefined();
    // The first read (to decide on the resume), and at most the one in flight.
    expect(s.calls.filter((c) => c === "get").length).toBeLessThanOrEqual(2);
  });

  it("retries a command the machine hasn't caught up with the resume for", async () => {
    let tries = 0;
    const s = scripted({
      states: ["stopped", "provisioning"],
      exec: async () => {
        if (++tries < 2) throw new SandboxError("conflict", "not ready", "sandbox_not_ready", "boat");
        return ran;
      }
    });
    expect((await fastWake(s.provider, "bx", s.options)).stages.engineExit).toBe(0);
    expect(tries).toBe(2);
  });

  it("doesn't resume a machine already on its way up, and still starts the engine", async () => {
    const s = scripted({ states: ["provisioning", "ready"], exec: async () => ran });
    await fastWake(s.provider, "bx", s.options);
    expect(s.calls).not.toContain("resume");
    expect(s.calls).toContain("exec");
  });

  it("retries a start the provider isn't ready for, on the same step", async () => {
    let tries = 0;
    const s = scripted({
      states: ["stopped", "provisioning"],
      exec: async () => {
        if (++tries < 3) throw new SandboxError("retryable", "starting", "sandbox_starting", "boat");
        return ran;
      }
    });
    const wake = await fastWake(s.provider, "bx", s.options);
    expect(tries).toBe(3);
    expect(wake.stages.engineExit).toBe(0);
  });

  it("leaves a start it can't send to the wait and restart that follow", async () => {
    const s = scripted({
      states: ["stopped", "ready"],
      exec: async () => {
        throw new SandboxError("config", "no exec", "api_key_action_forbidden", "boat", "exec");
      }
    });
    const wake = await fastWake(s.provider, "bx", s.options);
    expect(wake.state).toBe("ready");
    expect(wake.stages.engineExit).toBeNull();
  });

  it("doesn't hold the wake on a start that won't answer", async () => {
    const s = scripted({ states: ["stopped", "ready"] });
    const before = s.time();
    const wake = await fastWake(s.provider, "bx", s.options);
    expect(wake.state).toBe("ready");
    expect(wake.stages.engineExit).toBeUndefined();
    expect(s.time() - before).toBeLessThanOrEqual(250 + ENGINE_START_GRACE_MS);
  });

  it("resumes during a stop, and leaves a refusal to the slow path", async () => {
    const taken = scripted({ states: ["stopping", "ready"] });
    await fastWake(taken.provider, "bx", taken.options);
    expect(taken.calls).toContain("resume");

    const refused = scripted({
      states: ["stopping"],
      resume: async () => {
        throw new SandboxError("conflict", "still stopping", "sandbox_stopping", "boat");
      }
    });
    expect((await fastWake(refused.provider, "bx", refused.options)).state).toBe("stopping");
    expect(refused.calls).not.toContain("exec");
  });

  it("ends the step for a refused resume, and retries a passing one", async () => {
    const refused = scripted({
      states: ["stopped"],
      resume: async () => {
        throw new SandboxError("config", "no resume", "api_key_action_forbidden", "boat", "resume");
      }
    });
    await expect(fastWake(refused.provider, "bx", refused.options)).rejects.toMatchObject({ kind: "config" });

    let tries = 0;
    const flaky = scripted({
      states: ["stopped", "ready"],
      resume: async () => {
        if (++tries < 2) throw new SandboxError("retryable", "502", "http_502", "boat");
      }
    });
    expect((await fastWake(flaky.provider, "bx", flaky.options)).state).toBe("ready");
    expect(tries).toBe(2);
  });

  it("resumes on the size asked for, and returns a refusal a retry can't fix", async () => {
    const seen: unknown[] = [];
    const s = scripted({ states: ["stopped", "ready"] });
    const resume = s.provider.resume.bind(s.provider);
    s.provider.resume = async (id, options) => {
      seen.push(options);
      await resume(id, options);
    };
    await fastWake(s.provider, "bx", { ...s.options, machine: "large" });
    expect(seen).toEqual([{ ttlSeconds: null, machine: "large" }]);

    const failure = { kind: "disk_too_small", neededGiB: 118 } as const;
    const refused = scripted({
      states: ["stopped"],
      resume: async () => {
        throw new SandboxError("conflict", "too small", "type_too_small", "boat", undefined, failure);
      }
    });
    expect((await fastWake(refused.provider, "bx", refused.options)).refused).toEqual(failure);
    expect(refused.calls).not.toContain("exec");
  });

  it("reports a machine gone or broken instead of waiting on it", async () => {
    const gone = scripted({ states: ["deleted"] });
    expect((await fastWake(gone.provider, "bx", gone.options)).state).toBe("deleted");
    const broke = scripted({ states: ["stopped", "provisioning", "error"] });
    expect((await fastWake(broke.provider, "bx", broke.options)).state).toBe("error");
  });

  it("resumes a sandbox in the provider's error state: only that gets it out", async () => {
    const s = scripted({ states: ["error", "ready"], exec: async () => ran });
    const wake = await fastWake(s.provider, "bx", { ...s.options, machine: "large" });
    expect(s.calls).toContain("resume");
    expect(wake.state).toBe("ready");
  });

  it("gives up at its deadline so the slow path can take over", async () => {
    const s = scripted({ states: ["stopped", "provisioning"] });
    const wake = await fastWake(s.provider, "bx", { ...s.options, deadlineMs: 5_000 });
    expect(wake.state).toBe("provisioning");
    expect(wake.stages.ready).toBeUndefined();
    expect(s.time()).toBeLessThan(1_000 + 5_000 + ENGINE_START_GRACE_MS + 1_000);
  });

  it("treats a failed state read as one more poll", async () => {
    const s = scripted({ states: ["stopped", new SandboxError("retryable", "502", "http_502", "boat"), "ready"] });
    expect((await fastWake(s.provider, "bx", s.options)).state).toBe("ready");
  });
});

describe("wake timing", () => {
  it("lays every stage out against the moment the wake was requested", () => {
    const timing = wakeTiming(
      {
        requested: 10_000,
        asleepMs: 60_000,
        online: 13_000,
        stages: { started: 10_200, resumed: 11_000, ready: 12_000, engineStarted: 12_100, engineExit: 0 }
      },
      { sent: 9_800, processStart: 12_100, token: 12_900, relay: 13_100, command: 13_400, agentSpawn: 13_500, firstOutput: 15_000 }
    );
    expect(timing.at).toEqual({
      sent: -200,
      requested: 0,
      workflowStarted: 200,
      resumed: 1_000,
      ready: 2_000,
      engineStarted: 2_100,
      processStart: 2_100,
      token: 2_900,
      online: 3_000,
      relay: 3_100,
      command: 3_400,
      agentSpawn: 3_500,
      firstOutput: 5_000
    });
    expect(timing.spans).toMatchObject({ sentToRequested: 200, requestedToOnline: 3_000, sentToFirstOutput: 5_200, sentToAgentSpawn: 3_700 });
    expect(wakeTiming({ requested: 10 }, { sent: 5, agentSpawn: 20, agentStarted: 30 }).spans).toMatchObject({
      agentSpawnToAgentStarted: 10,
      sentToAgentStarted: 25
    });
    expect(timing).toMatchObject({ requestedAt: 10_000, asleepMs: 60_000, engineExit: 0 });
  });

  it("shows the time an update added, and which version answered", () => {
    const timing = wakeTiming(
      { requested: 1_000 },
      {
        engineRun: 2_000,
        updateChecked: 2_100,
        updateStarted: 2_100,
        updateRestart: 14_100,
        resumed: 16_000,
        released: 24_000,
        version: "0.2.140",
        updatedFrom: "0.2.139",
        updatedTo: "0.2.140",
        updateResult: "updated"
      }
    );
    expect(timing.spans).toMatchObject({
      engineRunToUpdateChecked: 100,
      updateStartedToUpdateRestart: 12_000,
      updateRestartToResumed: 1_900,
      resumedToReleased: 8_000,
      updateStartedToReleased: 21_900,
      engineRunToReleased: 22_000
    });
    expect(timing).toMatchObject({ version: "0.2.140", updatedFrom: "0.2.139", updatedTo: "0.2.140", updateResult: "updated" });
  });

  it("takes a known stage from a runner, or none", () => {
    expect(parseWakeStage({ stage: "updating" })).toEqual({ stage: "updating" });
    expect(parseWakeStage({ stage: "slow" })).toEqual({ stage: "slow" });
    expect(parseWakeStage({ stage: null })).toEqual({ stage: null });
    expect(parseWakeStage({})).toEqual({ stage: null });
    expect(parseWakeStage({ stage: "hacking" })).toBeUndefined();
    expect(parseWakeStage("updating")).toBeUndefined();
  });

  it("writes what it has when the engine reports nothing", () => {
    const timing = wakeTiming({ requested: 5, online: 8 });
    expect(timing.at).toEqual({ requested: 0, online: 3 });
    expect(timing.spans).toEqual({ requestedToOnline: 3 });
  });

  it("takes only known stages and plausible times from a runner", () => {
    const now = 1_000_000;
    expect(parseEngineTiming({ token: 5, bogus: "x", firstOutput: null }, now)).toEqual({ token: 5 });
    expect(parseEngineTiming({ token: "5" }, now)).toBeUndefined();
    expect(parseEngineTiming({ token: 1.5 }, now)).toBeUndefined();
    expect(parseEngineTiming({ token: -1 }, now)).toBeUndefined();
    expect(parseEngineTiming({ token: now + 86_400_001 }, now)).toBeUndefined();
    expect(parseEngineTiming("nope", now)).toBeUndefined();
    expect(parseEngineTiming({ version: "0.2.140", updateResult: "wentBack" }, now)).toEqual({ version: "0.2.140", updateResult: "wentBack" });
    expect(parseEngineTiming({ version: "<script>" }, now)).toBeUndefined();
    expect(parseEngineTiming({ updateResult: 5 }, now)).toBeUndefined();
  });
});

describe("slowWakeMs", () => {
  it("is 45 s unless configured", () => {
    expect(slowWakeMs(undefined)).toBe(FAST_WAKE_DEADLINE_MS);
    expect(FAST_WAKE_DEADLINE_MS).toBe(45_000);
    expect(slowWakeMs("3")).toBe(3_000);
    expect(slowWakeMs("nope")).toBe(FAST_WAKE_DEADLINE_MS);
    expect(slowWakeMs("0")).toBe(FAST_WAKE_DEADLINE_MS);
  });
});
