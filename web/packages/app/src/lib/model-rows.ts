import type { HarnessDescriptor, HarnessId, Model, ReasoningLevel } from "@zeron/proto";
import { matchRank } from "./picker-search";
import { defaultReasoning, reasoningLabel } from "./traits-summary";

/**
 * The model-list logic ported from `crates/ui/src/pickers.rs`:
 * `default_model` (`:165`), `scoped_model_rows` (`:3811`),
 * `normalize_model_rows` (`:3919`), `visible_harnesses`/`offered_harnesses`
 * (`:4031-4069`), and the `workspace_footer_row` flex model (`:2341-2422`)
 * as a pure function so the footer pair's leading-edge/gap invariants are
 * unit-testable (`workspace_footer_pair_keeps_its_leading_edge_and_gap`,
 * `pickers.rs:4340`).
 */

/** `default_model` — both curated catalogs lead with the flagship. */
export function defaultModel(models: readonly Model[]): Model | null {
  return models[0] ?? null;
}

// ---------------------------------------------------------------------------
// Harness visibility / offer
// ---------------------------------------------------------------------------

/**
 * `ZERON_HARNESS=mock` is a desktop-only dev rig the web deliberately does
 * not port (the ticket's Do-not list), so the mock opt-in is always false
 * here; the function still takes it so the tests can exercise both arms.
 */
export function visibleHarnessesImpl(
  list: readonly HarnessDescriptor[],
  allowMock: boolean,
): HarnessDescriptor[] {
  if (allowMock) {
    return [...list];
  }
  const real = list.filter((descriptor) => descriptor.id !== "mock");
  return real.length > 0 ? real : [...list];
}

/** Production pickers hide the mock harness unless it is literally all there is. */
export function visibleHarnesses(list: readonly HarnessDescriptor[]): HarnessDescriptor[] {
  return visibleHarnessesImpl(list, false);
}

/**
 * `registry.rs::descriptor_enabled` — a null flag falls back to detection,
 * which keeps the opt-in harnesses OFF: enabling antigravity downloads a
 * large server and runs a browser sign-in, so detection alone must never
 * set it off (registry.rs `opt_in`).
 */
export function descriptorEnabled(descriptor: HarnessDescriptor): boolean {
  if (descriptor.enabled !== null && descriptor.enabled !== undefined) {
    return descriptor.enabled;
  }
  return descriptor.installed && descriptor.id !== "mock" && descriptor.id !== "antigravity";
}

/**
 * What the composer actually offers: `visibleHarnesses` narrowed to the
 * catalog device's enabled set AND installed CLIs. There is NO fallback: a
 * catalog where nothing is both enabled and installed offers nothing, and
 * the composer surfaces the no-agents empty state and blocks new sends.
 */
export function offeredHarnessesImpl(
  list: readonly HarnessDescriptor[],
  allowMock: boolean,
): HarnessDescriptor[] {
  return visibleHarnessesImpl(list, allowMock).filter(
    (descriptor) => descriptor.installed && (descriptorEnabled(descriptor) || (allowMock && descriptor.id === "mock")),
  );
}

export function offeredHarnesses(list: readonly HarnessDescriptor[]): HarnessDescriptor[] {
  return offeredHarnessesImpl(list, false);
}

// ---------------------------------------------------------------------------
// The scoped model list
// ---------------------------------------------------------------------------

export type ModelRail = "favorites" | "harness";

/** One flattened row: the model plus the harness it came from (`ModelRowData`). */
export interface ModelRowData {
  readonly harness: HarnessId;
  readonly harnessName: string;
  readonly model: Model;
}

/**
 * `scoped_model_rows` (`pickers.rs:3811-3896`). The query never leaves the
 * viewed tab: Favorites scopes to starred rows, Harness to the effective
 * harness's catalog. With a query, rows rank by
 * `min(matchRank(query, label), matchRank(query, "{description} {label}") + 2)`
 * (a description-only hit still finds opencode's provider attribution inside
 * one tab), ties breaking starred-first then input order. Without one:
 * Favorites = every starred row in descriptor order; Harness = the effective
 * catalog partitioned so starred rows float to the top (stable within each
 * partition).
 */
export function scopedModelRows(
  query: string,
  rail: ModelRail,
  effective: HarnessId | null,
  descriptors: readonly HarnessDescriptor[],
  modelsFor: (harness: HarnessId) => readonly Model[] | null,
  isFavorite: (harness: HarnessId, model: string) => boolean,
): ModelRowData[] {
  const rows: ModelRowData[] = [];
  const row = (descriptor: HarnessDescriptor, model: Model): ModelRowData => ({
    harness: descriptor.id,
    harnessName: descriptor.name,
    model,
  });
  const inScope = (descriptor: HarnessDescriptor, model: Model): boolean =>
    rail === "favorites"
      ? isFavorite(descriptor.id, model.id)
      : descriptor.id === effective;

  if (query.trim().length > 0) {
    const ranked: { rank: number; unstarred: 0 | 1; inputIx: number; row: ModelRowData }[] = [];
    let inputIx = 0;
    for (const descriptor of descriptors) {
      const models = modelsFor(descriptor.id);
      if (models === null) {
        continue;
      }
      for (const model of models) {
        if (!inScope(descriptor, model)) {
          inputIx += 1;
          continue;
        }
        const byLabel = matchRank(query, model.label);
        const byDescription = matchRank(query, `${model.description ?? ""} ${model.label}`);
        const rank = minDefined(byLabel, byDescription === null ? null : byDescription + 2);
        if (rank !== null) {
          ranked.push({
            rank,
            unstarred: isFavorite(descriptor.id, model.id) ? 0 : 1,
            inputIx,
            row: row(descriptor, model),
          });
        }
        inputIx += 1;
      }
    }
    ranked.sort((a, b) => a.rank - b.rank || a.unstarred - b.unstarred || a.inputIx - b.inputIx);
    return ranked.map((entry) => entry.row);
  }

  if (rail === "favorites") {
    for (const descriptor of descriptors) {
      const models = modelsFor(descriptor.id);
      if (models === null) {
        continue;
      }
      for (const model of models) {
        if (isFavorite(descriptor.id, model.id)) {
          rows.push(row(descriptor, model));
        }
      }
    }
    return rows;
  }

  const descriptor = descriptors.find((entry) => entry.id === effective);
  const models = descriptor === undefined ? null : modelsFor(descriptor.id);
  if (descriptor === undefined || models === null) {
    return rows;
  }
  const starred: ModelRowData[] = [];
  const rest: ModelRowData[] = [];
  for (const model of models) {
    (isFavorite(descriptor.id, model.id) ? starred : rest).push(row(descriptor, model));
  }
  return [...starred, ...rest];
}

function minDefined(a: number | null, b: number | null): number | null {
  if (a === null) {
    return b;
  }
  if (b === null) {
    return a;
  }
  return Math.min(a, b);
}

// ---------------------------------------------------------------------------
// normalize_model_rows
// ---------------------------------------------------------------------------

/**
 * `zeron_harness::claude::catalog::static_models` (`catalog.rs:152-207`) —
 * the curated Claude catalog, flagship-first. Only the ids and labels matter
 * here: `normalizeModelRows` borrows them so the version number always shows
 * ("Opus 5", not the wire's terse "Opus" alias).
 */
const CLAUDE_CATALOG: readonly { id: string; label: string }[] = [
  { id: "claude-fable-5-1", label: "Fable 5.1" },
  { id: "claude-fable-5", label: "Fable 5" },
  { id: "claude-opus-5", label: "Opus 5" },
  { id: "claude-opus-4-8", label: "Opus 4.8" },
  { id: "claude-opus-4-7", label: "Opus 4.7" },
  { id: "claude-sonnet-5", label: "Sonnet 5" },
  { id: "claude-haiku-4-5", label: "Haiku 4.5" },
];

function strip1m(id: string): string | null {
  if (id.endsWith("[1m]")) {
    return id.slice(0, -"[1m]".length);
  }
  if (id.endsWith("-1m")) {
    return id.slice(0, -"-1m".length);
  }
  return null;
}

function normId(id: string): string {
  return id
    .split("")
    .filter((char) => /[0-9a-zA-Z]/.test(char))
    .join("")
    .toLowerCase();
}

/**
 * Display-side model-list hygiene for catalogs served by OLDER engines (the
 * space's device may run any engine version) — `normalize_model_rows`
 * (`pickers.rs:3919-3994`). Idempotent over already-clean lists:
 *
 * 1. The `"default"` alias row drops when any real row exists.
 * 2. An id ending in `[1m]`/`-1m`: the variant drops when the bare base is
 *    also listed; otherwise it folds to the base id, the `" (…)"` label
 *    suffix strips, and a `contextWindow` option (200K/1M, default 1M) is
 *    appended unless one exists.
 * 3. For `claudeCode`, labels adopt the curated catalog's — exact normalized
 *    id match, else (bare alphabetic aliases like `opus`) the first
 *    flagship-ordered family row whose normalized id contains the alias.
 */
export function normalizeModelRows(harness: HarnessId, models: readonly Model[]): Model[] {
  const catalog = harness === "claude-code" ? CLAUDE_CATALOG : [];
  const curatedLabel = (id: string): string | null => {
    const idNorm = normId(id);
    if (idNorm.length === 0) {
      return null;
    }
    const exact = catalog.find((entry) => normId(entry.id) === idNorm);
    if (exact !== undefined) {
      return exact.label;
    }
    if (!/^[a-z]+$/.test(idNorm)) {
      return null;
    }
    const family = catalog.find((entry) => normId(entry.id).includes(idNorm));
    return family === undefined ? null : family.label;
  };

  const ids = models.map((model) => model.id);
  const hasReal = ids.some((id) => id.toLowerCase() !== "default");

  const out: Model[] = [];
  for (const source of models) {
    let model: Model = source;
    if (hasReal && model.id.toLowerCase() === "default") {
      continue;
    }
    const base = strip1m(model.id);
    if (base !== null) {
      if (ids.some((other) => other === base)) {
        continue;
      }
      let label = model.label;
      const at = label.lastIndexOf(" (");
      if (at >= 0 && label.endsWith(")")) {
        label = label.slice(0, at).trimEnd();
      }
      const options =
        model.options.some((option) => option.id === "contextWindow")
          ? model.options
          : [
              ...model.options,
              {
                id: "contextWindow",
                label: "Context Window",
                choices: [
                  { id: "200k", label: "200K" },
                  { id: "1m", label: "1M" },
                ],
                defaultChoice: "1m",
              },
            ];
      model = { ...model, id: base, label, options };
    }
    const label = curatedLabel(model.id);
    if (label !== null) {
      model = { ...model, label };
    }
    out.push(model);
  }
  return out;
}

// ---------------------------------------------------------------------------
// workspace_footer_row — the flex model
// ---------------------------------------------------------------------------

/** One chip's box in the footer row's flex model. */
export interface FooterChipBox {
  readonly left: number;
  readonly right: number;
  readonly top: number;
}

/** The footer row's flex model (`workspaceFooterRow`, pickers.rs:2341-2422). */
export interface WorkspaceFooterLayout {
  readonly checkout: FooterChipBox;
  readonly ref: FooterChipBox;
  /** Right edge of the trailing cluster (change-request badge + usage). */
  readonly trailingRight: number;
}

/**
 * The pure layout of `workspace_footer_row`: a `100%`-wide flex row
 * (padding-inline `padding`), the checkout + ref chips leading with `gap`
 * between them, a `flex: 1` spring, then the trailing cluster. Ported so
 * `workspace_footer_pair_keeps_its_leading_edge_and_gap` (`pickers.rs:4340`)
 * runs as a plain assertion: the pair keeps its leading edge across
 * container widths, the inter-chip gap is exactly 4px, the chips share a
 * top, and the trailing cluster's right edge sits `width - 2 * padding`
 * from the pair's left edge.
 */
export function workspaceFooterLayout(
  containerWidth: number,
  options: {
    readonly padding?: number;
    readonly gap?: number;
    readonly checkoutWidth: number;
    readonly refWidth: number;
    readonly trailingWidth: number;
  },
): WorkspaceFooterLayout {
  const padding = options.padding ?? 10;
  const gap = options.gap ?? 4;
  const checkout = { left: padding, right: padding + options.checkoutWidth, top: 0 };
  const ref = { left: checkout.right + gap, right: checkout.right + gap + options.refWidth, top: 0 };
  return {
    checkout,
    ref,
    trailingRight: containerWidth - padding,
  };
}

/** The reasoning ladder type re-exported for consumers of this module. */
export type { ReasoningLevel };

// ---------------------------------------------------------------------------
// setting_groups — the nested model settings (9a4757be)
// ---------------------------------------------------------------------------

/** The reasoning ladder's group id; option groups use the option id. */
export const REASONING_SETTING_ID = "reasoning";

/** One choice inside a model setting's nested menu (`SettingChoice`). */
export interface SettingChoice {
  readonly label: string;
  /** The choice's id — empty for reasoning levels. */
  readonly value: string;
  readonly reasoning: ReasoningLevel | null;
  readonly selected: boolean;
  readonly isDefault: boolean;
}

/** One model setting: the reasoning ladder or one offered option. */
export interface SettingGroup {
  readonly id: string;
  readonly label: string;
  readonly choices: readonly SettingChoice[];
}

/**
 * `setting_groups` (pickers.rs, upstream 9a4757be) — the traits tray's
 * trigger rows: the reasoning ladder first, then every option that offers
 * choices. A group's selected choice is the saved pick (draft or chat
 * config) or the option's default, the same resolution the chip's traits
 * summary shows; the desktop never validates the saved string here either.
 */
export function settingGroups(
  model: Model | undefined,
  ladder: readonly ReasoningLevel[],
  reasoning: ReasoningLevel | null,
  selections: Readonly<Record<string, unknown>>,
): SettingGroup[] {
  const groups: SettingGroup[] = [];
  if (ladder.length > 0) {
    const fallback = defaultReasoning(ladder);
    groups.push({
      id: REASONING_SETTING_ID,
      label: "Reasoning",
      choices: ladder.map((level) => ({
        label: reasoningLabel(level),
        value: "",
        reasoning: level,
        selected: reasoning === level,
        isDefault: fallback === level,
      })),
    });
  }
  for (const option of model?.options ?? []) {
    if (option.choices.length === 0) {
      continue;
    }
    const saved = selections[option.id];
    const selected = typeof saved === "string" ? saved : option.defaultChoice;
    groups.push({
      id: option.id,
      label: option.label,
      choices: option.choices.map((choice) => ({
        label: choice.label,
        value: choice.id,
        reasoning: null,
        selected: selected === choice.id,
        isDefault: option.defaultChoice === choice.id,
      })),
    });
  }
  return groups;
}
