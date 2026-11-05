/**
 * CloudIndex — ONE Durable Object (`index1`) listing every user who ever had
 * a Cloud device, so billing can walk them without scanning DO storage.
 *
 * CloudAccount DOs register on enable, on sandbox creation and on delete with
 * `{orgId, userId, deviceId, sandboxes: [{provider, sandboxId}]}`; entries
 * are never removed (a deleted device's last month still has to be billed).
 * The hourly cron (billing.ts) walks it to poke each device's reconcile, and
 * once a day compares every configured provider's sandbox list against the
 * known refs: anything a provider runs that no user owns is an `orphan` —
 * billed to us, usually a crashed provision — kept here for the export.
 *
 * Writes are tiny and rare (a handful per user lifecycle), so a single
 * instance is not a hotspot.
 */
import { DurableObject } from "cloudflare:workers";
import type { Env } from "../env";
import type { SandboxRef } from "./metering";

export const CLOUD_INDEX_NAME = "index1";

export interface CloudIndexEntry {
  readonly orgId: string;
  readonly userId: string;
  readonly deviceId?: string;
  readonly sandboxes: readonly SandboxRef[];
  readonly updatedAt: number;
}

export interface CloudOrphan {
  readonly provider: string;
  readonly sandboxId: string;
  readonly state: string;
  readonly firstSeenAt: number;
  readonly lastSeenAt: number;
}

export interface RegisterRequest {
  readonly orgId: string;
  readonly userId: string;
  readonly deviceId?: string;
  readonly sandboxes: readonly SandboxRef[];
}

export class CloudIndex extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    const sql = ctx.storage.sql;
    sql.exec(`CREATE TABLE IF NOT EXISTS devices (
      org_id TEXT NOT NULL, user_id TEXT NOT NULL, device_id TEXT, updated_at INTEGER NOT NULL,
      PRIMARY KEY (org_id, user_id))`);
    sql.exec(`CREATE TABLE IF NOT EXISTS sandboxes (
      provider TEXT NOT NULL, sandbox_id TEXT NOT NULL, org_id TEXT NOT NULL, user_id TEXT NOT NULL,
      PRIMARY KEY (provider, sandbox_id))`);
    sql.exec(`CREATE TABLE IF NOT EXISTS orphans (
      provider TEXT NOT NULL, sandbox_id TEXT NOT NULL, state TEXT NOT NULL,
      first_seen_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL,
      PRIMARY KEY (provider, sandbox_id))`);
    sql.exec(`CREATE TABLE IF NOT EXISTS exports (month TEXT PRIMARY KEY, written_at INTEGER NOT NULL)`);
  }

  register(request: RegisterRequest): void {
    const sql = this.ctx.storage.sql;
    const now = Date.now();
    sql.exec(
      `INSERT INTO devices (org_id, user_id, device_id, updated_at) VALUES (?, ?, ?, ?)
       ON CONFLICT (org_id, user_id) DO UPDATE SET
         device_id = COALESCE(excluded.device_id, devices.device_id), updated_at = excluded.updated_at`,
      request.orgId,
      request.userId,
      request.deviceId ?? null,
      now
    );
    for (const ref of request.sandboxes) {
      sql.exec(
        "INSERT OR IGNORE INTO sandboxes (provider, sandbox_id, org_id, user_id) VALUES (?, ?, ?, ?)",
        ref.provider,
        ref.sandboxId,
        request.orgId,
        request.userId
      );
      // A sandbox flagged before its owner registered it (the create→register
      // window) is not an orphan after all.
      sql.exec("DELETE FROM orphans WHERE provider = ? AND sandbox_id = ?", ref.provider, ref.sandboxId);
    }
  }

  entries(): CloudIndexEntry[] {
    const sql = this.ctx.storage.sql;
    const sandboxes = new Map<string, SandboxRef[]>();
    for (const row of sql.exec("SELECT provider, sandbox_id, org_id, user_id FROM sandboxes")) {
      const key = `${row.org_id}/${row.user_id}`;
      const ref = { provider: row.provider as string, sandboxId: row.sandbox_id as string };
      sandboxes.set(key, [...(sandboxes.get(key) ?? []), ref]);
    }
    return [...sql.exec("SELECT * FROM devices ORDER BY org_id, user_id")].map((row) => ({
      orgId: row.org_id as string,
      userId: row.user_id as string,
      ...(row.device_id === null ? {} : { deviceId: row.device_id as string }),
      sandboxes: sandboxes.get(`${row.org_id}/${row.user_id}`) ?? [],
      updatedAt: row.updated_at as number
    }));
  }

  /** Ids of every sandbox some user owns on `provider`. */
  knownSandboxes(provider: string): string[] {
    return [...this.ctx.storage.sql.exec("SELECT sandbox_id FROM sandboxes WHERE provider = ?", provider)].map(
      (row) => row.sandbox_id as string
    );
  }

  /** Record the orphans one scan found; returns the ones seen for the first time. */
  recordOrphans(found: readonly { provider: string; sandboxId: string; state: string }[], at: number): string[] {
    const sql = this.ctx.storage.sql;
    const fresh: string[] = [];
    for (const orphan of found) {
      const known = [
        ...sql.exec("SELECT 1 FROM sandboxes WHERE provider = ? AND sandbox_id = ?", orphan.provider, orphan.sandboxId)
      ];
      if (known.length > 0) continue;
      const existed = [
        ...sql.exec("SELECT 1 FROM orphans WHERE provider = ? AND sandbox_id = ?", orphan.provider, orphan.sandboxId)
      ];
      if (existed.length === 0) fresh.push(`${orphan.provider}/${orphan.sandboxId}`);
      sql.exec(
        `INSERT INTO orphans (provider, sandbox_id, state, first_seen_at, last_seen_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (provider, sandbox_id) DO UPDATE SET state = excluded.state, last_seen_at = excluded.last_seen_at`,
        orphan.provider,
        orphan.sandboxId,
        orphan.state,
        at,
        at
      );
    }
    return fresh;
  }

  orphans(): CloudOrphan[] {
    return [...this.ctx.storage.sql.exec("SELECT * FROM orphans ORDER BY first_seen_at")].map((row) => ({
      provider: row.provider as string,
      sandboxId: row.sandbox_id as string,
      state: row.state as string,
      firstSeenAt: row.first_seen_at as number,
      lastSeenAt: row.last_seen_at as number
    }));
  }

  exportedAt(month: string): number | undefined {
    const row = [...this.ctx.storage.sql.exec("SELECT written_at FROM exports WHERE month = ?", month)][0];
    return row ? (row.written_at as number) : undefined;
  }

  markExported(month: string, at: number): void {
    this.ctx.storage.sql.exec(
      "INSERT INTO exports (month, written_at) VALUES (?, ?) ON CONFLICT (month) DO UPDATE SET written_at = excluded.written_at",
      month,
      at
    );
  }
}

export const cloudIndexStub = (env: Pick<Env, "CLOUD_INDEX">) =>
  env.CLOUD_INDEX ? env.CLOUD_INDEX.get(env.CLOUD_INDEX.idFromName(CLOUD_INDEX_NAME)) : undefined;
