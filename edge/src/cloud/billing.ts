/**
 * Cross-user Cloud billing (metering.ts has the per-user half):
 *
 * - `runCloudCron` (hourly `scheduled`, `17 * * * *`): walk the CloudIndex
 *   and poke every CloudAccount to reconcile against its providers' meters —
 *   a backstop for the DOs' own hourly alarms. Once a day (the 00:17 UTC
 *   run) also list every sandbox on every configured provider and flag any
 *   that no user owns as an `orphan` (billed to us; usually a crashed
 *   provision). When a month is over and every
 *   user's rows for it are closed, write the month's export to R2 at
 *   `usage/{YYYY-MM}.json` (once; re-writing is an idempotent overwrite).
 *
 * - `GET /admin/cloud/usage?month=YYYY-MM[&format=csv]`: the operator
 *   export, guarded by the `ADMIN_TOKEN` secret (constant-time compare; the
 *   route does not exist — 404 — when the secret is unset). Routed before
 *   the user-bearer gate: the admin token is not a JWT.
 */
import type { Env } from "../env";
import { json, jsonError } from "../http";
import type { CloudBillingRow } from "./cloud-account";
import { cloudIndexStub, type CloudOrphan } from "./cloud-index";
import { constantTimeEqual, sha256B64url } from "./crypto";
import {
  isMonthKey,
  monthClosable,
  monthEnd,
  monthKey,
  monthStart,
  previousMonth,
  roundDollars
} from "./metering";
import { cloudAccountName } from "./policy";
import { configuredProviders, type ProviderEnv } from "./providers";
import type { ListedSandbox } from "./sandbox-provider";

type BillingEnv = Pick<Env, "CLOUD_ACCOUNTS" | "CLOUD_INDEX" | "BLOBS" | "ADMIN_TOKEN"> & ProviderEnv;

export interface UsageExport {
  readonly month: string;
  readonly generatedAt: number;
  /** The month is over and every user's figure is final. */
  readonly closed: boolean;
  readonly totals: {
    readonly seconds: number;
    readonly dollars: number;
    readonly users: number;
    readonly orphans: number;
  };
  readonly users: readonly CloudBillingRow[];
  readonly orphans: readonly CloudOrphan[];
}

/** A sandbox younger than this may simply not be registered yet (the
 * create → register window), so it is not an orphan. */
const ORPHAN_MIN_AGE_MS = 60 * 60_000;
const FANOUT = 20;

const mapLimit = async <T, R>(items: readonly T[], limit: number, run: (item: T) => Promise<R>): Promise<R[]> => {
  const out: R[] = new Array(items.length);
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await run(items[i]!);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return out;
};

const deviceStub = (env: BillingEnv, orgId: string, userId: string) => {
  const ns = env.CLOUD_ACCOUNTS!;
  return ns.get(ns.idFromName(cloudAccountName(orgId, userId)));
};

export const buildUsageExport = async (env: BillingEnv, month: string, now: number): Promise<UsageExport> => {
  const index = cloudIndexStub(env);
  const entries = index && env.CLOUD_ACCOUNTS ? await index.entries() : [];
  const rows = await mapLimit(entries, FANOUT, async (entry): Promise<CloudBillingRow> => {
    const row = await deviceStub(env, entry.orgId, entry.userId).billingUsage(month, now);
    // A deleted device's DO has forgotten its id; the index has not.
    return { ...row, orgId: entry.orgId, userId: entry.userId, deviceId: row.deviceId ?? entry.deviceId };
  });
  const users = rows.filter((row) => row.sandboxes.length > 0);
  const start = monthStart(month);
  const end = monthEnd(month);
  const orphans = (index ? await index.orphans() : []).filter(
    (o) => o.firstSeenAt < end && o.lastSeenAt >= start
  );
  return {
    month,
    generatedAt: now,
    closed: monthClosable(month, now) && users.every((u) => u.closed),
    totals: {
      seconds: users.reduce((sum, u) => sum + u.seconds, 0),
      dollars: roundDollars(users.reduce((sum, u) => sum + u.dollars, 0)),
      users: users.length,
      orphans: orphans.length
    },
    users,
    orphans
  };
};

const csvField = (value: unknown): string => {
  const text = value === undefined || value === null ? "" : String(value);
  return /[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
};

/** One line per (user, sandbox), then one per orphan. */
export const usageCsv = (data: UsageExport): string => {
  const header = [
    "kind", "month", "orgId", "userId", "deviceId", "provider", "sandboxId", "sandboxType",
    "seconds", "dollars", "running", "reconciledAt", "closed", "note"
  ];
  const lines = [header.join(",")];
  for (const user of data.users) {
    for (const s of user.sandboxes) {
      lines.push(
        [
          "user", data.month, user.orgId, user.userId, user.deviceId, s.provider, s.sandboxId, s.sandboxType,
          s.seconds, s.dollars, s.running, new Date(s.reconciledAt).toISOString(), user.closed,
          user.errors.filter((e) => e.startsWith(`${s.provider}/${s.sandboxId}:`)).join("; ")
        ].map(csvField).join(",")
      );
    }
  }
  for (const o of data.orphans) {
    lines.push(
      ["orphan", data.month, "", "", "", o.provider, o.sandboxId, "", "", "", "", "", "", `state=${o.state}`]
        .map(csvField)
        .join(",")
    );
  }
  return `${lines.join("\n")}\n`;
};

/** `/admin/cloud/usage` — undefined when not this route. */
export const handleAdminRoute = async (
  request: Request,
  env: BillingEnv,
  url: URL
): Promise<Response | undefined> => {
  if (url.pathname !== "/admin/cloud/usage") return undefined;
  if (!env.ADMIN_TOKEN) return json({ error: "not_found" }, 404);
  // Header only (never `?token=`: query strings end up in logs).
  const header = request.headers.get("authorization") ?? "";
  const token = header.toLowerCase().startsWith("bearer ") ? header.slice(7).trim() : "";
  // Compare digests so neither the content nor the length leaks via timing.
  const [given, expected] = await Promise.all([sha256B64url(token), sha256B64url(env.ADMIN_TOKEN)]);
  if (!token || !constantTimeEqual(given, expected)) {
    return jsonError(401, "unauthenticated", "Admin token required.");
  }
  if (request.method !== "GET") return jsonError(405, "method_not_allowed", "GET only.");
  const month = url.searchParams.get("month") ?? monthKey(Date.now());
  if (!isMonthKey(month)) return jsonError(400, "bad_request", "month must be YYYY-MM.");
  const data = await buildUsageExport(env, month, Date.now());
  if (url.searchParams.get("format") === "csv") {
    return new Response(usageCsv(data), {
      headers: {
        "content-type": "text/csv; charset=utf-8",
        "content-disposition": `attachment; filename="zeron-cloud-usage-${month}.csv"`
      }
    });
  }
  return json(data);
};

/** A provider's sandboxes that no user's CloudAccount ever registered. */
export const findOrphans = (
  sandboxes: readonly ListedSandbox[],
  known: ReadonlySet<string>,
  now: number
): { sandboxId: string; state: string }[] =>
  sandboxes
    .filter((s) => !known.has(s.sandboxId) && s.state !== "deleted")
    .filter((s) => s.createdAt === undefined || now - s.createdAt >= ORPHAN_MIN_AGE_MS)
    .map((s) => ({ sandboxId: s.sandboxId, state: s.state }));

/** Daily: every configured provider's sandboxes vs. the index. Returns
 * `provider/sandboxId` of every orphan found this scan. */
export const scanOrphans = async (env: BillingEnv, now: number): Promise<string[]> => {
  const index = cloudIndexStub(env);
  if (!index) return [];
  const found: { provider: string; sandboxId: string; state: string }[] = [];
  for (const provider of configuredProviders(env)) {
    const listed: ListedSandbox[] = [];
    for await (const sandbox of provider.list()) listed.push(sandbox);
    const known = new Set(await index.knownSandboxes(provider.name));
    found.push(...findOrphans(listed, known, now).map((o) => ({ provider: provider.name, ...o })));
  }
  await index.recordOrphans(found, now);
  for (const orphan of found) {
    console.log(JSON.stringify({ event: "cloud.orphan", provider: orphan.provider, sandboxId: orphan.sandboxId, state: orphan.state }));
  }
  return found.map((o) => `${o.provider}/${o.sandboxId}`);
};

/** Write `usage/{month}.json` once every figure in it is final. Idempotent:
 * a month already exported is skipped (re-writing would be an identical
 * overwrite anyway). Returns whether this call wrote it. */
export const exportMonth = async (env: BillingEnv, month: string, now: number): Promise<boolean> => {
  const index = cloudIndexStub(env);
  if (!index || !monthClosable(month, now) || (await index.exportedAt(month)) !== undefined) return false;
  const data = await buildUsageExport(env, month, now);
  if (!data.closed) return false;
  await env.BLOBS.put(`usage/${month}.json`, JSON.stringify(data), {
    httpMetadata: { contentType: "application/json" }
  });
  await index.markExported(month, now);
  return true;
};

/** The last three ended months: the one that just closed, plus stragglers
 * whose rows closed late (a reconcile that failed around month end). */
export const writeClosedExports = async (env: BillingEnv, now: number): Promise<string[]> => {
  const written: string[] = [];
  let month = previousMonth(monthKey(now));
  for (let i = 0; i < 3; i++, month = previousMonth(month)) {
    if (await exportMonth(env, month, now)) written.push(month);
  }
  return written;
};

export const runCloudCron = async (env: BillingEnv, scheduledTime: number): Promise<void> => {
  const index = cloudIndexStub(env);
  if (!index || !env.CLOUD_ACCOUNTS) return;
  const entries = await index.entries();
  await mapLimit(entries, FANOUT, async (entry) => {
    try {
      await deviceStub(env, entry.orgId, entry.userId).reconcile();
    } catch (e) {
      console.warn("cloud cron: reconcile failed", entry.orgId, entry.userId, String(e));
    }
  });
  if (new Date(scheduledTime).getUTCHours() === 0) {
    try {
      await scanOrphans(env, scheduledTime);
    } catch (e) {
      console.warn("cloud cron: orphan scan failed", String(e));
    }
  }
  try {
    await writeClosedExports(env, scheduledTime);
  } catch (e) {
    console.warn("cloud cron: export failed", String(e));
  }
};
