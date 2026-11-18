/**
 * Devin sign-ins (`devin`): the `credentials.toml` Devin's own browser
 * sign-in writes, uploaded as a table. It holds a long-lived key
 * (`windsurf_api_key`; `devin_api_key` in older CLIs) and the servers that key
 * belongs to. There is no refresh token and nothing to refresh.
 *
 * Only string fields are kept (the file holds nothing else), bounded in
 * number and size, and every url must be an `https` Devin / Windsurf /
 * Codeium host: a grant hands them to a machine, which sends the key there.
 * The key goes out as the grant's token; every other field rides
 * `settings`, for the machine to write the file back as it was.
 */
import { keyId } from "../crypto";
import { utf8 } from "../encoding";
import { keyLabel } from "./keys";
import { HOUR_MS, isObject, type ParseResult, type ProviderAdapter } from "./types";

export interface DevinSecret {
  readonly key: string;
  /** Every other string field of `credentials.toml`, as written. */
  readonly fields: Readonly<Record<string, string>>;
}

const KEY_FIELDS = ["windsurf_api_key", "devin_api_key"] as const;
const KEY_RE = /^[\x21-\x7e]{8,1024}$/;
const FIELD_RE = /^[a-z][a-z0-9_]{0,63}$/;
const MAX_FIELDS = 32;
const MAX_VALUE = 2048;
/** Where Devin's servers live (a field named `*_url` must point at one). */
export const DEVIN_DOMAINS = ["devin.ai", "windsurf.com", "codeium.com"] as const;

export const isDevinUrl = (value: string): boolean => {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" || url.username || url.password || url.port) return false;
  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  return DEVIN_DOMAINS.some((domain) => host === domain || host.endsWith(`.${domain}`));
};

export const parseDevinUpload = (material: unknown): ParseResult<DevinSecret> => {
  const table = isObject(material) ? material.devinCredentials : undefined;
  if (!isObject(table)) return { ok: false, message: "devin material must be {devinCredentials}" };
  let key: string | undefined;
  const fields: Record<string, string> = {};
  let count = 0;
  for (const [name, value] of Object.entries(table)) {
    if (typeof value !== "string") continue;
    if ((KEY_FIELDS as readonly string[]).includes(name)) {
      key ??= value.trim();
      continue;
    }
    if (!FIELD_RE.test(name) || value.length > MAX_VALUE) {
      return { ok: false, message: `devin credentials field ${JSON.stringify(name.slice(0, 64))} is malformed` };
    }
    if (name.endsWith("_url") && value !== "" && !isDevinUrl(value)) {
      return { ok: false, message: `devin ${name} is not an https Devin server` };
    }
    if (++count > MAX_FIELDS) return { ok: false, message: "devin credentials has too many fields" };
    fields[name] = value;
  }
  if (!key || !KEY_RE.test(key)) return { ok: false, message: "devin credentials hold no usable key" };
  return { ok: true, secret: { key, fields } };
};

export const devinAdapter: ProviderAdapter<DevinSecret> = {
  id: "devin",
  parseUpload: (material) => parseDevinUpload(material),
  account: (secret) => keyLabel(secret.key),
  identity: async (secret) => `key:${await keyId(utf8(secret.key))}`,
  refreshable: () => false,
  refreshDue: () => false,
  grant: (secret, now) => ({
    accessToken: secret.key,
    expiresAt: now + HOUR_MS,
    account: keyLabel(secret.key),
    settings: { ...secret.fields }
  })
};
