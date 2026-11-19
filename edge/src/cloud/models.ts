/**
 * The models Cloud offers, per agent: what the phone (or any client without an
 * engine of its own) shows in a Cloud chat's model picker. Kept by hand, so a
 * new model ships with an edge deploy, not an app release. Match it to the
 * CLIs Cloud machines install (last checked: Claude Code 2.1.307, Codex
 * 0.163.0). Each agent's first model is its default.
 */

export interface ModelOption {
  readonly id: string;
  readonly label: string;
  readonly choices: readonly { readonly id: string; readonly label: string }[];
  readonly defaultChoice: string;
}

export interface CloudModel {
  readonly id: string;
  readonly label: string;
  readonly description: string;
  readonly reasoningLevels: readonly string[];
  readonly options: readonly ModelOption[];
}

export type CloudHarness = "claude-code" | "codex";

const CLAUDE_FULL = ["low", "medium", "high", "xhigh", "max", "ultracode", "ultrathink"];
const CLAUDE_XHIGH = ["low", "medium", "high", "xhigh", "max", "ultrathink"];
const CODEX_ULTRA = ["low", "medium", "high", "xhigh", "max", "ultra"];
const CODEX_MAX = ["low", "medium", "high", "xhigh", "max"];

const toggle = (id: string, label: string): ModelOption => ({
  id,
  label,
  choices: [
    { id: "off", label: "Off" },
    { id: "on", label: "On" },
  ],
  defaultChoice: "off",
});

const contextWindow: ModelOption = {
  id: "contextWindow",
  label: "Context Window",
  choices: [
    { id: "200k", label: "200K" },
    { id: "1m", label: "1M" },
  ],
  defaultChoice: "200k",
};

const serviceTier: ModelOption = {
  id: "serviceTier",
  label: "Service Tier",
  choices: [
    { id: "default", label: "Standard" },
    { id: "fast", label: "Fast" },
  ],
  defaultChoice: "default",
};

const model = (
  id: string,
  label: string,
  description: string,
  reasoningLevels: readonly string[],
  options: readonly ModelOption[] = []
): CloudModel => ({ id, label, description, reasoningLevels, options });

export const CLOUD_MODELS: Readonly<Record<CloudHarness, readonly CloudModel[]>> = {
  "claude-code": [
    model("claude-opus-5-5", "Opus 5.5", "Best for everyday, complex tasks", CLAUDE_FULL, [contextWindow, toggle("fastMode", "Fast Mode")]),
    model("claude-fable-5-1", "Fable 5.1", "Most intelligent model for building agents", CLAUDE_FULL, [contextWindow]),
    model("claude-sonnet-5-5", "Sonnet 5.5", "Most efficient for simpler tasks", CLAUDE_FULL),
    model("claude-haiku-5-5", "Haiku 5.5", "Fastest for quick answers", CLAUDE_FULL),
    model("claude-fable-5", "Fable 5", "Previous generation Fable", CLAUDE_FULL, [contextWindow]),
    model("claude-opus-5", "Opus 5", "Previous generation Opus", CLAUDE_FULL),
    model("claude-opus-4-8", "Opus 4.8", "Older generation Opus", CLAUDE_FULL, [toggle("fastMode", "Fast Mode")]),
    model("claude-sonnet-5", "Sonnet 5", "Previous generation Sonnet", CLAUDE_XHIGH, [contextWindow]),
    model("claude-haiku-4-5", "Haiku 4.5", "Previous generation Haiku", [], [toggle("thinking", "Thinking")]),
  ],
  codex: [
    model("gpt-6.1-sol", "GPT-6.1-Sol", "Latest workhorse model for coding and everyday work.", CODEX_ULTRA, [serviceTier]),
    model("gpt-6-astra", "GPT-6-Astra", "Frontier intelligence for the most demanding work.", CODEX_ULTRA, [serviceTier]),
    model("gpt-6-sol", "GPT-6-Sol", "Previous generation workhorse model.", CODEX_ULTRA, [serviceTier]),
    model("gpt-6-luna", "GPT-6-Luna", "Fast and affordable model for easier tasks.", CODEX_MAX, [serviceTier]),
    model("gpt-5.6-sol", "GPT-5.6-Sol", "Older generation workhorse model.", CODEX_ULTRA, [serviceTier]),
    model("gpt-5.6-terra", "GPT-5.6-Terra", "Older balanced model for straightforward work.", CODEX_ULTRA, [serviceTier]),
    model("gpt-5.6-luna", "GPT-5.6-Luna", "Older fast and efficient model.", CODEX_MAX, [serviceTier]),
    model("gpt-daybreak-blue-latest", "Daybreak Blue", "Latest frontier agentic coding model for broad defensive cybersecurity work.", CODEX_ULTRA),
  ],
};

// ── Catalogs Cloud machines publish ─────────────────────────────────────────

/**
 * Every agent a Cloud machine runs (crates/proto `CLOUD_HARNESSES`). Beyond
 * the two above, an agent's models depend on the credentials Cloud holds for
 * it (Pi lists the models of every provider it's signed in to), so its
 * catalog is the one a machine last discovered with Cloud's credentials
 * (`POST /runner/models`), kept per account and served while every machine
 * sleeps.
 */
export const CLOUD_AGENTS = [
  "claude-code",
  "codex",
  "cursor",
  "devin",
  "grok",
  "hermes",
  "fx",
  "pi",
  "opencode",
  "antigravity",
  "muse"
] as const;
export type CloudAgent = (typeof CLOUD_AGENTS)[number];

export const isCloudAgent = (value: unknown): value is CloudAgent =>
  typeof value === "string" && (CLOUD_AGENTS as readonly string[]).includes(value);

/** Whose models come from `CLOUD_MODELS` (kept by hand), never a machine. */
export const isStaticCloudAgent = (agent: CloudAgent): agent is CloudHarness => agent in CLOUD_MODELS;

/** A published model, as `crates/proto` `Model` serializes it. */
export interface PublishedModel {
  readonly id: string;
  readonly label: string;
  readonly description?: string;
  readonly provider?: { readonly id: string; readonly name: string };
  readonly reasoningLevels: readonly string[];
  readonly options: readonly ModelOption[];
  readonly default?: boolean;
}

export interface PublishedCatalog {
  readonly models: readonly PublishedModel[];
  /** The machine's credential fingerprint the catalog was discovered with
   * (opaque: which logins and keys, never their secrets). */
  readonly credential: string;
  /** Unix ms. */
  readonly at: number;
  /** The accounts it was discovered with (`provider@slot`, sorted, `,`-
   * joined; `parseAccounts`): whose models these are. Absent from a machine
   * whose vault names no account. */
  readonly accounts?: string;
}

/** What `CloudAccount.publishedModels` reads: each agent's newest catalog
 * and its catalog per set of accounts. */
export interface PublishedCatalogs {
  readonly latest: Partial<Record<CloudAgent, PublishedCatalog>>;
  readonly byAccount: Partial<Record<CloudAgent, PublishedCatalog[]>>;
}

/** How many accounts' catalogs one agent keeps, the newest first. */
export const MAX_ACCOUNT_CATALOGS = 8;

/**
 * The accounts a catalog belongs to, as a machine or a sign-in names them
 * (crates/engine `credential_accounts`): `provider@slot` pairs, sorted and
 * joined by `,`. Undefined when it isn't that shape.
 */
export const parseAccounts = (value: unknown): string | undefined => {
  if (typeof value !== "string" || value.length === 0 || value.length > 512) return undefined;
  const pairs = value.split(",");
  const shaped = pairs.every((pair) => /^[a-z0-9-]{1,40}@[A-Za-z0-9_-]{1,64}$/.test(pair));
  const sorted = pairs.every((pair, i) => i === 0 || pairs[i - 1] < pair);
  return shaped && sorted ? value : undefined;
};

/** One window of a subscription's usage, as a machine's run reported it
 * (crates/proto `AgentUsageWindow`). */
export interface UsageWindow {
  readonly label: string;
  readonly usedFraction: number;
  readonly resetsAt?: string;
}

/** A subscription's usage as a machine's run reported it (`POST
 * /runner/usage`), kept per account: what a Cloud card shows where the vault
 * can't ask the provider itself (ChatGPT refuses Cloudflare's servers). */
export interface ReportedUsage {
  readonly windows: readonly UsageWindow[];
  readonly planLabel?: string;
  /** Unix ms. */
  readonly at: number;
}

/** How many accounts' reported usage one Cloud account keeps. */
export const MAX_USAGE_REPORTS = 32;

/** A run's usage windows as kept: at most four, each well-formed, or
 * undefined. */
export const parseUsageWindows = (value: unknown): UsageWindow[] | undefined => {
  if (!Array.isArray(value) || value.length === 0 || value.length > 4) return undefined;
  const windows: UsageWindow[] = [];
  for (const raw of value) {
    if (!isRecord(raw)) return undefined;
    const label = text(raw.label, 40);
    const used = raw.usedFraction;
    if (!label || typeof used !== "number" || !Number.isFinite(used) || used < 0 || used > 100) return undefined;
    const resetsAt = text(raw.resetsAt, 40);
    windows.push({ label, usedFraction: used, ...(resetsAt ? { resetsAt } : {}) });
  }
  return windows;
};

/** The most models one agent's catalog keeps. */
export const MAX_PUBLISHED_MODELS = 400;
/** A catalog's stored size at most (a Durable Object value is ≤ 128 KiB). */
export const MAX_PUBLISHED_BYTES = 100 * 1024;
/** The body of `POST /runner/models` at most. */
export const MAX_PUBLISH_BODY = 512 * 1024;

/** The reasoning levels every client parses (crates/proto `ReasoningLevel`):
 * an older phone reads a whole catalog strictly, so nothing else is kept. */
const REASONING_LEVELS = new Set(["minimal", "low", "medium", "high", "xhigh", "max", "ultra", "ultracode", "ultrathink"]);

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const text = (value: unknown, max: number): string | undefined =>
  typeof value === "string" && value.length > 0 && value.length <= max ? value : undefined;

const option = (value: unknown): ModelOption | undefined => {
  if (!isRecord(value)) return undefined;
  const id = text(value.id, 100);
  const label = text(value.label, 100);
  const defaultChoice = text(value.defaultChoice, 100);
  if (!id || !label || !defaultChoice || !Array.isArray(value.choices) || value.choices.length > 32) return undefined;
  const choices: { id: string; label: string }[] = [];
  for (const choice of value.choices) {
    const choiceId = isRecord(choice) ? text(choice.id, 100) : undefined;
    const choiceLabel = isRecord(choice) ? text(choice.label, 100) : undefined;
    if (!choiceId || !choiceLabel) return undefined;
    choices.push({ id: choiceId, label: choiceLabel });
  }
  return { id, label, choices, defaultChoice };
};

/** One model, or undefined when it isn't one (dropped, not fatal). */
const publishedModel = (value: unknown): PublishedModel | undefined => {
  if (!isRecord(value)) return undefined;
  const id = text(value.id, 200);
  const label = text(value.label, 200);
  if (!id || !label) return undefined;
  const description = text(value.description, 500);
  const provider =
    isRecord(value.provider) && text(value.provider.id, 100) && text(value.provider.name, 100)
      ? { id: value.provider.id as string, name: value.provider.name as string }
      : undefined;
  const reasoningLevels = Array.isArray(value.reasoningLevels)
    ? value.reasoningLevels.filter((level): level is string => typeof level === "string" && REASONING_LEVELS.has(level))
    : [];
  const options = Array.isArray(value.options)
    ? value.options.slice(0, 16).map(option).filter((o): o is ModelOption => o !== undefined)
    : [];
  return {
    id,
    label,
    ...(description ? { description } : {}),
    ...(provider ? { provider } : {}),
    reasoningLevels: [...new Set(reasoningLevels)],
    options,
    ...(value.default === true ? { default: true } : {})
  };
};

/**
 * A machine's catalog as it is kept: the models that parse (the rest
 * dropped), at most `MAX_PUBLISHED_MODELS`, distinct by id, and small enough
 * to store — descriptions go first, then models from the end. Undefined when
 * nothing usable is left.
 */
export const parsePublishedModels = (value: unknown): PublishedModel[] | undefined => {
  if (!Array.isArray(value)) return undefined;
  const seen = new Set<string>();
  let models: PublishedModel[] = [];
  for (const raw of value) {
    if (models.length >= MAX_PUBLISHED_MODELS) break;
    const model = publishedModel(raw);
    if (!model || seen.has(model.id)) continue;
    seen.add(model.id);
    models.push(model);
  }
  const size = (list: readonly PublishedModel[]) => JSON.stringify(list).length;
  if (size(models) > MAX_PUBLISHED_BYTES) {
    models = models.map(({ description: _, ...rest }) => rest);
  }
  while (models.length > 0 && size(models) > MAX_PUBLISHED_BYTES) {
    models = models.slice(0, Math.floor(models.length * 0.9));
  }
  return models.length > 0 ? models : undefined;
};

/** Every agent's catalogs per account, the newest first, as `GET
 * /cloud/{orgId}/models` answers them beside `models` (`catalogs`): a client
 * shows the one discovered with the accounts Cloud runs that agent on now. */
export const accountCatalogs = (
  byAccount: Readonly<Partial<Record<CloudAgent, readonly PublishedCatalog[]>>>
): Record<string, { accounts: string; models: readonly PublishedModel[]; at: number }[]> => {
  const out: Record<string, { accounts: string; models: readonly PublishedModel[]; at: number }[]> = {};
  for (const agent of CLOUD_AGENTS) {
    const catalogs = (byAccount[agent] ?? [])
      .filter((c): c is PublishedCatalog & { accounts: string } => !!c.accounts && c.models.length > 0)
      .sort((a, b) => b.at - a.at)
      .map(({ accounts, models, at }) => ({ accounts, models, at }));
    if (!isStaticCloudAgent(agent) && catalogs.length > 0) out[agent] = catalogs;
  }
  return out;
};

/** What `GET /cloud/{orgId}/models` answers: the hand-kept lists, then every
 * other agent's last published catalog. */
export const cloudModels = (
  published: Readonly<Partial<Record<CloudAgent, PublishedCatalog>>>
): Record<string, readonly (CloudModel | PublishedModel)[]> => {
  const out: Record<string, readonly (CloudModel | PublishedModel)[]> = { ...CLOUD_MODELS };
  for (const agent of CLOUD_AGENTS) {
    const catalog = published[agent];
    if (!isStaticCloudAgent(agent) && catalog && catalog.models.length > 0) out[agent] = catalog.models;
  }
  return out;
};
