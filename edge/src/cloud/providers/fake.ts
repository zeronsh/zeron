/**
 * An in-memory `SandboxProvider` (`SANDBOX_PROVIDER=fake`): for `wrangler
 * dev` against no real machines, and for provider-contract tests. State is
 * per isolate and per instance, so it is only coherent where one isolate
 * runs everything (local dev); it never provisions anything.
 *
 * Semantics mirror what a real provider must guarantee: create is
 * idempotent per key (a different request under the same key conflicts),
 * a fresh sandbox becomes ready on its first `get`, stop/resume pass through
 * `stopping`/`provisioning`, usage is metered only while ready (stopped time
 * never counts) and is gone after delete.
 */
import {
  SandboxError,
  type CreateSandbox,
  type ExecResult,
  type ListedSandbox,
  type SandboxProvider,
  type SandboxState,
  type SandboxUsage
} from "../sandbox-provider";

interface FakeMachine {
  id: string;
  state: SandboxState;
  machine: string;
  env: Record<string, string>;
  createdAt: number;
  ttlSeconds: number | null;
  /** The template it started from, if any. */
  from?: string;
  /** Running stretches `[start, end)`; an open stretch has no end. */
  stretches: { start: number; end?: number }[];
  execs: string[];
}

const SECONDS_PER_DOLLAR = 100_000;

export class FakeProvider implements SandboxProvider {
  readonly name = "fake";
  readonly capabilities = { ttlRequired: false, maxTtlSeconds: null };
  private readonly machines = new Map<string, FakeMachine>();
  private readonly keys = new Map<string, { id: string; request: string }>();

  constructor(private readonly now: () => number = Date.now) {}

  private find(id: string): FakeMachine {
    const machine = this.machines.get(id);
    if (!machine || machine.state === "deleted") {
      throw new SandboxError("notFound", `no sandbox ${id}`, "not_found", this.name);
    }
    return machine;
  }

  /** Test hook: what ran on a sandbox. */
  execsOf(id: string): readonly string[] {
    return this.machines.get(id)?.execs ?? [];
  }

  /** Dev hook (`GET /dev/cloud/fake-sandboxes`): every machine with the env
   * it was created with, so a local e2e can start a real runner engine with
   * the enrollment a real sandbox would have received. */
  snapshot(): { id: string; state: SandboxState; env: Record<string, string>; from?: string; execs: readonly string[] }[] {
    return [...this.machines.values()].map(({ id, state, env, from, execs }) => ({
      id,
      state,
      env,
      ...(from ? { from } : {}),
      execs
    }));
  }

  async create(request: CreateSandbox): Promise<{ sandboxId: string; createdAt: number }> {
    const fingerprint = JSON.stringify([request.env, request.machine, request.ttlSeconds, request.from ?? null]);
    const prior = this.keys.get(request.idempotencyKey);
    if (prior) {
      if (prior.request !== fingerprint) {
        throw new SandboxError("conflict", "idempotency key reused with a different request", "idempotency_key_reused", this.name);
      }
      const machine = this.find(prior.id);
      return { sandboxId: machine.id, createdAt: machine.createdAt };
    }
    const now = this.now();
    const id = `fk_${crypto.randomUUID().slice(0, 8)}`;
    this.machines.set(id, {
      id,
      state: "provisioning",
      machine: request.machine,
      env: { ...request.env },
      createdAt: now,
      ttlSeconds: request.ttlSeconds,
      ...(request.from ? { from: request.from } : {}),
      stretches: [{ start: now }],
      execs: []
    });
    this.keys.set(request.idempotencyKey, { id, request: fingerprint });
    return { sandboxId: id, createdAt: now };
  }

  async get(sandboxId: string) {
    const machine = this.machines.get(sandboxId);
    if (!machine || machine.state === "deleted") return { state: "deleted" as const };
    if (machine.state === "provisioning") machine.state = "ready";
    else if (machine.state === "stopping") machine.state = "stopped";
    return { state: machine.state };
  }

  async stop(sandboxId: string): Promise<void> {
    const machine = this.find(sandboxId);
    if (machine.state === "stopped" || machine.state === "stopping") return;
    machine.state = "stopping";
    const open = machine.stretches.at(-1);
    if (open && open.end === undefined) open.end = this.now();
  }

  async resume(sandboxId: string, options: { ttlSeconds: number | null }): Promise<void> {
    const machine = this.find(sandboxId);
    if (machine.state !== "stopped") {
      throw new SandboxError("conflict", `cannot resume a ${machine.state} sandbox`, "not_stopped", this.name);
    }
    machine.state = "provisioning";
    machine.ttlSeconds = options.ttlSeconds;
    machine.stretches.push({ start: this.now() });
  }

  async delete(sandboxId: string): Promise<{ operationId: string }> {
    const machine = this.find(sandboxId);
    machine.state = "deleted";
    return { operationId: `fkop_${sandboxId}` };
  }

  async extendTtl(sandboxId: string, ttlSeconds: number | null): Promise<void> {
    this.find(sandboxId).ttlSeconds = ttlSeconds;
  }

  async exec(sandboxId: string, options: { command: string; timeoutSeconds: number }): Promise<ExecResult> {
    const machine = this.find(sandboxId);
    if (machine.state !== "ready") {
      throw new SandboxError("conflict", `sandbox is ${machine.state}`, "not_ready", this.name);
    }
    machine.execs.push(options.command);
    return { exitCode: 0, stdout: "", stderr: "" };
  }

  async usage(sandboxId: string, window: { since: number; until: number }): Promise<SandboxUsage> {
    const machine = this.find(sandboxId);
    const until = Math.min(window.until, this.now());
    let ms = 0;
    for (const stretch of machine.stretches) {
      const start = Math.max(stretch.start, window.since);
      const end = Math.min(stretch.end ?? until, until);
      if (end > start) ms += end - start;
    }
    const seconds = Math.floor(ms / 1000);
    return {
      seconds,
      dollars: Math.round((seconds / SECONDS_PER_DOLLAR) * 1e6) / 1e6,
      machine: machine.machine,
      running: machine.stretches.at(-1)?.end === undefined
    };
  }

  async *list(): AsyncIterable<ListedSandbox> {
    for (const machine of this.machines.values()) {
      if (machine.state !== "deleted") {
        yield { sandboxId: machine.id, state: machine.state, createdAt: machine.createdAt };
      }
    }
  }
}
