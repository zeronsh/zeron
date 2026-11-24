/**
 * Shape checks for everything that crosses the service binding. The main
 * Worker is trusted to have VERIFIED the caller, not to have validated every
 * field of a client-supplied body, so the vault re-checks types and bounds.
 */
import {
  CONNECTION_ID_RE,
  VAULT_PROVIDERS,
  type ConnectionGrantManyRequest,
  type ConnectionGrantRequest,
  type GrantManyRequest,
  type GrantRequest,
  type VaultCaller,
  type VaultConnectionKind,
  type VaultProviderId
} from "./api";
import { ID_RE } from "./env";
import { isRepoFullName } from "./providers/github-app";

/** More devices than any one user plausibly owns; bounds row size. */
const MAX_AUTHORIZED_DEVICES = 32;
/** An Ed25519 signature is 64 bytes = 86 base64url chars. */
const MAX_SIG_CHARS = 128;
const MAX_BATCH_CONNECTIONS = 100;

export const isProvider = (value: unknown): value is VaultProviderId =>
  typeof value === "string" && (VAULT_PROVIDERS as readonly string[]).includes(value);

export const isId = (value: unknown): value is string => typeof value === "string" && ID_RE.test(value);

export const isConnectionId = (value: unknown): value is string =>
  typeof value === "string" && CONNECTION_ID_RE.test(value);

export const isConnectionKind = (value: unknown): value is VaultConnectionKind => value === "mcp" || value === "plugin";

export const isCaller = (value: unknown): value is VaultCaller => {
  if (typeof value !== "object" || value === null) return false;
  const caller = value as Partial<VaultCaller>;
  if (!isId(caller.userId) || typeof caller.orgId !== "string") return false;
  if (caller.kind === "user") return caller.deviceId === undefined || isId(caller.deviceId);
  return caller.kind === "runner" && isId(caller.deviceId);
};

/** Deduplicated device list, or undefined when malformed. */
export const parseDeviceList = (value: unknown): string[] | undefined => {
  if (!Array.isArray(value) || value.length > MAX_AUTHORIZED_DEVICES) return undefined;
  if (!value.every(isId)) return undefined;
  return [...new Set(value as string[])];
};

export const isGrantRequest = (value: unknown): value is GrantRequest => {
  if (typeof value !== "object" || value === null) return false;
  const request = value as Partial<GrantRequest>;
  return (
    isProvider(request.provider) &&
    isId(request.deviceId) &&
    typeof request.ts === "number" &&
    Number.isSafeInteger(request.ts) &&
    request.ts > 0 &&
    typeof request.sig === "string" &&
    request.sig.length > 0 &&
    request.sig.length <= MAX_SIG_CHARS &&
    (request.repo === undefined || isRepoFullName(request.repo)) &&
    (request.inheritFrom === undefined || isId(request.inheritFrom))
  );
};

/** The most providers one batch may name: every provider but `github`. */
export const MAX_BATCH_PROVIDERS = VAULT_PROVIDERS.length - 1;

/** A batch names providers sorted and distinct (so the signed list has one
 * spelling), never `github` (its grants need a repository). */
export const isGrantManyRequest = (value: unknown): value is GrantManyRequest => {
  if (typeof value !== "object" || value === null) return false;
  const request = value as Partial<GrantManyRequest>;
  const providers = request.providers;
  if (!Array.isArray(providers) || providers.length === 0 || providers.length > MAX_BATCH_PROVIDERS) return false;
  for (let i = 0; i < providers.length; i++) {
    const provider: unknown = providers[i];
    if (!isProvider(provider) || provider === "github") return false;
    if (i > 0 && !((providers[i - 1] as string) < provider)) return false;
  }
  return (
    isId(request.deviceId) &&
    typeof request.ts === "number" &&
    Number.isSafeInteger(request.ts) &&
    request.ts > 0 &&
    typeof request.sig === "string" &&
    request.sig.length > 0 &&
    request.sig.length <= MAX_SIG_CHARS
  );
};

export const isConnectionGrantRequest = (value: unknown): value is ConnectionGrantRequest => {
  if (typeof value !== "object" || value === null) return false;
  const request = value as Partial<ConnectionGrantRequest>;
  return (
    isConnectionId(request.id) &&
    isId(request.deviceId) &&
    typeof request.ts === "number" &&
    Number.isSafeInteger(request.ts) &&
    request.ts > 0 &&
    typeof request.sig === "string" &&
    request.sig.length > 0 &&
    request.sig.length <= MAX_SIG_CHARS &&
    (request.forceRefresh === undefined || typeof request.forceRefresh === "boolean")
  );
};

export const isConnectionGrantManyRequest = (value: unknown): value is ConnectionGrantManyRequest => {
  if (typeof value !== "object" || value === null) return false;
  const request = value as Partial<ConnectionGrantManyRequest>;
  const ids = request.ids;
  if (!Array.isArray(ids) || ids.length === 0 || ids.length > MAX_BATCH_CONNECTIONS) return false;
  const seen = new Set<string>();
  for (const id of ids) {
    if (!isConnectionId(id) || seen.has(id)) return false;
    seen.add(id);
  }
  return (
    isId(request.deviceId) &&
    typeof request.ts === "number" &&
    Number.isSafeInteger(request.ts) &&
    request.ts > 0 &&
    typeof request.sig === "string" &&
    request.sig.length > 0 &&
    request.sig.length <= MAX_SIG_CHARS
  );
};

/** Flow ids are 16 random bytes, base64url. */
export const isFlowId = (value: unknown): value is string => typeof value === "string" && /^[A-Za-z0-9_-]{22}$/.test(value);
