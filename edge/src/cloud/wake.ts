/**
 * Waking a sleeping session's machine fast, and writing down how fast it was.
 *
 * `fastWake` is what WakeWorkflow's first step runs. It does everything in one
 * step, because a Workflow sleep is the coarsest clock in the path: a poll
 * every `step.sleep("2 seconds")` (then 10) charged whole seconds to a machine
 * that was already up. Inside a step a plain timer costs nothing but the poll.
 *
 * The engine is started as soon as the resume is accepted rather than after
 * the machine reads ready: the provider holds a command sent during start-up
 * until the machine can run it, so the exec races the machine's own start of
 * its enabled units and neither waits for the other.
 *
 * Pure of Workflows and of Boat: `SandboxProvider` only, so the timing can be
 * tested with a scripted clock. See docs/design/cloud-device.md, "Waking".
 */
import { SandboxError, type SandboxFailure, type SandboxMachine, type SandboxProvider, type SandboxState } from "./sandbox-provider";

/** Give up on the fast path after this long; WakeWorkflow's slow poll takes
 * over (a restore this slow is not one a tight poll would help). */
export const FAST_WAKE_DEADLINE_MS = 45_000;
/** How long past "ready" to wait for the engine's start command to answer. */
export const ENGINE_START_GRACE_MS = 3_000;
const CALL_RETRY_MS = 500;
const RESUME_ATTEMPTS = 3;

/** The poll's pace: tight while a resume usually lands, easing off after. */
export const wakePollMs = (elapsedMs: number): number => {
  if (elapsedMs < 10_000) return 250;
  return elapsedMs < 30_000 ? 500 : 1_000;
};

/** When each part of a wake's machine work happened (Unix ms). */
export interface WakeStages {
  readonly started: number;
  resumed?: number;
  ready?: number;
  /** The start command answered, whichever of it and the boot won. */
  engineStarted?: number;
  /** Its exit status; absent while it hadn't answered, null when it failed. */
  engineExit?: number | null;
  /** Ran a second time in its own step (the first didn't get through). */
  engineRetried?: number;
}

export interface FastWake {
  /** The last state seen: `ready`, or the slow path takes over from here. */
  readonly state: SandboxState;
  readonly stages: WakeStages;
  /** A resume refused in a way a retry can't fix: returned, not thrown, since a
   * thrown error reaches the workflow as text once it crosses the step. */
  readonly refused?: SandboxFailure;
}

export interface FastWakeOptions {
  readonly ttlSeconds: number | null;
  /** Come back on this size (omitted: the size it had). */
  readonly machine?: SandboxMachine;
  readonly startCommand: string;
  readonly deadlineMs?: number;
  readonly now?: () => number;
  readonly sleep?: (ms: number) => Promise<unknown>;
}

const transient = (e: unknown): boolean => !(e instanceof SandboxError) || e.kind === "retryable";

export const fastWake = async (
  provider: SandboxProvider,
  sandboxId: string,
  options: FastWakeOptions
): Promise<FastWake> => {
  const now = options.now ?? Date.now;
  const sleep = options.sleep ?? ((ms: number) => new Promise((resolve) => setTimeout(resolve, ms)));
  const started = now();
  const deadline = started + (options.deadlineMs ?? FAST_WAKE_DEADLINE_MS);
  const stages: WakeStages = { started };

  /** A transient failure is one more poll; anything else ends the step. */
  const read = async (fallback: SandboxState): Promise<SandboxState> => {
    try {
      return (await provider.get(sandboxId)).state;
    } catch (e) {
      if (transient(e)) return fallback;
      throw e;
    }
  };

  let state = await read("stopped");
  if (state === "error" || state === "deleted") return { state, stages };

  // Anything else is already on its way up (a retried step, another wake).
  if (state === "stopped" || state === "stopping") {
    // A resume during a stop keeps the machine and downloads nothing; a
    // provider that refuses it leaves the slow path to wait the stop out.
    const stopping = state === "stopping";
    for (let attempt = 1; ; attempt++) {
      try {
        await provider.resume(sandboxId, { ttlSeconds: options.ttlSeconds, ...(options.machine ? { machine: options.machine } : {}) });
        stages.resumed = now();
        state = "provisioning";
        break;
      } catch (e) {
        if (e instanceof SandboxError && e.failure) return { state, stages, refused: e.failure };
        if (stopping && !transient(e)) return { state, stages };
        if (!transient(e) || attempt >= RESUME_ATTEMPTS) throw e;
        await sleep(CALL_RETRY_MS);
      }
    }
  }

  // Best effort, and idempotent: the wait for the engine (and its restart)
  // still covers a start that never gets through.
  let answered = false;
  let over = false;
  const startEngine = async (): Promise<void> => {
    while (!over && now() < deadline) {
      try {
        const result = await provider.exec(sandboxId, { command: options.startCommand, timeoutSeconds: 60 });
        stages.engineStarted = now();
        stages.engineExit = result.exitCode;
        answered = true;
        return;
      } catch (e) {
        // A conflict is a machine that hasn't caught up with the resume yet.
        if (!transient(e) && !(e instanceof SandboxError && e.kind === "conflict")) {
          console.warn("cloud wake: starting the engine failed", String(e));
          stages.engineExit = null;
          answered = true;
          return;
        }
        await sleep(CALL_RETRY_MS);
      }
    }
  };
  const engine = startEngine();

  // A command that ran proves the machine is up, whatever the state reads.
  const commandRan = () => answered && typeof stages.engineExit === "number";
  while (state !== "ready" && !commandRan() && now() < deadline) {
    await sleep(wakePollMs(now() - started));
    if (commandRan()) break;
    state = await read(state);
    if (state === "error" || state === "deleted") {
      over = true;
      return { state, stages };
    }
  }
  if (commandRan()) state = "ready";
  if (state === "ready") stages.ready = now();
  if (!answered) await Promise.race([engine, sleep(ENGINE_START_GRACE_MS)]);
  over = true;
  return { state, stages };
};

// ── the timeline an operator reads ─────────────────────────────────────────

/** A session's wake, from the DO's side, until its engine reports its part. */
export interface WakeTrace {
  /** The DO decided to wake (a send's nudge reached it, or an explicit wake). */
  readonly requested: number;
  /** How long it had been asleep, when known: whether a longer idle window
   * would have caught this wake. */
  readonly asleepMs?: number;
  /** The runner's first token or heartbeat. */
  online?: number;
  stages?: WakeStages;
  /** The engine's report, when it beat the workflow to its end. */
  engine?: EngineTiming;
}

/** The engine's part of a wake (Unix ms; `crates/engine/src/wake_trace.rs`). */
export interface EngineTiming {
  /** The send's own time (the sender's clock), from the chat's command. */
  sent?: number;
  /** The machine's kernel started. */
  machineBoot?: number;
  processStart?: number;
  engineRun?: number;
  token?: number;
  relay?: number;
  nudge?: number;
  command?: number;
  agentSpawn?: number;
  /** The agent CLI reported its session: it is up. */
  agentStarted?: number;
  /** Its first text, thought or tool call (includes the model's own wait). */
  firstOutput?: number;
}

const ENGINE_KEYS = [
  "sent",
  "machineBoot",
  "processStart",
  "engineRun",
  "token",
  "relay",
  "nudge",
  "command",
  "agentSpawn",
  "agentStarted",
  "firstOutput"
] as const satisfies readonly (keyof EngineTiming)[];

/** A runner token may be stolen: only known keys and plausible times get in. */
export const parseEngineTiming = (body: unknown, now: number): EngineTiming | undefined => {
  if (typeof body !== "object" || body === null) return undefined;
  const raw = body as Record<string, unknown>;
  const timing: EngineTiming = {};
  for (const key of ENGINE_KEYS) {
    const value = raw[key];
    if (value === undefined || value === null) continue;
    if (typeof value !== "number" || !Number.isInteger(value)) return undefined;
    if (value < 0 || value > now + 86_400_000) return undefined;
    timing[key] = value;
  }
  return timing;
};

/** Every stage in the order it happens, with what produced its timestamp. */
const timeline = (trace: WakeTrace, engine: EngineTiming | undefined): readonly (readonly [string, number | undefined])[] => [
  ["sent", engine?.sent],
  ["requested", trace.requested],
  ["workflowStarted", trace.stages?.started],
  ["resumed", trace.stages?.resumed],
  ["machineBoot", engine?.machineBoot],
  ["ready", trace.stages?.ready],
  ["engineStarted", trace.stages?.engineStarted],
  ["processStart", engine?.processStart],
  ["engineRun", engine?.engineRun],
  ["token", engine?.token],
  ["online", trace.online],
  ["relay", engine?.relay],
  ["nudge", engine?.nudge],
  ["command", engine?.command],
  ["agentSpawn", engine?.agentSpawn],
  ["agentStarted", engine?.agentStarted],
  ["firstOutput", engine?.firstOutput]
];

export interface WakeTiming {
  /** Each stage reached, as milliseconds after the wake was requested
   * (negative: the send, which came first; other clocks can lag a little). */
  readonly at: Readonly<Record<string, number>>;
  /** What an operator asks first: whole milliseconds per user-felt span. */
  readonly spans: Readonly<Record<string, number>>;
  readonly requestedAt: number;
  readonly asleepMs?: number;
  readonly engineExit?: number | null;
  readonly engineRetried?: boolean;
}

/** The ledger's `wake_timing` detail: one JSON object, safe to compare. */
export const wakeTiming = (trace: WakeTrace, engine?: EngineTiming): WakeTiming => {
  const at: Record<string, number> = {};
  for (const [stage, time] of timeline(trace, engine)) {
    if (time !== undefined) at[stage] = time - trace.requested;
  }
  const span = (from: string, to: string): [string, number][] =>
    at[from] !== undefined && at[to] !== undefined ? [[`${from}To${to[0]!.toUpperCase()}${to.slice(1)}`, at[to]! - at[from]!]] : [];
  return {
    at,
    spans: Object.fromEntries([
      ...span("sent", "requested"),
      ...span("requested", "online"),
      ...span("requested", "ready"),
      ...span("online", "command"),
      ...span("command", "agentSpawn"),
      ...span("agentSpawn", "agentStarted"),
      ...span("agentStarted", "firstOutput"),
      ...span("sent", "agentSpawn"),
      ...span("sent", "agentStarted"),
      ...span("sent", "firstOutput")
    ]),
    requestedAt: trace.requested,
    ...(trace.asleepMs === undefined ? {} : { asleepMs: trace.asleepMs }),
    ...(trace.stages?.engineExit === undefined ? {} : { engineExit: trace.stages.engineExit }),
    ...(trace.stages?.engineRetried === undefined ? {} : { engineRetried: true })
  };
};
