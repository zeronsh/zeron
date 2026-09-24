import { methods, type EngineClient } from "@zeron/engine-client";
import type {
  AgentAccount,
  AgentAccountsSnapshot,
  AgentLoginPoll,
  AgentLoginStart,
  HarnessId,
} from "@zeron/proto";

/**
 * Harness accounts settings — the web peer of the desktop's
 * settings/accounts.rs page ("Agents" → Accounts; the wire keeps the legacy
 * `Agent*` names, ADR 0005). Usage thresholds, reset wording, provider
 * order, and the force-usage policy are ported verbatim so the meters read
 * identically on both surfaces. The add-account login flows (paste-code and
 * browser-poll) follow the desktop's LoginFlow state machine.
 */

// ── RPC wrappers ────────────────────────────────────────────────────────
// Every mutation replies with the fresh AgentAccountsSnapshot, so the page
// repaints from one round trip. `targetDeviceId` is the device switcher's
// passthrough (accounts.rs:259-265): null (the local device) sends nothing —
// the calls stay direct; an explicit device rides every call the page makes.

function targetParams(targetDeviceId: string | null | undefined): Record<string, string> {
  return targetDeviceId == null ? {} : { targetDeviceId };
}

export function listAgentAccounts(
  client: EngineClient,
  forceUsage: boolean,
  targetDeviceId?: string | null,
): Promise<AgentAccountsSnapshot> {
  return client.call<AgentAccountsSnapshot>(methods.LIST_AGENT_ACCOUNTS, {
    forceUsage,
    ...targetParams(targetDeviceId),
  });
}

export function activateAgentAccount(
  client: EngineClient,
  account: AgentAccount,
  targetDeviceId?: string | null,
): Promise<AgentAccountsSnapshot> {
  // Tolerant param shape (desktop parity): both `id` and `accountId`.
  return client.call<AgentAccountsSnapshot>(methods.ACTIVATE_AGENT_ACCOUNT, {
    id: account.id,
    accountId: account.id,
    harness: account.harness,
    ...targetParams(targetDeviceId),
  });
}

export function forgetAgentAccount(
  client: EngineClient,
  account: AgentAccount,
  targetDeviceId?: string | null,
): Promise<AgentAccountsSnapshot> {
  return client.call<AgentAccountsSnapshot>(methods.FORGET_AGENT_ACCOUNT, {
    id: account.id,
    accountId: account.id,
    harness: account.harness,
    ...targetParams(targetDeviceId),
  });
}

export function startAgentLogin(
  client: EngineClient,
  harness: HarnessId,
  targetDeviceId?: string | null,
): Promise<AgentLoginStart> {
  return client.call<AgentLoginStart>(methods.START_AGENT_LOGIN, {
    harness,
    ...targetParams(targetDeviceId),
  });
}

export function completeAgentLogin(
  client: EngineClient,
  loginId: string,
  code: string,
  targetDeviceId?: string | null,
): Promise<AgentAccountsSnapshot> {
  return client.call<AgentAccountsSnapshot>(methods.COMPLETE_AGENT_LOGIN, {
    loginId,
    code,
    ...targetParams(targetDeviceId),
  });
}

export function pollAgentLoginOnce(
  client: EngineClient,
  loginId: string,
  targetDeviceId?: string | null,
): Promise<AgentLoginPoll> {
  return client.call<AgentLoginPoll>(methods.POLL_AGENT_LOGIN, {
    loginId,
    ...targetParams(targetDeviceId),
  });
}

/** Best-effort; the desktop only debug-logs a failure. */
export async function cancelAgentLogin(
  client: EngineClient,
  loginId: string,
  targetDeviceId?: string | null,
): Promise<void> {
  await client.call(methods.CANCEL_AGENT_LOGIN, { loginId, ...targetParams(targetDeviceId) });
}

// ── Usage meters ────────────────────────────────────────────────────────

export const USAGE_WARN_FRACTION = 0.8;
export const USAGE_CRITICAL_FRACTION = 0.95;

/** Threshold classification of a usage fraction (usage_level). */
export type UsageLevel = "normal" | "warn" | "critical";

export function usageLevel(fraction: number): UsageLevel {
  if (fraction >= USAGE_CRITICAL_FRACTION) {
    return "critical";
  }
  if (fraction >= USAGE_WARN_FRACTION) {
    return "warn";
  }
  return "normal";
}

/**
 * The token behind each level (usage_color) at the meter-fill's own opacity
 * (accounts.rs:694-697): accent at 0.8 for Normal, warning/danger at 0.85 for
 * Warn/Critical — crossing a threshold swaps the bar's color AND lifts its
 * opacity a notch.
 */
export function usageColorVar(level: UsageLevel): string {
  switch (level) {
    case "critical":
      return "color-mix(in srgb, var(--rb-danger) 85%, transparent)";
    case "warn":
      return "color-mix(in srgb, var(--rb-warning) 85%, transparent)";
    default:
      return "color-mix(in srgb, var(--rb-accent) 80%, transparent)";
  }
}

/**
 * Compact absolute reset moment (format_reset): a local clock time when it
 * lands within ~22h, a short weekday within a week, else month + day. The
 * caller sees the "resets " prefix included.
 */
export function formatReset(resetsAt: string | null, now: number, locale: string = "en-US"): string | null {
  if (resetsAt === null) {
    return null;
  }
  const at = Date.parse(resetsAt);
  if (!Number.isFinite(at)) {
    return null;
  }
  const hours = (at - now) / 3_600_000;
  const date = new Date(at);
  if (hours < 22) {
    return `resets ${new Intl.DateTimeFormat(locale, { hour: "numeric", minute: "2-digit" }).format(date)}`;
  }
  if (hours < 24 * 7) {
    return `resets ${new Intl.DateTimeFormat(locale, { weekday: "short" }).format(date)}`;
  }
  return `resets ${new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" }).format(date)}`;
}

// ── Load policy ─────────────────────────────────────────────────────────

/** Why a ListAgentAccounts load is happening (LoadTrigger). */
export type LoadTrigger = "mount" | "retry" | "refresh" | "postLogin" | "postAction";

/**
 * Whether a load asks the engine to probe usage (`forceUsage`). The engine
 * only hits the provider when forced; the visit's first list must force or
 * every first open renders "Usage unavailable" until a manual Refresh.
 */
export function forceUsageFor(trigger: LoadTrigger): boolean {
  switch (trigger) {
    case "mount":
    case "retry":
    case "refresh":
    case "postLogin":
      return true;
    case "postAction":
      return false;
  }
}

// ── Provider sections ───────────────────────────────────────────────────

export interface ProviderDescriptor {
  readonly harness: HarnessId;
  readonly name: string;
  /** CLI command named in the empty-state copy. */
  readonly cli: string;
}

/** The provider cards, in display order (accounts.rs PROVIDERS). */
export const PROVIDERS: readonly ProviderDescriptor[] = [
  { harness: "claude-code", name: "Claude Code", cli: "claude" },
  { harness: "codex", name: "Codex", cli: "codex" },
  { harness: "cursor", name: "Cursor", cli: "cursor-agent" },
];

/**
 * Accounts of one provider, in the engine's order — no active-first
 * re-sort: switching must not move the switched-to card.
 */
export function providerAccounts(snapshot: AgentAccountsSnapshot, harness: HarnessId): AgentAccount[] {
  return snapshot.accounts.filter((account) => account.harness === harness);
}

/** The empty-card copy under a provider with no accounts. */
export function providerEmptyCopy(provider: ProviderDescriptor): string {
  if (provider.harness === "cursor") {
    // Cursor's app login is separate from `cursor-agent login`.
    return `${provider.name} isn't connected on this device — connect it to run Cursor sessions.`;
  }
  return `No ${provider.name} login detected on this device — sign in with “${provider.cli}” or add an account.`;
}

/** The row's primary label (email, else display name, else the fallback). */
export function accountLabel(account: AgentAccount): string {
  return account.email ?? account.displayName ?? "Unknown account";
}

/** The avatar initial: the label's first letter, uppercased. */
export function accountInitial(account: AgentAccount): string {
  const first = accountLabel(account).charAt(0);
  return first.length > 0 ? first.toUpperCase() : "?";
}

/** Fallback line when a row has no usage windows (meters XOR this line). */
export function usageFallback(account: AgentAccount): string {
  return account.switchable ? "Usage unavailable" : "Credentials unavailable";
}

// ── Add-account login flows ─────────────────────────────────────────────

/** Dialog title for a login flow (LoginFlow::title). */
export function loginTitle(harness: HarnessId): string {
  switch (harness) {
    case "codex":
      return "Add Codex account";
    case "cursor":
      return "Connect Cursor";
    default:
      return "Add Claude account";
  }
}

export interface PollLoginOptions {
  readonly intervalMs?: number;
  /** Return false to stop polling (dialog dismissed); the promise resolves null. */
  readonly isActive: () => boolean;
  /** Progress note from a pending poll ("Waiting for the browser…" chain). */
  readonly onPending: (message: string | null) => void;
  /** Injectable for tests. */
  readonly sleep?: (ms: number) => Promise<void>;
}

const DEFAULT_POLL_INTERVAL_MS = 1500;

/**
 * The browser-flow wait loop (spawn_poll): PollAgentLogin every 1.5s until
 * the flow lands (`done`/`error`) or the dialog goes away. A failed poll
 * resolves as an error-status poll so the dialog can show the message.
 */
export async function pollAgentLogin(
  client: EngineClient,
  loginId: string,
  options: PollLoginOptions,
  targetDeviceId?: string | null,
): Promise<AgentLoginPoll | null> {
  const intervalMs = options.intervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  while (options.isActive()) {
    await sleep(intervalMs);
    if (!options.isActive()) {
      return null;
    }
    let poll: AgentLoginPoll;
    try {
      poll = await pollAgentLoginOnce(client, loginId, targetDeviceId);
    } catch (error) {
      return { status: "error", message: `Poll failed: ${error instanceof Error ? error.message : String(error)}` };
    }
    switch (poll.status) {
      case "done":
      case "error":
        return poll;
      default:
        options.onPending(poll.message ?? null);
    }
  }
  return null;
}
