/** Provider registry. Secrets are opaque JSON to everything outside the
 * adapter that owns them, hence the one `unknown` cast below. */
import type { VaultProviderId } from "../api";
import { claudeAdapter } from "./claude";
import { codexAdapter } from "./codex";
import { githubAdapter } from "./github";
import { anthropicKeyAdapter, openaiKeyAdapter } from "./keys";
import type { ProviderAdapter } from "./types";

const ADAPTERS = {
  codex: codexAdapter,
  claude: claudeAdapter,
  github: githubAdapter,
  "anthropic-key": anthropicKeyAdapter,
  "openai-key": openaiKeyAdapter
} satisfies Record<VaultProviderId, { readonly id: VaultProviderId }>;

export const adapterFor = (provider: VaultProviderId): ProviderAdapter<unknown> =>
  ADAPTERS[provider] as unknown as ProviderAdapter<unknown>;

/** Providers the daily canary exercises: the ones with a refresh contract
 * (or, for Claude setup tokens, an expiry worth warning about). */
export const CANARY_PROVIDERS: readonly VaultProviderId[] = ["codex", "claude", "github"];
