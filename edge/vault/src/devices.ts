/**
 * VaultDevices — one Durable Object per user (`dev1/{userId}`): the user's
 * enrolled device keys, each device's grant replay fence, the per-user kill
 * switch, and an append-only audit log.
 *
 * A grant request is a statement signed by a device key:
 *   Ed25519("zeron-vault-grant\n{userId}\n{deviceId}\n{provider}\n{ts}")
 * accepted only while `ts` is within ±120 s of now AND strictly greater than
 * that device's last accepted `ts`. The window bounds how long a captured
 * request is useful; the fence makes it single-use even inside the window.
 * Consumers therefore must send strictly increasing `ts` per device (Unix ms,
 * bumped past the previous value when two requests share a millisecond).
 *
 * Parents: every Cloud session runs in its own sandbox, enrolled as its own
 * device (own key, own fence) with `parent_id` = the account's logical Cloud
 * device. A credential authorized for the parent covers every child, and
 * revoking the parent — even one that never enrolled a key of its own —
 * refuses all of its children, present and future. One level only: a child
 * cannot itself be a parent.
 *
 * The audit log records who did what to which slot — never token material,
 * never signatures. It is bounded by age, not by trimming recent history.
 */
import { DurableObject } from "cloudflare:workers";
import type { EnrollDeviceRequest, GrantRequest, VaultDeviceView, VaultProviderId, VaultResult } from "./api";
import { fromBase64Url, utf8 } from "./encoding";
import type { Env } from "./env";
import { fail, ok, type VaultFailure } from "./result";

export const GRANT_WINDOW_MS = 120_000;
const AUDIT_RETAIN_MS = 400 * 24 * 60 * 60_000;

export const grantMessage = (userId: string, deviceId: string, provider: VaultProviderId, ts: number): string =>
  `zeron-vault-grant\n${userId}\n${deviceId}\n${provider}\n${ts}`;

export type AuditEvent =
  | "enroll"
  | "revoke"
  | "disable"
  | "enable"
  | "upload"
  | "authorize"
  | "disconnect"
  | "grant"
  | "grant_denied"
  | "refresh"
  | "refresh_failed"
  | "github_connect"
  | "github_installation_token";

export interface AuditEntry {
  readonly event: AuditEvent;
  readonly orgId?: string;
  readonly deviceId?: string;
  readonly provider?: VaultProviderId;
  /** Short non-secret context: an error code, a generation, a device count. */
  readonly detail?: string;
}

export interface AuditRecord extends AuditEntry {
  readonly seq: number;
  readonly at: number;
}

/** A verified grant request's principals: the device and, for a Cloud
 * session sandbox, its logical parent (either may be authorized). */
export interface GrantPrincipal {
  readonly deviceId: string;
  readonly parentId?: string;
}

export interface DevicesSnapshot {
  readonly disabled: boolean;
  readonly devices: VaultDeviceView[];
}

type DeviceRow = {
  device_id: string;
  kind: string;
  public_key: string;
  enrolled_at: number;
  revoked_at: number | null;
  last_grant_ts: number;
  parent_id: string | null;
};

/** `public_key` of a placeholder row: a parent revoked without ever enrolling
 * a key of its own. Never decodes, so it can never verify anything. */
const NO_KEY = "";

type AuditRow = {
  seq: number;
  at: number;
  event: string;
  org_id: string | null;
  device_id: string | null;
  provider: string | null;
  detail: string | null;
};

const importDeviceKey = (raw: Uint8Array<ArrayBuffer>): Promise<CryptoKey> =>
  crypto.subtle.importKey("raw", raw, { name: "Ed25519" }, false, ["verify"]);

export class VaultDevices extends DurableObject<Env> {
  private readonly sql: SqlStorage;
  /** Imported verify keys, keyed by the base64url public key. */
  private readonly keys = new Map<string, CryptoKey>();
  private pruned = false;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
    this.sql.exec(
      `CREATE TABLE IF NOT EXISTS devices (
        device_id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        public_key TEXT NOT NULL,
        enrolled_at INTEGER NOT NULL,
        revoked_at INTEGER,
        last_grant_ts INTEGER NOT NULL DEFAULT 0,
        parent_id TEXT
      )`
    );
    this.sql.exec("CREATE INDEX IF NOT EXISTS devices_parent ON devices (parent_id)");
    this.sql.exec(
      `CREATE TABLE IF NOT EXISTS audit (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        event TEXT NOT NULL,
        org_id TEXT,
        device_id TEXT,
        provider TEXT,
        detail TEXT
      )`
    );
  }

  // ── RPC surface (called only by VaultApi / VaultAccount) ─────────────────

  snapshot(userId: string): VaultResult<DevicesSnapshot> {
    const owner = this.claim(userId);
    if (owner) return owner;
    return ok({ disabled: this.disabledFlag(), devices: this.listDevices() });
  }

  isDisabled(userId: string): VaultResult<boolean> {
    const owner = this.claim(userId);
    if (owner) return owner;
    return ok(this.disabledFlag());
  }

  async enroll(userId: string, orgId: string, request: EnrollDeviceRequest): Promise<VaultResult<null>> {
    const owner = this.claim(userId);
    if (owner) return owner;
    if (this.disabledFlag()) return fail("disabled", "the vault is disabled for this user");
    let raw: Uint8Array<ArrayBuffer>;
    try {
      raw = fromBase64Url(request.publicKey);
    } catch {
      return fail("bad_request", "publicKey must be base64url");
    }
    if (raw.length !== 32) return fail("bad_request", "publicKey must be a raw 32-byte Ed25519 key");
    try {
      this.keys.set(request.publicKey, await importDeviceKey(raw));
    } catch {
      return fail("bad_request", "publicKey is not a valid Ed25519 key");
    }
    // One level of parents. A child under a revoked parent is refused here
    // already (and would be refused at every grant anyway).
    const parentId = request.parentId ?? null;
    if (parentId !== null) {
      if (parentId === request.deviceId) return fail("bad_request", "a device cannot be its own parent");
      const parent = this.device(parentId);
      if (parent?.parent_id) return fail("bad_request", "a session device cannot be a parent");
      if (parent && parent.revoked_at !== null) return fail("device_revoked", "parent device was revoked");
      if (this.hasChildren(request.deviceId)) return fail("bad_request", "a parent device cannot be enrolled as a child");
    }
    const now = Date.now();
    const existing = this.device(request.deviceId);
    const sameKey = existing?.public_key === request.publicKey;
    // A revoke retires the KEY, not just the row: engines may re-enroll
    // idempotently on startup, and that must never silently re-trust a key
    // the user revoked (lost laptop) — under its old id or any other.
    // Recovery means a freshly generated key.
    const revokedKey = this.sql
      .exec("SELECT 1 FROM devices WHERE public_key = ? AND revoked_at IS NOT NULL LIMIT 1", request.publicKey)
      .toArray()[0];
    if (revokedKey) return fail("device_revoked", "this device key was revoked; enroll a new key");
    // Re-enrolling with a new key (rotation, recovery after a revoke) resets
    // the replay fence: a new key has its own `ts` history.
    this.sql.exec(
      `INSERT INTO devices (device_id, kind, public_key, enrolled_at, revoked_at, last_grant_ts, parent_id)
       VALUES (?, ?, ?, ?, NULL, 0, ?)
       ON CONFLICT(device_id) DO UPDATE SET
         kind = excluded.kind,
         public_key = excluded.public_key,
         enrolled_at = CASE WHEN ? THEN devices.enrolled_at ELSE excluded.enrolled_at END,
         revoked_at = NULL,
         last_grant_ts = CASE WHEN ? THEN devices.last_grant_ts ELSE 0 END,
         parent_id = excluded.parent_id`,
      request.deviceId,
      request.kind,
      request.publicKey,
      now,
      parentId,
      sameKey ? 1 : 0,
      sameKey ? 1 : 0
    );
    this.appendAudit({
      event: "enroll",
      orgId,
      deviceId: request.deviceId,
      detail: `${existing ? (sameKey ? "re-enroll" : "rotate") : request.kind}${parentId ? ` parent=${parentId}` : ""}`
    });
    return ok(null);
  }

  /**
   * Revoke a device. Revoking a parent also stamps every child (so the UI
   * shows them revoked) and, if the parent never enrolled a key itself, leaves
   * a keyless revoked row for it: children enrolled LATER are refused too.
   * Revoking a child touches only that child.
   */
  revoke(userId: string, orgId: string, deviceId: string): VaultResult<null> {
    const owner = this.claim(userId);
    if (owner) return owner;
    const existing = this.device(deviceId);
    const children = this.hasChildren(deviceId);
    if (!existing && !children) return fail("not_found", "no such device");
    const now = Date.now();
    if (!existing) {
      this.sql.exec(
        "INSERT INTO devices (device_id, kind, public_key, enrolled_at, revoked_at) VALUES (?, 'cloud', ?, ?, ?)",
        deviceId,
        NO_KEY,
        now,
        now
      );
      this.appendAudit({ event: "revoke", orgId, deviceId, detail: "parent without key" });
    } else if (existing.revoked_at === null) {
      this.sql.exec("UPDATE devices SET revoked_at = ? WHERE device_id = ?", now, deviceId);
      this.appendAudit({ event: "revoke", orgId, deviceId });
    }
    if (children) {
      const stamped = this.sql.exec(
        "UPDATE devices SET revoked_at = ? WHERE parent_id = ? AND revoked_at IS NULL",
        now,
        deviceId
      ).rowsWritten;
      if (stamped > 0) this.appendAudit({ event: "revoke", orgId, deviceId, detail: `children=${stamped}` });
    }
    return ok(null);
  }

  setDisabled(userId: string, orgId: string, disabled: boolean): VaultResult<null> {
    const owner = this.claim(userId);
    if (owner) return owner;
    if (this.disabledFlag() !== disabled) {
      this.setMeta("disabled", disabled ? "1" : "0");
      this.appendAudit({ event: disabled ? "disable" : "enable", orgId });
    }
    return ok(null);
  }

  /**
   * Authenticate a grant request: user not disabled, device (and its parent)
   * enrolled-or-absent and not revoked, signature valid, `ts` fresh and past
   * the fence. On success the fence advances — before the grant is even
   * issued — so the request is spent whatever happens next. Returns the
   * principals the account checks against `authorizedDevices`.
   */
  async verifyGrant(userId: string, orgId: string, request: GrantRequest): Promise<VaultResult<GrantPrincipal>> {
    const owner = this.claim(userId);
    if (owner) return owner;
    const refuse = (failure: VaultFailure): VaultFailure => {
      this.appendAudit({
        event: "grant_denied",
        orgId,
        deviceId: request.deviceId,
        provider: request.provider,
        detail: failure.error
      });
      return failure;
    };
    if (this.disabledFlag()) return refuse(fail("disabled", "the vault is disabled for this user"));
    const device = this.device(request.deviceId);
    if (!device || device.public_key === NO_KEY) return refuse(fail("device_unknown", "device is not enrolled"));
    const revoked = this.revocation(device);
    if (revoked) return refuse(revoked);

    // Signature before freshness: callers without the key learn nothing about
    // the fence.
    let signature: Uint8Array<ArrayBuffer>;
    try {
      signature = fromBase64Url(request.sig);
    } catch {
      return refuse(fail("bad_signature", "signature is not base64url"));
    }
    let valid = false;
    try {
      const key = await this.verifyKey(device.public_key);
      valid = await crypto.subtle.verify(
        { name: "Ed25519" },
        key,
        signature,
        utf8(grantMessage(userId, request.deviceId, request.provider, request.ts))
      );
    } catch {
      valid = false;
    }
    if (!valid) return refuse(fail("bad_signature", "signature does not verify"));

    // Re-read after the awaits: a concurrent request (or a revoke/re-enroll)
    // may have landed meanwhile. Everything from here to the write is
    // synchronous, so the check-and-advance is atomic.
    const current = this.device(request.deviceId);
    if (!current || current.public_key !== device.public_key) {
      return refuse(fail("device_unknown", "device key changed"));
    }
    const revokedNow = this.revocation(current);
    if (revokedNow) return refuse(revokedNow);
    if (Math.abs(Date.now() - request.ts) > GRANT_WINDOW_MS) {
      return refuse(fail("stale", "ts is outside the ±120 s window"));
    }
    if (request.ts <= current.last_grant_ts) return refuse(fail("stale", "ts was already used (replay fence)"));
    this.sql.exec("UPDATE devices SET last_grant_ts = ? WHERE device_id = ?", request.ts, request.deviceId);
    return ok(current.parent_id ? { deviceId: current.device_id, parentId: current.parent_id } : { deviceId: current.device_id });
  }

  audit(userId: string, entry: AuditEntry): VaultResult<null> {
    const owner = this.claim(userId);
    if (owner) return owner;
    this.appendAudit(entry);
    return ok(null);
  }

  /** Newest first. Ops/test surface: entries are non-secret by construction. */
  auditLog(userId: string, limit = 100): VaultResult<AuditRecord[]> {
    const owner = this.claim(userId);
    if (owner) return owner;
    const rows = this.sql
      .exec<AuditRow>("SELECT * FROM audit ORDER BY seq DESC LIMIT ?", Math.max(1, Math.min(limit, 1000)))
      .toArray();
    return ok(
      rows.map((row) => ({
        seq: row.seq,
        at: row.at,
        event: row.event as AuditEvent,
        orgId: row.org_id ?? undefined,
        deviceId: row.device_id ?? undefined,
        provider: (row.provider ?? undefined) as VaultProviderId | undefined,
        detail: row.detail ?? undefined
      }))
    );
  }

  // ── internals ────────────────────────────────────────────────────────────

  /** Pin the DO to the first user that touches it. Names are derived from the
   * user id, so a mismatch means a routing bug — refuse rather than mix. */
  private claim(userId: string): VaultFailure | undefined {
    const owner = this.getMeta("owner");
    if (owner === undefined) this.setMeta("owner", userId);
    else if (owner !== userId) return fail("forbidden", "vault object belongs to another user");
    return undefined;
  }

  private disabledFlag(): boolean {
    return this.getMeta("disabled") === "1";
  }

  /** The device's own revoke, or its parent's (a parent row that is revoked,
   * keyless placeholder or not). */
  private revocation(device: DeviceRow): VaultFailure | undefined {
    if (device.revoked_at !== null) return fail("device_revoked", "device was revoked");
    if (device.parent_id) {
      const parent = this.device(device.parent_id);
      if (parent && parent.revoked_at !== null) return fail("device_revoked", "parent device was revoked");
    }
    return undefined;
  }

  private hasChildren(deviceId: string): boolean {
    return this.sql.exec("SELECT 1 FROM devices WHERE parent_id = ? LIMIT 1", deviceId).toArray().length > 0;
  }

  private device(deviceId: string): DeviceRow | undefined {
    return this.sql.exec<DeviceRow>("SELECT * FROM devices WHERE device_id = ?", deviceId).toArray()[0];
  }

  private listDevices(): VaultDeviceView[] {
    return this.sql
      .exec<DeviceRow>("SELECT * FROM devices ORDER BY enrolled_at, device_id")
      .toArray()
      .map((row) => ({
        deviceId: row.device_id,
        kind: row.kind === "cloud" ? "cloud" : "laptop",
        enrolledAt: row.enrolled_at,
        ...(row.revoked_at === null ? {} : { revokedAt: row.revoked_at }),
        ...(row.parent_id ? { parentId: row.parent_id } : {})
      }));
  }

  private async verifyKey(publicKey: string): Promise<CryptoKey> {
    let key = this.keys.get(publicKey);
    if (!key) {
      key = await importDeviceKey(fromBase64Url(publicKey));
      this.keys.set(publicKey, key);
    }
    return key;
  }

  private appendAudit(entry: AuditEntry): void {
    const now = Date.now();
    this.sql.exec(
      "INSERT INTO audit (at, event, org_id, device_id, provider, detail) VALUES (?, ?, ?, ?, ?, ?)",
      now,
      entry.event,
      entry.orgId ?? null,
      entry.deviceId ?? null,
      entry.provider ?? null,
      entry.detail?.slice(0, 200) ?? null
    );
    if (!this.pruned) {
      this.pruned = true;
      this.sql.exec("DELETE FROM audit WHERE at < ?", now - AUDIT_RETAIN_MS);
    }
  }

  private getMeta(key: string): string | undefined {
    return this.sql.exec<{ value: string }>("SELECT value FROM meta WHERE key = ?", key).toArray()[0]?.value;
  }

  private setMeta(key: string, value: string): void {
    this.sql.exec(
      "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
      key,
      value
    );
  }
}
