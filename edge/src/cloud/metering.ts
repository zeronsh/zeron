/**
 * Cloud metering: what each user's sandboxes cost, reconciled against the
 * sandbox provider's own meter (`SandboxProvider.usage`), which is the
 * authoritative figure — billable seconds with the machine-size multiplier
 * applied, never counting stopped time, split pro rata when a window edge
 * falls inside a running stretch. We never derive usage from our own lifecycle timestamps; those
 * only decide WHICH windows to ask the provider about.
 *
 * Every sandbox reference is `{provider, sandboxId}`: a user may hold
 * sandboxes on two providers during a migration, and each is metered by the
 * provider that runs it.
 *
 * Per CloudAccount DO (SQLite):
 *   sandboxes — every sandbox the user ever had (delete + re-enable makes more)
 *   ledger    — append-only lifecycle events (create, ready, wake, sleep, stop,
 *               delete, ttl_extend, error), for disputes and debugging
 *   usage_days — one row per (UTC day, sandbox): the last reconciled figure.
 *               `closed` rows are final and never change again. A month is
 *               the sum of its days (the operator export).
 *   credit_grants — credits added to the account (starting grant, operator
 *               grants). The balance is grants minus credits used.
 *
 * Day D is reconciled with since = D start, until = min(now, D end). Once D
 * has ended (plus a grace hour for the provider's meter to settle) one more
 * reconcile with until = D end closes the row. A deleted sandbox's figure is
 * read after it stopped and before the DELETE (providers stop serving usage
 * for deleted sandboxes), and its rows are closed right then.
 *
 * Credits: one credit is one minute of a small machine — 60 of the
 * provider's billable seconds, which already carry the machine-size
 * multiplier.
 *
 * The planning and month math are pure (unit-tested in Node); the SQL
 * helpers take the DO's `SqlStorage`.
 */

export const CLOSE_GRACE_MS = 60 * 60_000;
/** Reconcile at least this often while anything is open. */
export const RECONCILE_EVERY_MS = 60 * 60_000;

const MONTH_RE = /^(\d{4})-(0[1-9]|1[0-2])$/;
const DAY_RE = /^(\d{4})-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$/;
export const DAY_MS = 24 * 60 * 60_000;

/** Billable seconds per credit by default: a minute of a machine the
 * provider bills at 1×. `CLOUD_CREDIT_SECONDS` overrides it, so a credit is
 * a minute of the default machine whatever its provider multiplier. */
export const CREDIT_SECONDS = 60;

/** `CLOUD_CREDIT_SECONDS`, when a positive number; else [`CREDIT_SECONDS`]. */
export const creditSeconds = (configured: string | undefined): number => {
  const value = Number(configured);
  return Number.isFinite(value) && value > 0 ? value : CREDIT_SECONDS;
};

export const creditsOf = (seconds: number, perCredit = CREDIT_SECONDS): number =>
  Math.round((seconds / perCredit) * 100) / 100;

export const isDayKey = (value: string): boolean => DAY_RE.test(value) && dayKey(Date.parse(`${value}T00:00:00Z`)) === value;

/** `YYYY-MM-DD` (UTC) of a Unix-ms instant. */
export const dayKey = (ms: number): string => new Date(ms).toISOString().slice(0, 10);

export const dayStart = (day: string): number => {
  if (!DAY_RE.test(day)) throw new Error(`bad day ${day}`);
  return Date.parse(`${day}T00:00:00Z`);
};

/** Exclusive end: the first instant of the next day. */
export const dayEnd = (day: string): number => dayStart(day) + DAY_MS;

/** Days `[from, to]` inclusive, oldest first. */
export const daysBetween = (from: number, to: number): string[] => {
  const out: string[] = [];
  const last = dayKey(Math.max(from, to));
  for (let d = dayKey(from); ; d = dayKey(dayEnd(d))) {
    out.push(d);
    if (d === last) return out;
  }
};

/** Days whose figures are final once reconciled after they end. */
export const dayClosable = (day: string, now: number, graceMs = CLOSE_GRACE_MS): boolean =>
  now >= dayEnd(day) + graceMs;

export const isMonthKey = (value: string): boolean => MONTH_RE.test(value);

/** `YYYY-MM` (UTC) of a Unix-ms instant. */
export const monthKey = (ms: number): string => new Date(ms).toISOString().slice(0, 7);

export const monthStart = (month: string): number => {
  const m = MONTH_RE.exec(month);
  if (!m) throw new Error(`bad month ${month}`);
  return Date.UTC(Number(m[1]), Number(m[2]) - 1, 1);
};

/** Exclusive end: the first instant of the next month. */
export const monthEnd = (month: string): number => {
  const m = MONTH_RE.exec(month);
  if (!m) throw new Error(`bad month ${month}`);
  return Date.UTC(Number(m[1]), Number(m[2]), 1);
};

export const nextMonth = (month: string): string => monthKey(monthEnd(month));

export const previousMonth = (month: string): string => monthKey(monthStart(month) - 1);

/** Months whose figures are final once reconciled after they end. */
export const monthClosable = (month: string, now: number, graceMs = CLOSE_GRACE_MS): boolean =>
  now >= monthEnd(month) + graceMs;

export interface SandboxRecord {
  readonly provider: string;
  readonly sandboxId: string;
  /** The session (chat) the sandbox ran. */
  readonly chatId?: string;
  readonly type: string;
  readonly createdAt: number;
  readonly deletedAt?: number;
  /** When the pre-delete final usage capture succeeded. */
  readonly finalAt?: number;
}

export interface UsageRow {
  /** UTC day, `YYYY-MM-DD`. */
  readonly day: string;
  readonly provider: string;
  readonly sandboxId: string;
  readonly sandboxType: string;
  readonly seconds: number;
  readonly dollars: number;
  readonly running: boolean;
  /** Unix ms of the reconcile that produced the figures (0 = never). */
  readonly reconciledAt: number;
  readonly closed: boolean;
  readonly reconcileError?: string;
}

export interface ReconcileTask {
  readonly day: string;
  readonly provider: string;
  readonly sandboxId: string;
  readonly sandboxType: string;
  /** Unix ms window `[since, until)` to ask the provider about. */
  readonly since: number;
  readonly until: number;
  /** Ask the provider (false = the figure is already final; only close the row). */
  readonly fetch: boolean;
  /** Mark the row closed with this reconcile. */
  readonly close: boolean;
  readonly deleted: boolean;
}

const rowKey = (day: string, provider: string, sandboxId: string) =>
  `${day}\u0000${provider}\u0000${sandboxId}`;

/** Months `[from, to]` inclusive, oldest first. */
export const monthsBetween = (from: number, to: number): string[] => {
  const out: string[] = [];
  const last = monthKey(Math.max(from, to));
  for (let m = monthKey(from); ; m = nextMonth(m)) {
    out.push(m);
    if (m === last) return out;
  }
};

/**
 * Every (day, sandbox) window that still needs the provider's figure, or just a
 * close. Closed rows are skipped for good. Pure: `now` is a parameter so the
 * day-boundary math is testable.
 */
export const planReconcile = (
  sandboxes: readonly SandboxRecord[],
  rows: readonly UsageRow[],
  now: number,
  graceMs = CLOSE_GRACE_MS
): ReconcileTask[] => {
  const byKey = new Map(rows.map((row) => [rowKey(row.day, row.provider, row.sandboxId), row]));
  const tasks: ReconcileTask[] = [];
  for (const sandbox of sandboxes) {
    const lastAt = Math.min(now, sandbox.deletedAt ?? now);
    for (const day of daysBetween(sandbox.createdAt, lastAt)) {
      const row = byKey.get(rowKey(day, sandbox.provider, sandbox.sandboxId));
      if (row?.closed) continue;
      const since = dayStart(day);
      const until = Math.min(now, dayEnd(day), sandbox.deletedAt ?? Infinity);
      const close = dayClosable(day, now, graceMs);
      const deleted = sandbox.deletedAt !== undefined;
      const final =
        sandbox.finalAt !== undefined && row !== undefined && row.reconciledAt >= sandbox.finalAt;
      if (final) {
        if (close) {
          tasks.push({ day, provider: sandbox.provider, sandboxId: sandbox.sandboxId, sandboxType: row.sandboxType, since, until, fetch: false, close, deleted });
        }
        continue;
      }
      if (until <= since) continue;
      tasks.push({ day, provider: sandbox.provider, sandboxId: sandbox.sandboxId, sandboxType: row?.sandboxType ?? sandbox.type, since, until, fetch: true, close, deleted });
    }
  }
  return tasks;
};

/** Whether anything still needs a periodic reconcile. */
export const meteringOpen = (
  sandboxes: readonly SandboxRecord[],
  rows: readonly UsageRow[],
  now: number
): boolean =>
  sandboxes.some((s) => s.deletedAt === undefined) || planReconcile(sandboxes, rows, now).length > 0;

export interface CloudSandboxUsage {
  readonly provider: string;
  readonly sandboxId: string;
  readonly chatId?: string;
  readonly sandboxType: string;
  readonly seconds: number;
  readonly dollars: number;
  readonly running: boolean;
  readonly reconciledAt: number;
}

/** One user's month (the operator export). */
export interface MonthUsage {
  readonly month: string;
  readonly seconds: number;
  readonly dollars: number;
  readonly sandboxes: readonly CloudSandboxUsage[];
  readonly closed: boolean;
  readonly available: true;
}

export const roundDollars = (value: number): number => Math.round(value * 1e6) / 1e6;

/**
 * One user's month. `closed` = the month is over AND every sandbox that
 * existed in it has a closed row — the billable figure.
 */
export const summarizeMonth = (
  month: string,
  sandboxes: readonly SandboxRecord[],
  rows: readonly UsageRow[],
  now: number
): MonthUsage & { readonly errors: readonly string[] } => {
  const inMonth = rows.filter((row) => row.day.startsWith(`${month}-`));
  const start = monthStart(month);
  const end = monthEnd(month);
  // Closed once the month (plus grace) is over and none of its days is
  // still open for any sandbox that existed in it.
  const pending = planReconcile(sandboxes, rows, Math.min(now, end + CLOSE_GRACE_MS)).filter(
    (task) => task.day.startsWith(`${month}-`) && task.since < end && task.until > start
  );
  const closed = monthClosable(month, now) && pending.length === 0 && inMonth.every((row) => row.closed);
  const chatOf = new Map(sandboxes.map((s) => [`${s.provider}/${s.sandboxId}`, s.chatId]));
  // One figure per sandbox: the sum of its days.
  const perSandbox = new Map<string, CloudSandboxUsage>();
  for (const row of inMonth) {
    const key = `${row.provider}/${row.sandboxId}`;
    const prev = perSandbox.get(key);
    const chatId = chatOf.get(key);
    perSandbox.set(key, {
      provider: row.provider,
      sandboxId: row.sandboxId,
      ...(chatId ? { chatId } : {}),
      sandboxType: row.sandboxType,
      seconds: (prev?.seconds ?? 0) + row.seconds,
      dollars: roundDollars((prev?.dollars ?? 0) + row.dollars),
      running: (prev?.running ?? false) || row.running,
      reconciledAt: Math.max(prev?.reconciledAt ?? 0, row.reconciledAt)
    });
  }
  return {
    month,
    seconds: inMonth.reduce((sum, row) => sum + row.seconds, 0),
    dollars: roundDollars(inMonth.reduce((sum, row) => sum + row.dollars, 0)),
    sandboxes: [...perSandbox.values()],
    closed,
    available: true,
    errors: inMonth.flatMap((row) =>
      row.reconcileError ? [`${row.provider}/${row.sandboxId} ${row.day}: ${row.reconcileError}`] : []
    )
  };
};

/** Credits used per day over `[from, to]` (UTC days, inclusive): every day
 * present, zero when nothing ran. */
export const creditsPerDay = (
  from: string,
  to: string,
  rows: readonly UsageRow[],
  perCredit = CREDIT_SECONDS
): { readonly day: string; readonly credits: number }[] => {
  const seconds = new Map<string, number>();
  for (const row of rows) seconds.set(row.day, (seconds.get(row.day) ?? 0) + row.seconds);
  return daysBetween(dayStart(from), dayStart(to)).map((day) => ({
    day,
    credits: creditsOf(seconds.get(day) ?? 0, perCredit)
  }));
};

/** Wire shape of `CloudUsage` (crates/proto/src/cloud.rs): the caller's
 * credits — balance, and use per day over a range. */
export interface CloudUsage {
  readonly from: string;
  readonly to: string;
  readonly days: readonly { readonly day: string; readonly credits: number }[];
  /** Credits used in the range. */
  readonly credits: number;
  /** Credits left (may be negative: machines that ran past zero). */
  readonly balance: number;
  readonly available: true;
}

// ── SQLite (CloudAccount DO) ─────────────────────────────────────────────────

export type LedgerEvent =
  | "create"
  | "ready"
  | "wake"
  | "sleep"
  | "stop"
  | "delete"
  | "ttl_extend"
  | "error";

export interface LedgerEntry {
  readonly at: number;
  readonly event: LedgerEvent;
  readonly chatId?: string;
  readonly provider?: string;
  readonly sandboxId?: string;
  readonly type?: string;
  readonly workflowId?: string;
  readonly detail?: string;
}

export const ensureMeteringTables = (sql: SqlStorage): void => {
  sql.exec(`CREATE TABLE IF NOT EXISTS sandboxes (
    provider TEXT NOT NULL, sandbox_id TEXT NOT NULL, chat_id TEXT, type TEXT NOT NULL,
    created_at INTEGER NOT NULL, deleted_at INTEGER, final_at INTEGER,
    PRIMARY KEY (provider, sandbox_id))`);
  sql.exec(`CREATE TABLE IF NOT EXISTS ledger (
    seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, event TEXT NOT NULL,
    chat_id TEXT, provider TEXT, sandbox_id TEXT, type TEXT, workflow_id TEXT, detail TEXT)`);
  sql.exec(`CREATE TABLE IF NOT EXISTS usage_days (
    day TEXT NOT NULL, provider TEXT NOT NULL, sandbox_id TEXT NOT NULL, sandbox_type TEXT NOT NULL,
    seconds INTEGER NOT NULL, dollars REAL NOT NULL, running INTEGER NOT NULL,
    reconciled_at INTEGER NOT NULL, closed INTEGER NOT NULL DEFAULT 0, reconcile_error TEXT,
    PRIMARY KEY (day, provider, sandbox_id))`);
  sql.exec(`CREATE TABLE IF NOT EXISTS credit_grants (
    seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, credits REAL NOT NULL, reason TEXT NOT NULL)`);
};

export interface CreditGrant {
  readonly at: number;
  readonly credits: number;
  readonly reason: string;
}

export const addCredits = (sql: SqlStorage, grant: CreditGrant): void => {
  sql.exec("INSERT INTO credit_grants (at, credits, reason) VALUES (?, ?, ?)", grant.at, grant.credits, grant.reason.slice(0, 200));
};

/** The reason the one-time starting grant is recorded under. */
export const STARTING_CREDITS = "starting credits";

export const hasGrant = (sql: SqlStorage, reason: string): boolean =>
  [...sql.exec("SELECT 1 FROM credit_grants WHERE reason = ? LIMIT 1", reason)].length > 0;

export const creditsGranted = (sql: SqlStorage): number =>
  ([...sql.exec("SELECT COALESCE(SUM(credits), 0) AS total FROM credit_grants")][0]?.total as number) ?? 0;

/** One paid plan period: its credits, spendable until it ends. */
export interface CreditPeriod {
  /** The invoice that paid for it (one row per invoice). */
  readonly invoice: string;
  readonly plan: string;
  /** Unix ms, `[start, end)`. */
  readonly start: number;
  readonly end: number;
  readonly credits: number;
}

export interface CreditBreakdown {
  /** Plan credits left in the current period (0 without one). */
  readonly monthly: number;
  /** The current period's plan credits. */
  readonly monthlyTotal: number;
  /** When they expire (the current period's end). */
  readonly monthlyEndsAt?: number;
  /** Credits that never expire (bought, granted), after what they covered. */
  readonly extra: number;
}

/**
 * Where the balance stands. Each day's use is charged to the plan period
 * that day falls in (the latest-starting one, after a mid-period plan
 * change) up to its credits; whatever a period doesn't cover — and all use
 * outside a period — comes out of the credits that never expire. Unused
 * plan credits simply lapse with their period. Pure.
 */
export const creditBreakdown = (
  extraGranted: number,
  periods: readonly CreditPeriod[],
  rows: readonly UsageRow[],
  now: number,
  perCredit = CREDIT_SECONDS
): CreditBreakdown => {
  const byStart = [...periods].sort((a, b) => a.start - b.start);
  const used = new Map<CreditPeriod, number>();
  let uncovered = 0;
  for (const row of rows) {
    const credits = creditsOf(row.seconds, perCredit);
    if (credits === 0) continue;
    let period: CreditPeriod | undefined;
    for (const p of byStart) {
      if (dayKey(p.start) <= row.day && row.day < dayKey(p.end)) period = p;
    }
    if (period) used.set(period, (used.get(period) ?? 0) + credits);
    else uncovered += credits;
  }
  for (const [period, credits] of used) uncovered += Math.max(0, credits - period.credits);
  let current: CreditPeriod | undefined;
  for (const p of byStart) if (p.start <= now && now < p.end) current = p;
  const round = (value: number) => Math.round(value * 100) / 100;
  return {
    monthly: current ? round(Math.max(0, current.credits - (used.get(current) ?? 0))) : 0,
    monthlyTotal: current?.credits ?? 0,
    ...(current ? { monthlyEndsAt: current.end } : {}),
    extra: round(extraGranted - uncovered)
  };
};

export const ensureBillingTables = (sql: SqlStorage): void => {
  sql.exec(`CREATE TABLE IF NOT EXISTS credit_periods (
    invoice TEXT PRIMARY KEY, plan TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL, credits REAL NOT NULL)`);
  sql.exec("CREATE TABLE IF NOT EXISTS billing_events (id TEXT PRIMARY KEY, at INTEGER NOT NULL)");
  sql.exec("CREATE TABLE IF NOT EXISTS billing_state (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
};

export const upsertPeriod = (sql: SqlStorage, period: CreditPeriod): void => {
  sql.exec(
    `INSERT INTO credit_periods (invoice, plan, start, end, credits) VALUES (?, ?, ?, ?, ?)
     ON CONFLICT (invoice) DO UPDATE SET plan = excluded.plan, start = excluded.start, end = excluded.end, credits = excluded.credits`,
    period.invoice,
    period.plan,
    period.start,
    period.end,
    period.credits
  );
};

export const listPeriods = (sql: SqlStorage): CreditPeriod[] =>
  [...sql.exec("SELECT * FROM credit_periods ORDER BY start")].map((row) => ({
    invoice: row.invoice as string,
    plan: row.plan as string,
    start: row.start as number,
    end: row.end as number,
    credits: row.credits as number
  }));

/** Record a webhook event id; `false` when it was already handled. */
export const firstSeen = (sql: SqlStorage, id: string, at: number): boolean => {
  if ([...sql.exec("SELECT 1 FROM billing_events WHERE id = ?", id)].length > 0) return false;
  sql.exec("INSERT INTO billing_events (id, at) VALUES (?, ?)", id, at);
  return true;
};

export const readBillingState = <T>(sql: SqlStorage, key: string): T | undefined => {
  const row = [...sql.exec("SELECT value FROM billing_state WHERE key = ?", key)][0];
  return row ? (JSON.parse(row.value as string) as T) : undefined;
};

export const writeBillingState = (sql: SqlStorage, key: string, value: unknown): void => {
  sql.exec(
    "INSERT INTO billing_state (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    key,
    JSON.stringify(value)
  );
};

export interface SandboxRef {
  readonly provider: string;
  readonly sandboxId: string;
}

export const recordSandbox = (sql: SqlStorage, sandbox: SandboxRecord): void => {
  sql.exec(
    "INSERT OR IGNORE INTO sandboxes (provider, sandbox_id, chat_id, type, created_at) VALUES (?, ?, ?, ?, ?)",
    sandbox.provider,
    sandbox.sandboxId,
    sandbox.chatId ?? null,
    sandbox.type,
    sandbox.createdAt
  );
};

export const markSandboxFinal = (sql: SqlStorage, ref: SandboxRef, finalAt: number): void => {
  sql.exec("UPDATE sandboxes SET final_at = ? WHERE provider = ? AND sandbox_id = ?", finalAt, ref.provider, ref.sandboxId);
};

export const markSandboxDeleted = (sql: SqlStorage, ref: SandboxRef, deletedAt: number): void => {
  sql.exec(
    "UPDATE sandboxes SET deleted_at = ? WHERE provider = ? AND sandbox_id = ? AND deleted_at IS NULL",
    deletedAt,
    ref.provider,
    ref.sandboxId
  );
};

export const listSandboxes = (sql: SqlStorage): SandboxRecord[] =>
  [...sql.exec("SELECT * FROM sandboxes ORDER BY created_at")].map((row) => ({
    provider: row.provider as string,
    sandboxId: row.sandbox_id as string,
    ...(row.chat_id === null ? {} : { chatId: row.chat_id as string }),
    type: row.type as string,
    createdAt: row.created_at as number,
    ...(row.deleted_at === null ? {} : { deletedAt: row.deleted_at as number }),
    ...(row.final_at === null ? {} : { finalAt: row.final_at as number })
  }));

export const appendLedger = (sql: SqlStorage, entry: LedgerEntry): void => {
  sql.exec(
    "INSERT INTO ledger (at, event, chat_id, provider, sandbox_id, type, workflow_id, detail) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    entry.at,
    entry.event,
    entry.chatId ?? null,
    entry.provider ?? null,
    entry.sandboxId ?? null,
    entry.type ?? null,
    entry.workflowId ?? null,
    entry.detail ?? null
  );
};

export const listLedger = (sql: SqlStorage, limit = 500): LedgerEntry[] =>
  [...sql.exec("SELECT * FROM ledger ORDER BY seq DESC LIMIT ?", limit)].reverse().map((row) => ({
    at: row.at as number,
    event: row.event as LedgerEvent,
    ...(row.chat_id === null ? {} : { chatId: row.chat_id as string }),
    ...(row.provider === null ? {} : { provider: row.provider as string }),
    ...(row.sandbox_id === null ? {} : { sandboxId: row.sandbox_id as string }),
    ...(row.type === null ? {} : { type: row.type as string }),
    ...(row.workflow_id === null ? {} : { workflowId: row.workflow_id as string }),
    ...(row.detail === null ? {} : { detail: row.detail as string })
  }));

/** Write the provider's figure. A closed row is never touched again (the
 * WHERE on the conflict branch), which is what makes closed months immutable. */
export const upsertUsage = (sql: SqlStorage, row: UsageRow): void => {
  sql.exec(
    `INSERT INTO usage_days (day, provider, sandbox_id, sandbox_type, seconds, dollars, running, reconciled_at, closed, reconcile_error)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
     ON CONFLICT (day, provider, sandbox_id) DO UPDATE SET
       sandbox_type = excluded.sandbox_type, seconds = excluded.seconds, dollars = excluded.dollars,
       running = excluded.running, reconciled_at = excluded.reconciled_at, closed = excluded.closed,
       reconcile_error = NULL
     WHERE usage_days.closed = 0`,
    row.day,
    row.provider,
    row.sandboxId,
    row.sandboxType,
    Math.max(0, Math.round(row.seconds)),
    Math.max(0, row.dollars),
    row.running ? 1 : 0,
    row.reconciledAt,
    row.closed ? 1 : 0
  );
};

/** Keep the last good figure, note why this reconcile failed; optionally
 * close (a sandbox the provider no longer knows can never be re-read). */
export const recordUsageFailure = (
  sql: SqlStorage,
  task: Pick<ReconcileTask, "day" | "provider" | "sandboxId" | "sandboxType">,
  message: string,
  close: boolean
): void => {
  sql.exec(
    `INSERT INTO usage_days (day, provider, sandbox_id, sandbox_type, seconds, dollars, running, reconciled_at, closed, reconcile_error)
     VALUES (?, ?, ?, ?, 0, 0, 0, 0, ?, ?)
     ON CONFLICT (day, provider, sandbox_id) DO UPDATE SET
       reconcile_error = excluded.reconcile_error, closed = excluded.closed
     WHERE usage_days.closed = 0`,
    task.day,
    task.provider,
    task.sandboxId,
    task.sandboxType,
    close ? 1 : 0,
    message.slice(0, 500)
  );
};

export const closeUsageRow = (sql: SqlStorage, day: string, ref: SandboxRef): void => {
  sql.exec(
    "UPDATE usage_days SET closed = 1 WHERE day = ? AND provider = ? AND sandbox_id = ? AND closed = 0",
    day,
    ref.provider,
    ref.sandboxId
  );
};

/** Day rows, optionally only days in `[from, to]` (`YYYY-MM-DD`, inclusive;
 * a month's days are `[M-01, M-31]`). */
export const listUsage = (sql: SqlStorage, range?: { readonly from: string; readonly to: string }): UsageRow[] =>
  [
    ...(range === undefined
      ? sql.exec("SELECT * FROM usage_days ORDER BY day, provider, sandbox_id")
      : sql.exec(
          "SELECT * FROM usage_days WHERE day >= ? AND day <= ? ORDER BY day, provider, sandbox_id",
          range.from,
          range.to
        ))
  ].map((row) => ({
    day: row.day as string,
    provider: row.provider as string,
    sandboxId: row.sandbox_id as string,
    sandboxType: row.sandbox_type as string,
    seconds: row.seconds as number,
    dollars: row.dollars as number,
    running: row.running === 1,
    reconciledAt: row.reconciled_at as number,
    closed: row.closed === 1,
    ...(row.reconcile_error === null ? {} : { reconcileError: row.reconcile_error as string })
  }));
