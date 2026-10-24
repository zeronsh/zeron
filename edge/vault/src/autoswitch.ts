/**
 * Auto-switch policy, pure so it is unit-tested here: when to read an
 * account's plan usage again, and which account takes over from a full one.
 * `VaultAccount` runs it before Claude and Codex grants, for users who turned
 * it on (`setAutoSwitch`).
 *
 * Reads are budgeted. Anthropic's usage endpoint serves about 28–30 requests
 * per account per rolling hour, shared by everything that polls the account
 * (the desktop's meters included), and a 429 past it can block the account's
 * reads for up to an hour (measured by claude-swap:
 * github.com/realiti4/claude-swap, src/claude_swap/poll_policy.py). So a
 * reading is reused for at least `READ_MIN_MS` — 20 reads an hour at most,
 * however often grants come — and a 429's Retry-After is honored.
 */
import type { PlanUsed } from "./providers/usage";

/** A reading is reused this long at least, and this long at most below the
 * threshold. */
const READ_MIN_MS = 3 * 60_000;
const READ_MAX_MS = 5 * 60_000;
/** An exhausted account is read this often all the same: quota can come
 * back before its reported reset. */
const READ_FULL_MS = 10 * 60_000;
/** After a failed read, on top of any Retry-After: a retry right at the
 * deadline is refused again. */
const READ_FAILED_MS = 5 * 60_000;
/** A Retry-After is honored up to this, so a bad header can't park an
 * account for good. */
const RETRY_AFTER_MAX_MS = 60 * 60_000;
/** An account needs this many more points of room than the active one to
 * take over, so two nearly full accounts don't flap. */
const SWITCH_MARGIN = 5;
/** The longest a grant waits for a check: far under the engine's 15 s HTTP
 * timeout on grants (crates/engine/src/runner.rs `HTTP_TIMEOUT`). */
export const SWITCH_WAIT_MS = 2_000;

/** One account's usage as a check sees it. */
export interface Candidate {
  readonly slot: string;
  /** Its fullest window, in percent; undefined when it couldn't be read. */
  readonly percent: number | undefined;
}

/** When to read again an account read at `plan` at `now`: in 5 minutes when
 * empty, 3 at `threshold`, 10 when exhausted; never past a window's reset,
 * never sooner than `READ_MIN_MS`. */
export const readAgainAt = (plan: PlanUsed, threshold: number, now: number): number => {
  const fresh =
    plan.percent >= 100
      ? READ_FULL_MS
      : READ_MAX_MS - (READ_MAX_MS - READ_MIN_MS) * Math.min(1, plan.percent / threshold);
  return Math.max(now + READ_MIN_MS, Math.min(now + fresh, plan.resetsAt ?? Infinity));
};

/** When to retry a read that failed at `now`, after the 429's Retry-After
 * if there was one. */
export const retryReadAt = (now: number, retryAfterMs = 0): number =>
  now + Math.min(retryAfterMs, RETRY_AFTER_MAX_MS) + READ_FAILED_MS;

/** The account that takes over from an active one at `percent`: the
 * readable one with the most room, if it has `SWITCH_MARGIN` more. */
export const pickAccount = (
  percent: number,
  candidates: readonly Candidate[]
): { readonly slot: string; readonly percent: number } | undefined => {
  let best: { readonly slot: string; readonly percent: number } | undefined;
  for (const { slot, percent: room } of candidates) {
    if (room !== undefined && (best === undefined || room < best.percent)) best = { slot, percent: room };
  }
  return best !== undefined && percent - best.percent >= SWITCH_MARGIN ? best : undefined;
};
