/**
 * Cloud session lifecycle workflows (docs/design/cloud-device.md, "Workflows"):
 * provision, wake, sleep, and the explicit delete — one instance per session
 * lifecycle, since every session has its own sandbox.
 *
 * The CloudAccount DO decides; these do the slow work durably. Every provider
 * call is its own `step.do` with retries and backoff, polling sleeps with
 * `step.sleep`, and the engine's enrollment / first contact arrive as
 * events. Each workflow reports back to the DO tagged with the `generation`
 * it was started for, and any terminal failure becomes `state: "error"` with
 * a message the user can act on. Provider-neutral: the provider named in the
 * params (the one that owns the sandbox) is looked up from the registry.
 *
 * Secrets: the enrollment code arrives in the instance params and is used
 * only inside the create/install steps; no step returns it (those steps are
 * also marked `sensitive: "output"`).
 */
import { WorkflowEntrypoint, type WorkflowEvent, type WorkflowStep } from "cloudflare:workers";
import { NonRetryableError } from "cloudflare:workflows";
import type { Env } from "../env";
import type { LifecycleParams, ProvisionParams } from "./cloud-account";
import { RESTART_ENGINE_COMMAND, installCommand, type SessionEnv } from "./install-script";
import { cloudAccountName, parseMachine, parseTemplate, parseTtlSeconds } from "./policy";
import { sandboxProvider } from "./providers";
import {
  SandboxError,
  classifyFailure,
  effectiveTtl,
  stopRetryable,
  type SandboxProvider,
  type SandboxState
} from "./sandbox-provider";

type StepConfig = Parameters<WorkflowStep["do"]>[1] & object;

/** Provider calls: idempotent reads/actions, retried with backoff. */
const PROVIDER_STEP = {
  retries: { limit: 6, delay: "5 seconds", backoff: "exponential" },
  timeout: "2 minutes"
} as const satisfies StepConfig;

/** Reports to the CloudAccount DO (cheap, idempotent per generation). */
const DO_STEP = {
  retries: { limit: 10, delay: "2 seconds", backoff: "exponential" },
  timeout: "30 seconds"
} as const satisfies StepConfig;

/** A refused stop keeps being retried (providers bill nothing meanwhile). */
const STOP_STEP = {
  retries: { limit: 8, delay: "30 seconds", backoff: "exponential" },
  timeout: "2 minutes"
} as const satisfies StepConfig;

/** Errors a step throws: retryable ones go back to the step's policy, the
 * rest end the workflow at once with the user-facing message. */
const providerCall = async <T>(doing: string, run: () => Promise<T>): Promise<T> => {
  try {
    return await run();
  } catch (e) {
    const failure = classifyFailure(e, doing);
    throw failure.retryable ? new Error(failure.message) : new NonRetryableError(failure.message);
  }
};

/** The user-facing message of a failure. Errors that crossed a step
 * boundary come back as "NonRetryableError: …" — the class name is noise. */
const failureMessage = (e: unknown): string => {
  const message = (e instanceof Error ? e.message : String(e)).replace(/^(?:[A-Za-z]*Error: )+/, "");
  return (message || "Cloud setup failed.").slice(0, 300);
};

const tail = (text: string): string => text.trim().split("\n").slice(-5).join(" | ").slice(-300);

const pollDelay = (attempt: number) => (attempt <= 5 ? "2 seconds" : "10 seconds");

const readState = (step: WorkflowStep, provider: SandboxProvider, sandboxId: string, name: string) =>
  step.do(name, PROVIDER_STEP, async () =>
    (await providerCall("check the Cloud sandbox", () => provider.get(sandboxId))).state
  );

const gone = (state: SandboxState): never => {
  throw new NonRetryableError(
    state === "deleted"
      ? "The Cloud sandbox no longer exists. Delete Cloud and enable it again."
      : "The Cloud sandbox is in an error state."
  );
};

/** Poll until `done(state)`; returns the last state seen (the caller decides
 * what a timeout means). A failed or vanished sandbox ends the workflow. */
const pollSandbox = async (
  step: WorkflowStep,
  provider: SandboxProvider,
  sandboxId: string,
  label: string,
  done: (state: SandboxState) => boolean,
  maxPolls: number
): Promise<SandboxState> => {
  let state: SandboxState = "provisioning";
  for (let i = 1; i <= maxPolls; i++) {
    state = await readState(step, provider, sandboxId, `${label}-poll-${i}`);
    if (done(state)) return state;
    if (state === "error" || state === "deleted") gone(state);
    if (i < maxPolls) await step.sleep(`${label}-wait-${i}`, pollDelay(i));
  }
  return state;
};

/** Bring the sandbox up from whatever state it is in: wait out a stop in
 * progress, resume if stopped, then wait until it is ready. */
const ensureRunning = async (
  step: WorkflowStep,
  provider: SandboxProvider,
  sandboxId: string,
  ttlSeconds: number | null,
  label: string
): Promise<void> => {
  let state = await readState(step, provider, sandboxId, `${label}-state`);
  if (state === "error" || state === "deleted") gone(state);
  if (state === "stopping") {
    state = await pollSandbox(step, provider, sandboxId, `${label}-stopped`, (s) => s === "stopped", 40);
  }
  if (state === "stopped") {
    await step.do(`${label}-resume`, PROVIDER_STEP, async () => {
      await providerCall("wake the Cloud sandbox", () => provider.resume(sandboxId, { ttlSeconds }));
      return true;
    });
  }
  if (state !== "ready") {
    const final = await pollSandbox(step, provider, sandboxId, `${label}-ready`, (s) => s === "ready", 60);
    if (final !== "ready") throw new NonRetryableError("The Cloud sandbox didn't start in time.");
  }
};

/**
 * Stop and wait until stopped. Never forced; a refused stop is retried with
 * backoff (STOP_STEP), and a stop the provider silently undid is re-issued,
 * up to three rounds. Returns "stopped", or "error" for a sandbox in the
 * provider's error state (which cannot be snapshotted); throws when it
 * simply won't stop.
 */
const stopSandbox = async (
  step: WorkflowStep,
  provider: SandboxProvider,
  sandboxId: string,
  doing: string
): Promise<"stopped" | "error"> => {
  for (let round = 1; round <= 3; round++) {
    const state = await readState(step, provider, sandboxId, `state-${round}`);
    if (state === "stopped" || state === "error") return state;
    if (state === "deleted") gone(state);
    if (state !== "stopping") {
      await step.do(`stop-${round}`, STOP_STEP, async () => {
        try {
          await provider.stop(sandboxId);
        } catch (e) {
          const { message } = classifyFailure(e, doing);
          throw stopRetryable(e) ? new Error(message) : new NonRetryableError(message);
        }
        return true;
      });
    }
    if ((await pollSandbox(step, provider, sandboxId, `stopped-${round}`, (s) => s === "stopped", 30)) === "stopped") {
      return "stopped";
    }
  }
  throw new NonRetryableError(`Couldn't ${doing}: the Cloud sandbox didn't stop.`);
};

/** Wait for the DO-tracked signal: check first (it may already have
 * happened), wait for the event, re-check after a timeout (an event sent
 * before the wait began is not lost — just slower). */
const awaitSignal = async (
  step: WorkflowStep,
  type: "enrolled" | "online",
  suffix: string,
  check: () => Promise<boolean>,
  timeout: "5 minutes" | "10 minutes"
): Promise<boolean> => {
  if (await step.do(`check-${type}${suffix}`, DO_STEP, check)) return true;
  try {
    await step.waitForEvent(`wait-${type}${suffix}`, { type, timeout });
    return true;
  } catch {
    return step.do(`recheck-${type}${suffix}`, DO_STEP, check);
  }
};

const accountStub = (env: Env, p: { orgId: string; userId: string }) => {
  const ns = env.CLOUD_ACCOUNTS;
  if (!ns) throw new NonRetryableError("Cloud is not configured on this server.");
  return ns.get(ns.idFromName(cloudAccountName(p.orgId, p.userId)));
};

const requireProvider = (env: Env, name: string): SandboxProvider => {
  const provider = sandboxProvider(env, name);
  if (!provider) throw new NonRetryableError(`Cloud is not configured on this server (no "${name}" sandbox provider).`);
  return provider;
};

const ttlFor = (env: Env, provider: SandboxProvider) =>
  effectiveTtl(provider.capabilities, parseTtlSeconds(env.CLOUD_TTL_SECONDS));

/** Everything a session sandbox is provisioned with: its enrollment and its
 * project. Used as the create-time env and passed again to the install. */
const sessionEnv = (p: ProvisionParams): SessionEnv => ({
  ZERON_RUNNER_ENROLL: `${p.orgId}.${p.userId}.${p.deviceId}.${p.enrollCode}`,
  ZERON_EDGE_URL: p.edgeUrl,
  ZERON_DEVICE_NAME: "Cloud session",
  ZERON_DEVICE_PLATFORM: "cloud",
  ZERON_CLOUD_ACCOUNT: p.accountDeviceId,
  ZERON_CLOUD_CHAT: p.chatId,
  ZERON_CLOUD_REPO: p.project.cloneUrl,
  ZERON_CLOUD_PATH: p.project.path,
  ZERON_CLOUD_BRANCH: p.project.defaultBranch
});

/**
 * ProvisionWorkflow — create (or reuse) the session's sandbox, install the
 * engine as a system service, wait for it to enroll, mark the session ready.
 * The engine clones the project itself (with its own GitHub grant) before it
 * runs anything.
 */
export class ProvisionWorkflow extends WorkflowEntrypoint<Env, ProvisionParams> {
  async run(event: WorkflowEvent<ProvisionParams>, step: WorkflowStep): Promise<void> {
    const p = event.payload;
    const account = accountStub(this.env, p);
    try {
      const provider = requireProvider(this.env, p.provider);
      const ttlSeconds = ttlFor(this.env, provider);
      const machine = parseMachine(this.env.CLOUD_MACHINE);
      const template = parseTemplate(this.env.CLOUD_TEMPLATE);
      const env = sessionEnv(p);
      let sandboxId = p.sandboxId;
      if (!sandboxId) {
        const created = await step.do("create-sandbox", { ...PROVIDER_STEP, sensitive: "output" }, async () => {
          const sandbox = await providerCall("create the Cloud sandbox", () =>
            provider.create({
              // Per (session device, generation): step retries reuse the
              // sandbox; a retry-after-error (new code, so a new request)
              // gets its own key.
              idempotencyKey: `${p.deviceId}-g${p.generation}`,
              machine,
              ttlSeconds,
              env: { ...env },
              ...(template ? { from: template } : {})
            })
          );
          return { id: sandbox.sandboxId, createdAt: sandbox.createdAt ?? Date.now() };
        });
        sandboxId = created.id;
        const current = await step.do("record-sandbox", DO_STEP, () =>
          account.wfSandboxCreated(p.chatId, p.generation, p.provider, created.id, machine, created.createdAt)
        );
        // Superseded (deleted while we created): the delete path owns cleanup,
        // and the daily orphan scan reports anything it could not see.
        if (!current) return;
      }
      const id = sandboxId;
      await ensureRunning(step, provider, id, ttlSeconds, "boot");
      await step.do(
        "install-engine",
        { retries: { limit: 3, delay: "15 seconds", backoff: "exponential" }, timeout: "11 minutes", sensitive: "output" },
        async () => {
          const result = await providerCall("install the Cloud engine", () =>
            provider.exec(id, { command: installCommand(env), timeoutSeconds: 600 })
          );
          if (result.exitCode !== 0) {
            // The script is idempotent, so a retry is safe even if the
            // provider lost track of a run that is still going.
            throw new Error(
              `Cloud engine install failed (exit ${result.exitCode ?? "?"}${result.timedOut ? ", timed out" : ""}): ${tail(result.stderr || result.stdout)}`
            );
          }
          return { installed: true };
        }
      );
      if (!(await awaitSignal(step, "enrolled", "", () => account.wfEnrolled(p.chatId, p.generation), "10 minutes"))) {
        throw new NonRetryableError("The Cloud engine didn't come online (no enrollment within 10 minutes).");
      }
      await step.do("mark-ready", DO_STEP, () => account.wfReady(p.chatId, p.generation));
    } catch (e) {
      await step.do("mark-error", DO_STEP, () => account.wfFailed(p.chatId, p.generation, failureMessage(e)));
    }
  }
}

/**
 * WakeWorkflow — resume the sandbox and wait for the engine's first contact.
 * Enabled system units come back on resume; if the engine stays silent, one
 * idempotent `systemctl restart` before giving up.
 */
export class WakeWorkflow extends WorkflowEntrypoint<Env, LifecycleParams> {
  async run(event: WorkflowEvent<LifecycleParams>, step: WorkflowStep): Promise<void> {
    const p = event.payload;
    const account = accountStub(this.env, p);
    try {
      const provider = requireProvider(this.env, p.provider);
      await ensureRunning(step, provider, p.sandboxId, ttlFor(this.env, provider), "wake");
      const online = () => account.wfOnline(p.chatId, p.generation);
      let up = await awaitSignal(step, "online", "", online, "5 minutes");
      if (!up) {
        await step.do(
          "restart-engine",
          { retries: { limit: 2, delay: "10 seconds", backoff: "constant" }, timeout: "2 minutes" },
          async () => {
            const result = await providerCall("restart the Cloud engine", () =>
              provider.exec(p.sandboxId, { command: RESTART_ENGINE_COMMAND, timeoutSeconds: 60 })
            );
            return { exitCode: result.exitCode };
          }
        );
        up = await awaitSignal(step, "online", "-2", online, "5 minutes");
      }
      if (!up) {
        throw new NonRetryableError("The Cloud engine didn't come back after waking (no contact within 10 minutes).");
      }
      await step.do("mark-ready", DO_STEP, () => account.wfReady(p.chatId, p.generation));
    } catch (e) {
      await step.do("mark-error", DO_STEP, () => account.wfFailed(p.chatId, p.generation, failureMessage(e)));
    }
  }
}

/** SleepWorkflow — stop the session's sandbox; the session reads `sleeping`. */
export class SleepWorkflow extends WorkflowEntrypoint<Env, LifecycleParams> {
  async run(event: WorkflowEvent<LifecycleParams>, step: WorkflowStep): Promise<void> {
    const p = event.payload;
    const account = accountStub(this.env, p);
    try {
      const provider = requireProvider(this.env, p.provider);
      if ((await stopSandbox(step, provider, p.sandboxId, "put Cloud to sleep")) === "error") {
        throw new NonRetryableError("The Cloud sandbox is in an error state.");
      }
      await step.do("mark-sleeping", DO_STEP, () => account.wfStopped(p.chatId, p.generation));
    } catch (e) {
      await step.do("mark-error", DO_STEP, () => account.wfFailed(p.chatId, p.generation, failureMessage(e)));
    }
  }
}

/**
 * DeleteWorkflow — explicit user delete, exact to the second for billing:
 * stop → stopped → final usage read with `running: false` (re-read after a
 * short sleep while the meter settles) → rows persisted and closed → only
 * then delete (providers stop serving usage for deleted sandboxes). If the
 * stop is refused for good, nothing is deleted: the device shows the error
 * and a retried delete resumes.
 */
export class DeleteWorkflow extends WorkflowEntrypoint<Env, LifecycleParams> {
  async run(event: WorkflowEvent<LifecycleParams>, step: WorkflowStep): Promise<void> {
    const p = event.payload;
    const account = accountStub(this.env, p);
    const ref = { provider: p.provider, sandboxId: p.sandboxId };
    try {
      const provider = requireProvider(this.env, p.provider);
      if ((await readState(step, provider, p.sandboxId, "initial-state")) === "deleted") {
        // Already gone on the provider's side: record what can be recorded.
        await step.do("final-usage-gone", DO_STEP, () => account.wfCaptureFinalUsage(p.chatId, p.generation, ref, true));
        await step.do("mark-deleted", DO_STEP, () => account.wfDeleted(p.chatId, p.generation, ref));
        return;
      }
      // A sandbox in the provider's error state cannot be snapshotted; its
      // meter is not moving either, so it goes straight to the final read.
      await stopSandbox(step, provider, p.sandboxId, "stop Cloud before deleting it");
      for (let attempt = 1; attempt <= 6; attempt++) {
        const outcome = await step.do(`final-usage-${attempt}`, { ...DO_STEP, timeout: "2 minutes" }, () =>
          account.wfCaptureFinalUsage(p.chatId, p.generation, ref, attempt === 6)
        );
        if (outcome === "stale") return;
        if (outcome === "final" || attempt === 6) break;
        await step.sleep(`final-usage-wait-${attempt}`, "10 seconds");
      }
      const deleted = await step.do("delete-sandbox", PROVIDER_STEP, async () => {
        try {
          return await provider.delete(p.sandboxId);
        } catch (e) {
          if (e instanceof SandboxError && e.kind === "notFound") return {}; // already gone
          const failure = classifyFailure(e, "delete the Cloud sandbox");
          throw failure.retryable ? new Error(failure.message) : new NonRetryableError(failure.message);
        }
      });
      await step.do("mark-deleted", DO_STEP, () => account.wfDeleted(p.chatId, p.generation, ref, deleted.operationId));
    } catch (e) {
      await step.do("mark-error", DO_STEP, () => account.wfDeleteFailed(p.chatId, p.generation, failureMessage(e)));
    }
  }
}
