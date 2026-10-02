/**
 * Static API keys (`anthropic-key`, `openai-key`): the user's own key, pasted
 * on the laptop and uploaded once. Nothing to refresh; grants still expire
 * hourly so a revoke or disconnect reaches consumers within the hour.
 */
import type { VaultProviderId } from "../api";
import { HOUR_MS, type ParseResult, type ProviderAdapter } from "./types";

export interface KeySecret {
  readonly key: string;
}

/** Real keys are ~100 printable chars; the bounds only reject garbage. */
const KEY_RE = /^[\x21-\x7e]{8,512}$/;

export const keyLabel = (key: string): string => `…${key.slice(-4)}`;

const keyAdapter = (id: VaultProviderId): ProviderAdapter<KeySecret> => ({
  id,
  parseUpload: (material): ParseResult<KeySecret> => {
    if (!("key" in material) || typeof material.key !== "string") {
      return { ok: false, message: `${id} material must be {key}` };
    }
    const key = material.key.trim();
    if (!KEY_RE.test(key)) return { ok: false, message: `${id} key is malformed` };
    return { ok: true, secret: { key } };
  },
  account: (secret) => keyLabel(secret.key),
  refreshable: () => false,
  refreshDue: () => false,
  grant: (secret, now) => ({ accessToken: secret.key, expiresAt: now + HOUR_MS, account: keyLabel(secret.key) })
});

export const anthropicKeyAdapter = keyAdapter("anthropic-key");
export const openaiKeyAdapter = keyAdapter("openai-key");
