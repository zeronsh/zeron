import type { Chat, HarnessDescriptor, HarnessId, Model, ReasoningLevel } from "@zeron/proto";
import type { DraftConfig, DraftConfigUpdate } from "./composer-actions";
import { buildChatConfig } from "./composer-actions";
import type { StorageLike } from "./engine-store";
import { clampReasoning, effectiveReasoningLadder, offeredOptions } from "./traits-summary";

/**
 * The native reasoning normalization (`pickers.rs:1493-1512`): clamp ONLY
 * against a NONEMPTY effective ladder. An empty one means the metadata
 * hasn't resolved — the stored preference is retained verbatim, never
 * destructively cleared.
 */
function normalizeReasoning(level: ReasoningLevel | null, ladder: readonly ReasoningLevel[]): ReasoningLevel | null {
  return ladder.length > 0 ? clampReasoning(level, ladder) : level;
}

/**
 * Sensible defaults when a fresh chat has no `ChatConfig` yet, derived from
 * the loaded harness/model catalogs. Picked fields are intentionally
 * narrow: the user's first picker choice is the first enabled harness and
 * its first model, with `workspace-write` sandbox. The composer only
 * invokes this when the catalog has actually loaded.
 *
 * A fresh chat also seeds the sticky picks — the native new-chat resolution
 * (`pickers.rs::effective_harness`/`effective_model_id`, 713-745): the
 * remembered harness while the catalog still lists it (seeded even while
 * the catalog is still empty, so the reconciliation lands on it once the
 * rows arrive), else the first enabled harness; then the remembered model
 * for that harness while the list still offers it, else the first model. A
 * remembered pick the catalog no longer offers falls back to the default
 * exactly as if nothing had been remembered.
 *
 * Reasoning follows the native precedence (`pickers.rs::effective_reasoning`,
 * 762-775): a fresh chat has NO explicit draft value, so the new-chat
 * `remembered` last-used level is the preference layer — kept verbatim while
 * no effective ladder has resolved (retained, never destructively cleared),
 * and once one has, clamped against it (an offered level stays; anything else
 * heals to the native default: High, else Medium, else the first advertised
 * level). The old first-level-or-"medium" seed invented an explicit value
 * that bypassed that default selection; it is gone.
 */
export function defaultDraft(
  catalog: readonly HarnessDescriptor[],
  models: readonly Model[],
  remembered: ReasoningLevel | null = null,
  sticky: StickyDraftPicks | null = null,
): DraftConfig {
  const harness =
    (sticky !== null ? catalog.find((row) => row.id === sticky.harness) : undefined) ??
    catalog.find((row) => row.enabled !== false) ??
    catalog[0] ??
    null;
  // An empty catalog seeds the remembered harness itself (the models have
  // not landed — a null model, seeded by the reconciliation once they do).
  const harnessId: HarnessId = harness?.id ?? sticky?.harness ?? "claude-code";
  // The remembered model only applies while the resolved harness IS the
  // remembered one — the model list is per-harness.
  const rememberedModel = sticky !== null && sticky.harness === harnessId ? sticky.model : null;
  const modelRow = models.find((row) => row.id === rememberedModel?.id) ?? models[0] ?? null;
  const model = modelRow?.id ?? null;
  const ladder = effectiveReasoningLadder(modelRow, harness);
  const reasoning = normalizeReasoning(remembered, ladder);
  return {
    harness: harnessId,
    model,
    reasoning,
    sandbox: "workspace-write",
    modelOptions: {},
  };
}

/**
 * Initialize the composer's draft from the chat's persisted ChatConfig (may
 * be null). `remembered` is the sticky last-used reasoning level and `sticky`
 * the remembered harness/model pair — both consulted only for a fresh chat
 * (an established chat's persisted config is the explicit layer and wins
 * outright, matching the native precedence).
 */
export function draftFromChat(
  chat: Chat,
  catalog: readonly HarnessDescriptor[],
  models: readonly Model[],
  remembered: ReasoningLevel | null = null,
  sticky: StickyDraftPicks | null = null,
): DraftConfig {
  const config = chat.config;
  if (config === null) {
    return defaultDraft(catalog, models, remembered, sticky);
  }
  const harnessId: HarnessId = config.harness;
  const reasoning = config.reasoning;
  const modelOptions: Record<string, unknown> = { ...(config.modelOptions ?? {}) };
  return {
    harness: harnessId,
    model: config.model,
    reasoning,
    sandbox: config.sandbox,
    modelOptions,
  };
}

/** True when a chat has a persisted ChatConfig (locks the harness picker). */
export function isHarnessLocked(chat: Chat | null): boolean {
  return chat !== null && chat.config !== null;
}

/** Two drafts differ when any user-facing field changed. */
export function draftsEqual(a: DraftConfig, b: DraftConfig): boolean {
  return JSON.stringify(buildChatConfig(a)) === JSON.stringify(buildChatConfig(b));
}

// ---------------------------------------------------------------------------
// Sticky picks — the web peer of `settings/composer.rs::ComposerDefaults`
// ---------------------------------------------------------------------------

/** One remembered model: the id plus the label the chip can fall back to. */
export interface RememberedModel {
  readonly id: string;
  readonly label: string;
}

/**
 * The sticky picks a fresh chat pre-selects: the remembered harness plus the
 * remembered model for it — the `modelByHarness` entry the composer resolves
 * via `rememberedModelFor`. Null when nothing was remembered.
 */
export interface StickyDraftPicks {
  readonly harness: HarnessId;
  readonly model: RememberedModel | null;
}

/** One starred model, kept in starring order. */
export interface ModelFavorite {
  readonly harness: HarnessId;
  readonly model: string;
}

/**
 * `ComposerDefaults` (`settings/composer.rs`) — the sticky-picks file. The
 * desktop loads it synchronously before first paint (temp-file + rename
 * persistence); the web's equivalent is one origin-scoped `localStorage` key
 * read once at module scope, so the remembered harness/model/reasoning show
 * immediately. Fields per the desktop: `harness`, `modelByHarness`
 * (`{id,label}` per harness), `reasoning`, `modelOptionsByModel` (harness →
 * model id → option map), `modelLabels` (id → label cache, the chip's
 * fallback while a list loads), `device`, `project`, `noProject`,
 * `favorites` (in starring order).
 */
export interface ComposerDefaults {
  readonly harness: HarnessId | null;
  readonly modelByHarness: Readonly<Record<string, RememberedModel>>;
  readonly reasoning: ReasoningLevel | null;
  readonly modelOptionsByModel: Readonly<Record<string, Readonly<Record<string, string>>>>;
  readonly modelLabels: Readonly<Record<string, string>>;
  readonly device: string | null;
  readonly project: string | null;
  readonly noProject: boolean;
  readonly favorites: readonly ModelFavorite[];
}

const COMPOSER_DEFAULTS_KEY = "zeron.composer-defaults.v1";

const EMPTY_DEFAULTS: ComposerDefaults = {
  harness: null,
  modelByHarness: {},
  reasoning: null,
  modelOptionsByModel: {},
  modelLabels: {},
  device: null,
  project: null,
  noProject: false,
  favorites: [],
};

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

/** Per-field healing: one wrong key never discards the rest. */
export function healComposerDefaults(value: unknown): ComposerDefaults {
  const raw = asRecord(value);
  const modelByHarness: Record<string, RememberedModel> = {};
  for (const [harness, entry] of Object.entries(asRecord(raw.modelByHarness))) {
    const model = asRecord(entry);
    if (typeof model.id === "string" && typeof model.label === "string") {
      modelByHarness[harness] = { id: model.id, label: model.label };
    }
  }
  const modelOptionsByModel: Record<string, Record<string, string>> = {};
  for (const [modelId, options] of Object.entries(asRecord(raw.modelOptionsByModel))) {
    const picks: Record<string, string> = {};
    for (const [optionId, choice] of Object.entries(asRecord(options))) {
      if (typeof choice === "string") {
        picks[optionId] = choice;
      }
    }
    if (Object.keys(picks).length > 0) {
      modelOptionsByModel[modelId] = picks;
    }
  }
  const modelLabels: Record<string, string> = {};
  for (const [id, label] of Object.entries(asRecord(raw.modelLabels))) {
    if (typeof label === "string" && label.length > 0) {
      modelLabels[id] = label;
    }
  }
  const favorites: ModelFavorite[] = [];
  if (Array.isArray(raw.favorites)) {
    for (const entry of raw.favorites) {
      const favorite = asRecord(entry);
      if (typeof favorite.harness === "string" && typeof favorite.model === "string") {
        favorites.push({ harness: favorite.harness as HarnessId, model: favorite.model });
      }
    }
  }
  const reasoning = raw.reasoning;
  return {
    harness: typeof raw.harness === "string" ? (raw.harness as HarnessId) : null,
    modelByHarness,
    reasoning:
      typeof reasoning === "string"
        ? ([
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "ultra",
            "ultracode",
            "ultrathink",
          ] as const).includes(reasoning as ReasoningLevel)
          ? (reasoning as ReasoningLevel)
          : null
        : null,
    modelOptionsByModel,
    modelLabels,
    device: typeof raw.device === "string" ? raw.device : null,
    project: typeof raw.project === "string" ? raw.project : null,
    noProject: raw.noProject === true,
    favorites,
  };
}

function defaultStorage(): StorageLike {
  try {
    return (globalThis as { localStorage?: StorageLike }).localStorage ?? memoryStorage();
  } catch {
    return memoryStorage();
  }
}

/** A writable view of a defaults patch — `Partial` keeps readonly modifiers. */
type DefaultsPatch = { -readonly [K in keyof ComposerDefaults]?: ComposerDefaults[K] };

function memoryStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

/**
 * The sticky-picks store. One snapshot, healed on write, subscribed to by
 * the pickers so a star or a remembered pick repaints immediately.
 */
export class ComposerDefaultsStore {
  readonly #storage: StorageLike;
  #defaults: ComposerDefaults;
  #serialized: string;
  readonly #listeners = new Set<() => void>();

  constructor(options: { storage?: StorageLike } = {}) {
    this.#storage = options.storage ?? defaultStorage();
    let raw: string | null = null;
    try {
      raw = this.#storage.getItem(COMPOSER_DEFAULTS_KEY);
    } catch {
      raw = null;
    }
    this.#defaults =
      raw === null ? EMPTY_DEFAULTS : healComposerDefaults(safeParse(raw) ?? EMPTY_DEFAULTS);
    this.#serialized = JSON.stringify(this.#defaults);
  }

  getSnapshot(): ComposerDefaults {
    return this.#defaults;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** Apply a patch, heal, persist, notify — the desktop's atomic save. */
  update(patch: DefaultsPatch): void {
    const next = healComposerDefaults({ ...this.#defaults, ...patch });
    const serialized = JSON.stringify(next);
    if (serialized === this.#serialized) {
      return;
    }
    this.#defaults = next;
    this.#serialized = serialized;
    try {
      this.#storage.setItem(COMPOSER_DEFAULTS_KEY, serialized);
    } catch {
      // Private mode or a full quota — a sticky pick is not worth a crash.
    }
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

function safeParse(raw: string): unknown {
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return null;
  }
}

/** The one store for this page load; loaded synchronously before first paint. */
export const composerDefaults = new ComposerDefaultsStore();

// — typed accessors over the snapshot ————————————————————————————————

/** The remembered model for a harness (id + label), or null. */
export function rememberedModelFor(harness: HarnessId): RememberedModel | null {
  return composerDefaults.getSnapshot().modelByHarness[harness] ?? null;
}

/** The remembered label for a bare model id (the chip's loading fallback). */
export function rememberedLabelFor(modelId: string): string | null {
  return composerDefaults.getSnapshot().modelLabels[modelId] ?? null;
}

/** The remembered per-model option picks (already offered-filtered by writes). */
export function rememberedModelOptions(harness: HarnessId, modelId: string): Record<string, string> {
  return { ...(composerDefaults.getSnapshot().modelOptionsByModel[`${harness}/${modelId}`] ?? {}) };
}

/** True when (harness, model) is starred. */
export function isModelFavorite(harness: HarnessId, model: string): boolean {
  return composerDefaults
    .getSnapshot()
    .favorites.some((favorite) => favorite.harness === harness && favorite.model === model);
}

/**
 * `toggle_model_favorite` — the shared favorites list (not per-chat). New
 * stars append (starring order); unstarring removes in place.
 */
export function toggleModelFavorite(harness: HarnessId, model: string, label?: string): boolean {
  const snapshot = composerDefaults.getSnapshot();
  const starred = isModelFavorite(harness, model);
  const favorites = starred
    ? snapshot.favorites.filter((favorite) => !(favorite.harness === harness && favorite.model === model))
    : [...snapshot.favorites, { harness, model }];
  const patch: DefaultsPatch = { favorites };
  if (!starred && typeof label === "string" && label.length > 0) {
    patch.modelLabels = { ...snapshot.modelLabels, [model]: label };
  }
  composerDefaults.update(patch);
  return !starred;
}

/** `pick_harness` — remember the harness; a changed harness clears its model pick. */
export function rememberHarness(harness: HarnessId): void {
  composerDefaults.update({ harness });
}

/** `pick_model` — remember (harness, id, label) and cache the label. */
export function rememberModel(harness: HarnessId, id: string, label: string): void {
  const snapshot = composerDefaults.getSnapshot();
  composerDefaults.update({
    modelByHarness: { ...snapshot.modelByHarness, [harness]: { id, label } },
    modelLabels: { ...snapshot.modelLabels, [id]: label },
  });
}

/** `pick_reasoning` — the remembered global default (not per-harness). */
export function rememberReasoning(level: ReasoningLevel | null): void {
  composerDefaults.update({ reasoning: level });
}

/** `pick_option` — store the per-model pick; a default choice removes the key. */
export function rememberModelOption(
  harness: HarnessId,
  modelId: string,
  optionId: string,
  choiceId: string,
  isDefault: boolean,
): void {
  const snapshot = composerDefaults.getSnapshot();
  const key = `${harness}/${modelId}`;
  const picks: Record<string, string> = { ...(snapshot.modelOptionsByModel[key] ?? {}) };
  if (isDefault) {
    delete picks[optionId];
  } else {
    picks[optionId] = choiceId;
  }
  const modelOptionsByModel = { ...snapshot.modelOptionsByModel };
  if (Object.keys(picks).length > 0) {
    modelOptionsByModel[key] = picks;
  } else {
    delete modelOptionsByModel[key];
  }
  composerDefaults.update({ modelOptionsByModel });
}

/** `remember_target` — the new-chat device/project defaults. */
export function rememberTarget(device: string | null, project: string | null, noProject: boolean): void {
  composerDefaults.update({ device, project, noProject });
}

/**
 * The canvas's "Don't work in a project" pick — the web peer of the shell's
 * rule (shell.rs:1767-1774): restore the opt-out target AND take the
 * sidebar's space filter, because "retaining a project filter would hide
 * the session on its first send" — a projectless row carries no spaceId,
 * and the active list is narrowed by the filter. The sidebar store is a
 * parameter so tests can drive a fresh `SidebarStore`; the footer passes
 * the app singleton (whose setter persists through ui-settings, so the
 * clear survives refresh).
 */
export function rememberNoProject(
  device: string | null,
  sidebar: { setSpaceFilter(spaceId: string | null): void },
): void {
  rememberTarget(device, null, true);
  sidebar.setSpaceFilter(null);
}

// ---------------------------------------------------------------------------
// Per-chat text drafts (composer.rs `drafts: HashMap<chat_key, String>`)
// ---------------------------------------------------------------------------

/**
 * The per-chat draft text map, swapped on navigation (`composer.rs`'s
 * `self.drafts`, key `""` = the new-chat canvas — the canvas route itself is
 * ticket 15). Module-scoped so drafts survive a composer unmount, exactly as
 * the desktop's `Composer` entity outlives any one route.
 *
 * A cleared draft leaves the map (no empty-string tombstones): `get` returns
 * "" either way, and the map stays the size of the user's actual drafts.
 */
export class ChatDraftStore {
  readonly #drafts = new Map<string, string>();

  get(key: string): string {
    return this.#drafts.get(key) ?? "";
  }

  set(key: string, text: string): void {
    if (text.length === 0) {
      this.#drafts.delete(key);
      return;
    }
    this.#drafts.set(key, text);
  }

  clear(key: string): void {
    this.#drafts.delete(key);
  }

  /** Test seam — drop every draft. */
  reset(): void {
    this.#drafts.clear();
  }
}

/** The one draft map for this page load. */
export const chatDrafts = new ChatDraftStore();

// ---------------------------------------------------------------------------
// Draft application
// ---------------------------------------------------------------------------

/**
 * `update_chat_config`'s local half: apply a picker's update to the draft,
 * then re-clamp reasoning to the (possibly just-changed) selection's
 * EFFECTIVE ladder and re-run `offeredOptions` so a model switch can't
 * carry picks the new model doesn't offer (e.g. a 1M-context pick surviving
 * a switch to a model without that option). `resolveModel` looks up the
 * catalog row for (harness, model id); `resolveDescriptor` the harness
 * descriptor — both resolved for the NEXT draft's harness, so a harness
 * switch never clamps against the harness being left.
 *
 * The clamp mirrors the native persisted-config normalization
 * (`pickers.rs:1493-1512`): only a NONEMPTY effective ladder re-derives the
 * level. An empty model list falls back to the descriptor's advertised
 * levels (`trait_ladder`), and while neither is available the stored
 * preference is retained verbatim — never destructively nulled just because
 * the metadata hasn't resolved.
 */
export function applyDraftUpdate(
  draft: DraftConfig,
  update: DraftConfigUpdate,
  resolveModel: (harness: HarnessId, modelId: string | null) => Model | null,
  resolveDescriptor: (harness: HarnessId) => HarnessDescriptor | null,
): DraftConfig {
  const next: DraftConfig = {
    harness: update.harness ?? draft.harness,
    model: update.model !== undefined ? update.model : draft.model,
    reasoning: update.reasoning !== undefined ? update.reasoning : draft.reasoning,
    // Never user-picked: written on create, preserved from then on.
    sandbox: draft.sandbox,
    modelOptions: update.modelOptions !== undefined ? { ...update.modelOptions } : draft.modelOptions,
  };
  const model = resolveModel(next.harness, next.model);
  const ladder = effectiveReasoningLadder(model, resolveDescriptor(next.harness));
  const reasoning = normalizeReasoning(next.reasoning, ladder);
  const modelOptions = model === null ? next.modelOptions : offeredOptions(model, next.modelOptions);
  return { ...next, reasoning, modelOptions };
}

/**
 * The composer reconciliation effect's pure core — the sticky-default model
 * seeding (`pickers.rs:748-796`) plus the descriptor-aware reasoning
 * normalization (`pickers.rs:1493-1512`), run against the harness's loaded
 * model rows and its MATCHING descriptor:
 *
 * - The draft's model still resolves in the list: only reasoning re-derives,
 *   and only against a nonempty effective ladder — a model whose own list is
 *   empty falls back to the descriptor's, and an empty effective ladder
 *   retains the stored preference rather than erasing it.
 * - Otherwise seed the remembered model when the list still offers it, else
 *   the harness default (first row), re-deriving reasoning the same way.
 *
 * Returns the PRIOR draft when nothing changed — the effect's setState-loop
 * guard, so an equivalent catalog refresh is observationally a no-op.
 */
export function reconcileDraftModel(
  current: DraftConfig,
  models: readonly Model[],
  descriptor: HarnessDescriptor | null,
  rememberedModel: RememberedModel | null,
): DraftConfig {
  const found =
    current.model === null ? undefined : models.find((model) => model.id === current.model);
  if (found !== undefined) {
    const ladder = effectiveReasoningLadder(found, descriptor);
    const reasoning = normalizeReasoning(current.reasoning, ladder);
    return reasoning === current.reasoning ? current : { ...current, reasoning };
  }
  const seeded =
    rememberedModel !== null && models.some((model) => model.id === rememberedModel.id)
      ? rememberedModel.id
      : models[0]?.id;
  if (seeded === undefined) {
    return current;
  }
  const model = models.find((row) => row.id === seeded) ?? null;
  const ladder = effectiveReasoningLadder(model, descriptor);
  const reasoning = normalizeReasoning(current.reasoning, ladder);
  // The model necessarily changes here (the current one failed to resolve),
  // so this branch is always a real update.
  return { ...current, model: seeded, reasoning };
}
