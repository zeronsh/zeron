import type { HarnessDescriptor, Model, ReasoningLevel } from "@zeron/proto";

/** `pickers.rs::reasoning_label` — the ladder's display names. */
const REASONING_LABELS: Record<ReasoningLevel, string> = {
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "X-High",
  max: "Max",
  ultra: "Ultra",
  ultracode: "Ultracode",
  ultrathink: "Ultrathink",
};

export function reasoningLabel(level: ReasoningLevel): string {
  return REASONING_LABELS[level] ?? level;
}

/**
 * `pickers.rs::default_reasoning` — the recommended default is High
 * (user-corrected), falling to Medium, then the ladder's first entry; `null`
 * only for ladder-less models (e.g. Haiku's thinking toggle instead).
 */
export function defaultReasoning(ladder: readonly ReasoningLevel[]): ReasoningLevel | null {
  if (ladder.includes("high")) {
    return "high";
  }
  if (ladder.includes("medium")) {
    return "medium";
  }
  return ladder[0] ?? null;
}

/**
 * `pickers.rs::clamp_reasoning` — keep a picked/remembered level when the
 * ladder offers it, else heal to the model's default (never a stale or
 * foreign level).
 */
export function clampReasoning(
  level: ReasoningLevel | null,
  ladder: readonly ReasoningLevel[],
): ReasoningLevel | null {
  if (level !== null && ladder.includes(level)) {
    return level;
  }
  return defaultReasoning(ladder);
}

/**
 * `pickers.rs::trait_ladder` — the ladder a reasoning selection resolves
 * against: the selected model's own levels in their advertised order when
 * NONEMPTY, else the matching harness descriptor's advertised levels (Haiku's
 * empty list falls back to Claude's; an OpenCode model with no recognized
 * variants falls back to OpenCode's). No selected model means no effective
 * ladder at all — never a union of the two lists, never a descriptor peek
 * before the model resolves.
 */
export function effectiveReasoningLadder(
  model: Model | null | undefined,
  descriptor: HarnessDescriptor | null | undefined,
): readonly ReasoningLevel[] {
  if (model === null || model === undefined) {
    return [];
  }
  if (model.reasoningLevels.length > 0) {
    return model.reasoningLevels;
  }
  return descriptor?.reasoningLevels ?? [];
}

/**
 * The identity chip's muted second tone — a port of
 * `pickers.rs::traits_summary`.
 *
 * The effective reasoning level plus every model option's effective choice
 * (the explicit pick when one is saved and the model still offers it, else the
 * option's default), joined with " · ": "High · 1M · Fast", or Cursor's
 * "Agent · Balance". Defaults are spelled out rather than hidden, so the run's
 * configuration reads without opening anything. `null` only when the model has
 * nothing to describe — no ladder and no options.
 */
export function traitsSummary(
  model: Model | undefined,
  reasoning: ReasoningLevel | null,
  selections: Readonly<Record<string, unknown>>,
): string | null {
  const parts: string[] = [];
  if (reasoning !== null) {
    parts.push(reasoningLabel(reasoning));
  }
  for (const option of model?.options ?? []) {
    const saved = selections[option.id];
    const picked =
      typeof saved === "string" && option.choices.some((choice) => choice.id === saved)
        ? saved
        : option.defaultChoice;
    const choice = option.choices.find((candidate) => candidate.id === picked);
    if (choice !== undefined) {
      parts.push(choice.label);
    }
  }
  return parts.length === 0 ? null : parts.join(" · ");
}

/**
 * `pickers.rs::offered_options` — keep only the picks `model` still offers.
 * Remembered picks outlive the model they were made on, and harnesses apply
 * some options blindly (Claude appends `[1m]` to any model id when
 * `contextWindow` is "1m").
 */
export function offeredOptions(
  model: Model,
  selections: Readonly<Record<string, unknown>>,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [id, choice] of Object.entries(selections)) {
    const option = model.options.find((entry) => entry.id === id);
    if (option === undefined || typeof choice !== "string") {
      continue;
    }
    if (option.choices.some((entry) => entry.id === choice)) {
      out[id] = choice;
    }
  }
  return out;
}

/**
 * `pickers.rs::traits_customized` — whether any trait departs from its
 * default; the chip's suffix brightens only then, so a customized run still
 * stands out now that the summary always names the effective choices.
 */
export function traitsCustomized(
  model: Model | undefined,
  reasoning: ReasoningLevel | null,
  ladder: readonly ReasoningLevel[],
  selections: Readonly<Record<string, unknown>>,
): boolean {
  if (reasoning !== defaultReasoning(ladder)) {
    return true;
  }
  return (model?.options ?? []).some((option) => {
    const saved = selections[option.id];
    return (
      typeof saved === "string" &&
      saved !== option.defaultChoice &&
      option.choices.some((choice) => choice.id === saved)
    );
  });
}

/**
 * `traitsActive` kept for the pre-parity call sites, now comparing against
 * `defaultReasoning(ladder)` (gap row 9) instead of the ladder's first entry
 * — wrong for any ladder that doesn't start with High.
 */
export function traitsActive(
  model: Model | undefined,
  reasoning: ReasoningLevel | null,
  selections: Readonly<Record<string, unknown>>,
): boolean {
  return traitsCustomized(model, reasoning, model?.reasoningLevels ?? [], selections);
}
