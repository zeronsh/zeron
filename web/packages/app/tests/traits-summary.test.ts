import { describe, expect, it } from "vitest";
import type { HarnessDescriptor, Model, ReasoningLevel } from "@zeron/proto";
import {
  clampReasoning,
  defaultReasoning,
  effectiveReasoningLadder,
  offeredOptions,
  reasoningLabel,
  traitsCustomized,
  traitsSummary,
} from "../src/lib/traits-summary";

const LADDER: readonly ReasoningLevel[] = ["medium", "high"];

const MODEL: Model = {
  id: "opus",
  label: "Opus",
  description: null,
  reasoningLevels: ["medium", "high"],
  options: [
    {
      id: "context",
      label: "Context window",
      choices: [
        { id: "standard", label: "Standard" },
        { id: "1m", label: "1M" },
      ],
      defaultChoice: "standard",
    },
    {
      id: "speed",
      label: "Speed",
      choices: [
        { id: "normal", label: "Normal" },
        { id: "fast", label: "Fast" },
      ],
      defaultChoice: "normal",
    },
  ],
};

describe("reasoning ladder helpers", () => {
  it("default_reasoning_prefers_high_then_medium", () => {
    // Recommended default is High (user-corrected), even on full ladders.
    expect(defaultReasoning(["low", "medium", "high", "xhigh", "max", "ultracode", "ultrathink"])).toBe("high");
    expect(defaultReasoning(["low", "medium", "high", "max"])).toBe("high");
    // No High: Medium.
    expect(defaultReasoning(["minimal", "low", "medium"])).toBe("medium");
    // Neither offered: first entry.
    expect(defaultReasoning(["minimal", "low"])).toBe("minimal");
    // Ladder-less model (Haiku): no reasoning at all.
    expect(defaultReasoning([])).toBeNull();
  });

  it("clamp_reasoning_keeps_offered_levels_and_heals_foreign_ones", () => {
    const ladder: readonly ReasoningLevel[] = ["low", "medium", "high", "max"];
    // A pick the ladder offers survives.
    expect(clampReasoning("max", ladder)).toBe("max");
    // A remembered level the new model doesn't offer heals to its default.
    expect(clampReasoning("xhigh", ladder)).toBe("high");
    // No pick at all resolves to the concrete default too.
    expect(clampReasoning(null, ladder)).toBe("high");
    expect(clampReasoning("high", [])).toBeNull();
  });

  it("reasoning_label_spells_the_levels", () => {
    expect(reasoningLabel("minimal")).toBe("Minimal");
    expect(reasoningLabel("high")).toBe("High");
    expect(reasoningLabel("xhigh")).toBe("X-High");
    expect(reasoningLabel("ultrathink")).toBe("Ultrathink");
  });
});

describe("effective_reasoning_ladder_prefers_model_then_descriptor", () => {
  const model = (reasoningLevels: ReasoningLevel[]): Model => ({
    id: "m",
    label: "M",
    reasoningLevels,
    options: [],
  });
  const descriptor = (reasoningLevels: ReasoningLevel[]): HarnessDescriptor => ({
    id: "claude-code",
    name: "Claude",
    supportsSteering: true,
    steeringMode: "step-boundary",
    reasoningLevels,
    installed: true,
    enabled: true,
  });

  it("uses the model's nonempty ladder in its advertised order — never a union", () => {
    // pickers.rs:1551-1552: extra harness levels must not merge in.
    expect(effectiveReasoningLadder(model(["low", "high"]), descriptor(["medium", "max"]))).toEqual([
      "low",
      "high",
    ]);
  });

  it("falls back to the matching descriptor when the model's list is empty", () => {
    // pickers.rs:1554-1561: Haiku's [] against Claude's advertised ladder.
    expect(effectiveReasoningLadder(model([]), descriptor(["low", "medium", "high"]))).toEqual([
      "low",
      "medium",
      "high",
    ]);
  });

  it("keeps the model's list when no descriptor matches", () => {
    expect(effectiveReasoningLadder(model(["low", "medium"]), null)).toEqual(["low", "medium"]);
    expect(effectiveReasoningLadder(model(["low"]), descriptor([]))).toEqual(["low"]);
  });

  it("no selected model means no effective ladder, even with a nonempty descriptor", () => {
    // pickers.rs:1548-1550: nothing resolves before the model does — the
    // loading UI must not invent model capabilities.
    expect(effectiveReasoningLadder(undefined, descriptor(["low", "medium", "high"]))).toEqual([]);
    expect(effectiveReasoningLadder(null, descriptor(["low", "medium", "high"]))).toEqual([]);
  });

  it("both empty means no ladder (the Reasoning section omits, options render independently)", () => {
    expect(effectiveReasoningLadder(model([]), descriptor([]))).toEqual([]);
  });

  it("a null or foreign selection heals to the native default of the resolved ladder", () => {
    // The composition every consumer relies on: the effective ladder feeds
    // clamp_reasoning/default_reasoning unchanged.
    const fallback = effectiveReasoningLadder(model([]), descriptor(["low", "medium", "high"]));
    expect(clampReasoning(null, fallback)).toBe("high");
    expect(clampReasoning("ultra", fallback)).toBe("high");
    expect(clampReasoning("low", fallback)).toBe("low");
  });
});

describe("traits", () => {
  it("traits_summary_formats_non_defaults", () => {
    const selections: Record<string, unknown> = { context: "1m", speed: "fast" };
    expect(traitsSummary(MODEL, "high", selections)).toBe("High · 1M · Fast");
    // All defaults: the effective choices still read on the trigger.
    expect(traitsSummary(MODEL, null, {})).toBe("Standard · Normal");
    // A saved choice the option no longer offers falls back to the default
    // label rather than vanishing or echoing a stale id.
    expect(traitsSummary(MODEL, null, { speed: "ludicrous" })).toBe("Standard · Normal");
    // Reasoning shows without a model too.
    expect(traitsSummary(undefined, "ultrathink", {})).toBe("Ultrathink");
    // Nothing to describe.
    expect(traitsSummary(undefined, null, {})).toBeNull();
  });

  it("offered_options_keeps_only_picks_the_model_still_offers", () => {
    // Remembered picks drop what the model doesn't offer before sending.
    const remembered: Record<string, unknown> = {
      context: "1m",
      speed: "ludicrous",
      fastMode: "on",
    };
    expect(offeredOptions(MODEL, remembered)).toEqual({ context: "1m" });
  });

  it("traits_customized_flags_only_real_departures", () => {
    const selections: Record<string, unknown> = { context: "1m", speed: "fast" };
    // Customized (bright trigger) only when something departs from its
    // default: default-choice selections and the default reasoning level
    // don't count; stale ids don't either.
    expect(traitsCustomized(MODEL, "high", LADDER, selections)).toBe(true);
    expect(traitsCustomized(MODEL, defaultReasoning(LADDER), LADDER, {})).toBe(false);
    expect(
      traitsCustomized(MODEL, defaultReasoning(LADDER), LADDER, { speed: "normal" }),
    ).toBe(false);
    expect(traitsCustomized(MODEL, defaultReasoning(LADDER), LADDER, { speed: "ludicrous" })).toBe(false);
    // A non-default level on a medium-first ladder counts.
    expect(traitsCustomized(MODEL, "medium", LADDER, {})).toBe(true);
  });
});
