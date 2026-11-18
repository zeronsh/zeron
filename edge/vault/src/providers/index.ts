/** Provider registry. Secrets are opaque JSON to everything outside the
 * adapter that owns them, hence the one `unknown` cast below. */
import type { VaultProviderId } from "../api";
import { claudeAdapter } from "./claude";
import { codexAdapter } from "./codex";
import { copilotAdapter } from "./copilot";
import { devinAdapter } from "./devin";
import { githubAdapter } from "./github";
import { grokAdapter } from "./grok";
import { anthropicKeyAdapter, keyAdapter, openaiKeyAdapter } from "./keys";
import type { ProviderAdapter } from "./types";

const ADAPTERS = {
  codex: codexAdapter,
  claude: claudeAdapter,
  github: githubAdapter,
  copilot: copilotAdapter,
  devin: devinAdapter,
  grok: grokAdapter,
  "anthropic-key": anthropicKeyAdapter,
  "openai-key": openaiKeyAdapter,
  "openrouter-key": keyAdapter("openrouter-key"),
  "gemini-key": keyAdapter("gemini-key"),
  "xai-key": keyAdapter("xai-key"),
  "deepseek-key": keyAdapter("deepseek-key"),
  "ai-gateway-key": keyAdapter("ai-gateway-key"),
  "meta-key": keyAdapter("meta-key"),
  "cursor-key": keyAdapter("cursor-key")
} satisfies Record<VaultProviderId, { readonly id: VaultProviderId }>;

export const adapterFor = (provider: VaultProviderId): ProviderAdapter<unknown> =>
  ADAPTERS[provider] as unknown as ProviderAdapter<unknown>;

/** Providers the daily canary exercises: the ones with a refresh contract
 * (or, for Claude setup tokens, an expiry worth warning about). */
export const CANARY_PROVIDERS: readonly VaultProviderId[] = ["codex", "claude", "github", "grok"];
