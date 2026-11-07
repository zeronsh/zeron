/**
 * Static API keys (`anthropic-key`, `openai-key`): the user's own key, pasted
 * on the laptop and uploaded once. Nothing to refresh; grants still expire
 * hourly so a revoke or disconnect reaches consumers within the hour.
 *
 * An Anthropic key may name its own base URL (an LLM gateway or proxy): it
 * is sealed with the key and granted beside it, and Claude Code sends its
 * requests there (`ANTHROPIC_BASE_URL`) instead of Anthropic's API.
 */
import type { VaultProviderId } from "../api";
import { HOUR_MS, type ParseResult, type ProviderAdapter } from "./types";

export interface KeySecret {
  readonly key: string;
  /** `anthropic-key` only: https, no trailing slash. */
  readonly baseUrl?: string;
}

/** Real keys are ~100 printable chars; the bounds only reject garbage. */
const KEY_RE = /^[\x21-\x7e]{8,512}$/;
const BASE_URL_RE = /^[\x21-\x7e]{1,512}$/;

/** The key's last 4 characters, then the host of its base URL if it has one
 * (`…abcd · llm-gateway.example.com`). Never secret material. */
export const keyLabel = (secret: KeySecret): string => {
  const suffix = `…${secret.key.slice(-4)}`;
  return secret.baseUrl ? `${suffix} · ${new URL(secret.baseUrl).host}` : suffix;
};

/**
 * A base URL as stored: trimmed, without trailing slashes; `baseUrl` is
 * absent when the value is absent or empty. Same rules as
 * `VaultProvider::api_base_url` (crates/proto/src/cloud.rs), which answers
 * first in the user's words.
 */
export const parseBaseUrl = (
  value: unknown
): { readonly ok: true; readonly baseUrl?: string } | { readonly ok: false; readonly message: string } => {
  if (value === undefined || value === null) return { ok: true };
  if (typeof value !== "string") return { ok: false, message: "baseUrl must be a string" };
  const raw = value.trim();
  if (!raw) return { ok: true };
  if (!BASE_URL_RE.test(raw)) return { ok: false, message: "baseUrl is malformed" };
  if (!/^https:\/\//i.test(raw)) return { ok: false, message: "baseUrl must be https://" };
  const url = raw.replace(/\/+$/, "");
  const rest = url.slice("https://".length);
  const authority = rest.split(/[/?#]/)[0] ?? "";
  if (!authority) return { ok: false, message: "baseUrl has no host" };
  if (authority.includes("@")) return { ok: false, message: "baseUrl must not carry credentials" };
  if (/[?#]/.test(rest)) return { ok: false, message: "baseUrl must not have a query or fragment" };
  if (/\/v1$/i.test(rest.slice(authority.length))) {
    return { ok: false, message: "baseUrl must not end in /v1 (Claude Code adds /v1/messages)" };
  }
  try {
    new URL(url);
  } catch {
    return { ok: false, message: "baseUrl is malformed" };
  }
  return { ok: true, baseUrl: url };
};

const keyAdapter = (id: VaultProviderId, takesBaseUrl: boolean): ProviderAdapter<KeySecret> => ({
  id,
  parseUpload: (material): ParseResult<KeySecret> => {
    if (!("key" in material) || typeof material.key !== "string") {
      return { ok: false, message: `${id} material must be {key}` };
    }
    const key = material.key.trim();
    if (!KEY_RE.test(key)) return { ok: false, message: `${id} key is malformed` };
    const parsed = parseBaseUrl(material.baseUrl);
    if (!parsed.ok) return { ok: false, message: `${id} ${parsed.message}` };
    if (parsed.baseUrl === undefined) return { ok: true, secret: { key } };
    if (!takesBaseUrl) return { ok: false, message: `${id} takes no baseUrl` };
    return { ok: true, secret: { key, baseUrl: parsed.baseUrl } };
  },
  account: (secret) => keyLabel(secret),
  refreshable: () => false,
  refreshDue: () => false,
  grant: (secret, now) => ({
    accessToken: secret.key,
    expiresAt: now + HOUR_MS,
    account: keyLabel(secret),
    ...(secret.baseUrl ? { baseUrl: secret.baseUrl } : {})
  })
});

export const anthropicKeyAdapter = keyAdapter("anthropic-key", true);
export const openaiKeyAdapter = keyAdapter("openai-key", false);
