/** `VaultResult` constructors. Every refusal carries the HTTP status the main
 * Worker should answer with, so the binding stays a dumb pass-through. */
import type { VaultErrorCode, VaultResult } from "./api";

const DEFAULT_STATUS: Record<VaultErrorCode, number> = {
  disabled: 403,
  bad_request: 400,
  forbidden: 403,
  not_found: 404,
  bad_signature: 401,
  stale: 401,
  device_unknown: 403,
  device_revoked: 403,
  not_authorized: 403,
  needs_reconnect: 409,
  upstream: 502,
  unavailable: 503
};

export type VaultFailure = Extract<VaultResult<never>, { ok: false }>;

export const ok = <T>(value: T): VaultResult<T> => ({ ok: true, value });

export const fail = (error: VaultErrorCode, message: string, status = DEFAULT_STATUS[error]): VaultFailure => ({
  ok: false,
  error,
  message,
  status
});
