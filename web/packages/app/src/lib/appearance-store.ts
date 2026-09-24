import {
  accentPresets,
  type AccentPresetId,
  type Appearance,
  type SurfaceTreatment,
  type ThemeVariant,
} from "@zeron/theme";
import type { IconName } from "@zeron/icons";
import type { StorageLike } from "./engine-store";
import { UiSettingsStore, uiSettings, type UiSettings } from "../state/ui-settings";
import { findVariantAnywhere, variantsForAppearanceAll } from "./theme-library";

/**
 * Appearance preferences — the web peer of the desktop's device-local
 * appearance settings (crates/ui/src/appearance.rs + settings/appearance.rs):
 * an appearance mode, an independent light/dark theme variant pair, an accent
 * selection, and a surface preference. They live in the consolidated
 * `state/ui-settings.ts` store (`appearance`, `themeSelection.light`/`.dark`,
 * `accent`, `surface`), browser-scoped to the serving origin ("These settings
 * stay in this browser") and applied live — every mutation re-installs the
 * theme on the document root.
 *
 * What stays here is the part settings storage cannot know: a variant id is
 * only valid for the appearance it was authored for, so the pair is validated
 * against the theme registry on the way out of the store, not on the way in.
 *
 * The interface font/size pickers, the new-thread background rows and the
 * custom theme library (ticket 28) read/write the settings store directly;
 * the library itself lives in `lib/theme-library.ts` and its variants merge
 * into this module's registry overlay.
 */

/** The persisted mode choice; `system` follows `prefers-color-scheme`. */
export type AppearanceMode = "system" | "light" | "dark";

/** Accent selection: the variant's authored accent, or one of the 7 presets. */
export type AccentSelection = "themeDefault" | AccentPresetId;

/**
 * Surface policy. Deliberate web deviation from the desktop (which offers
 * Theme default / Frosted / Opaque, settings/appearance.rs:522-527): the
 * frosted choice is removed by product decision and the resolution is forced
 * opaque — see `resolveSurfaceTreatment`.
 */
export type SurfacePreference = "themeDefault" | "opaque";

export interface AppearancePreferences {
  readonly mode: AppearanceMode;
  /** Variant id used whenever the light appearance is active. */
  readonly lightVariant: string;
  /** Variant id used whenever the dark appearance is active. */
  readonly darkVariant: string;
  readonly accent: AccentSelection;
  readonly surface: SurfacePreference;
}

/** The desktop's `ThemeSelection::default` + default accent/surface. */
export const DEFAULT_APPEARANCE: AppearancePreferences = {
  mode: "system",
  lightVariant: "zeron-light",
  darkVariant: "zeron-dark",
  accent: "themeDefault",
  surface: "themeDefault",
};

export const APPEARANCE_MODES: readonly AppearanceMode[] = ["system", "light", "dark"];
export const SURFACE_PREFERENCES: readonly SurfacePreference[] = ["themeDefault", "opaque"];

/** The user's choice combined with the OS state (appearance.rs `resolve`). */
export function resolveAppearance(mode: AppearanceMode, system: Appearance): Appearance {
  switch (mode) {
    case "light":
      return "light";
    case "dark":
      return "dark";
    default:
      return system;
  }
}

/** The variant id for a resolved appearance; the pair is independent. */
export function resolveVariantId(preferences: AppearancePreferences, appearance: Appearance): string {
  return appearance === "dark" ? preferences.darkVariant : preferences.lightVariant;
}

/**
 * The surface treatment in effect. A product decision (2026-09-17): the web
 * never frosts — the treatment is forced opaque regardless of the stored
 * preference or the theme author's recommendation (both default themes
 * recommend frosted, so honoring "themeDefault" would leave the app frosted
 * anyway). A deliberate deviation from the desktop's
 * `SurfacePreference::resolve`; the one seam a future flip restores.
 */
export function resolveSurfaceTreatment(): SurfaceTreatment {
  return "opaque";
}

/**
 * The selector's variant list for one appearance: only variants authored for
 * it, registry order (family by family) — the desktop's
 * `variants_for(appearance)`. Installed custom-library variants merge in
 * after the builtins (ticket 28 §2.11's registry entry), exactly like the
 * desktop's `ThemeRegistry::active()`.
 */
export function variantChoices(appearance: Appearance): readonly ThemeVariant[] {
  return variantsForAppearanceAll(appearance);
}

/** Row label for an appearance mode card (AppearanceMode::label). */
export function appearanceModeLabel(mode: AppearanceMode): string {
  switch (mode) {
    case "system":
      return "System";
    case "light":
      return "Light";
    case "dark":
      return "Dark";
  }
}

/**
 * Shared glyph for appearance controls throughout the app
 * (AppearanceMode::icon, upstream b4dd24d7).
 */
export function appearanceModeIcon(mode: AppearanceMode): IconName {
  switch (mode) {
    case "system":
      return "monitor";
    case "light":
      return "sun";
    case "dark":
      return "moon";
  }
}

/** Row label for a surface choice (surface_label). */
export function surfaceLabel(surface: SurfacePreference): string {
  switch (surface) {
    case "themeDefault":
      return "Theme default";
    case "opaque":
      return "Opaque";
  }
}

/** Helper copy under the accent row (accent_helper). */
export function accentHelper(accent: AccentSelection): string {
  if (accent === "themeDefault") {
    return "Theme default · Uses the palette's intended color.";
  }
  const preset = accentPresets.find((entry) => entry.id === accent);
  return `${preset?.label ?? accent} · Controls, glyphs, selections, code, and activity.`;
}

/** Helper copy under the glass row (surface_helper). */
export function surfaceHelper(surface: SurfacePreference, resolved: SurfaceTreatment): string {
  switch (surface) {
    case "themeDefault":
      return `Uses this theme's ${resolved} default.`;
    case "opaque":
      return "Solid surfaces for every theme.";
  }
}

/** The swatch color for an accent choice under a resolved appearance. */
export function accentSwatchColor(accent: AccentSelection, variant: ThemeVariant): string {
  if (accent === "themeDefault") {
    return variant.accent.primary;
  }
  const preset = accentPresets.find((entry) => entry.id === accent);
  if (preset === undefined) {
    return variant.accent.primary;
  }
  return variant.appearance === "dark" ? preset.dark : preset.light;
}

// ---------------------------------------------------------------------------
// Interface font (typography.rs:13-77, web scope: the 3 fixed choices)
// ---------------------------------------------------------------------------

/** The fixed web catalog — no OS font probe (ticket 28 §2.9 / §5). */
export type UiFontChoice = "geist" | "geistMono" | "system";

export const UI_FONT_CHOICES: readonly UiFontChoice[] = ["geist", "geistMono", "system"];

/**
 * `resolve_effective` (typography.rs:295-299), web form: an `installed:*`
 * request has no availability probe to satisfy, so it resolves to the first
 * available choice (Geist) — the desktop's fallback when a requested family
 * is not installed on the device.
 */
export function effectiveUiFontFamily(requested: string): UiFontChoice {
  if (requested === "geistMono" || requested === "system") {
    return requested;
  }
  return "geist";
}

/** `UiFontFamily::label`. */
export function fontFamilyLabel(family: UiFontChoice): string {
  switch (family) {
    case "geist":
      return "Geist";
    case "geistMono":
      return "Geist Mono";
    case "system":
      return "System UI";
  }
}

/** The CSS stack a choice installs on `--rb-font-sans`. */
export function fontFamilyStack(family: UiFontChoice): string {
  switch (family) {
    case "geist":
      return '"Geist", ui-sans-serif, system-ui, sans-serif';
    case "geistMono":
      return '"Geist Mono", ui-monospace, monospace';
    case "system":
      return "system-ui, ui-sans-serif, sans-serif";
  }
}

/**
 * `step_font` (appearance.rs:462-480), degenerate web form: a clamped index
 * step over the 3 always-available choices — stepping past either end is a
 * no-op. (The desktop's availability-skipping walk has nothing to skip: no
 * OS font probe exists on the web, all choices are always available.)
 */
export function stepFont(current: UiFontChoice, delta: number): UiFontChoice {
  const currentIx = UI_FONT_CHOICES.indexOf(current);
  if (currentIx < 0 || delta === 0) {
    return current;
  }
  const next = currentIx + Math.sign(delta);
  if (next < 0 || next >= UI_FONT_CHOICES.length) {
    return current;
  }
  return UI_FONT_CHOICES[next]!;
}

// ---------------------------------------------------------------------------
// Terminal + code fonts (typography.rs, upstream #374)
// ---------------------------------------------------------------------------

/**
 * The terminal's catalog is narrowed to fixed-width families
 * (`fixed_width_choices`): the terminal grid's cursor, selection and
 * cell_at all assume one advance per cell, and the desktop qualifies
 * families by comparing real i/m/W/0 advances. No OS font probe exists on
 * the web, so the catalog is exactly the bundled monospace.
 */
export const TERMINAL_FONT_CHOICES: readonly UiFontChoice[] = ["geistMono"];

/** Code and diffs lay text out naturally; the whole catalog qualifies. */
export const CODE_FONT_CHOICES: readonly UiFontChoice[] = UI_FONT_CHOICES;

/**
 * `terminal_effective` (typography.rs): a persisted proportional family
 * falls back to Geist Mono. The store heals writes the same way
 * (`healTerminalFontFamily`); this is the read-side guard.
 */
export function effectiveTerminalFontFamily(requested: string): UiFontChoice {
  return TERMINAL_FONT_CHOICES.includes(requested as UiFontChoice)
    ? (requested as UiFontChoice)
    : TERMINAL_FONT_CHOICES[0]!;
}

/**
 * `code_effective` (typography.rs): like the interface resolver, except the
 * fallback for an unsatisfiable request (`installed:*` the web cannot
 * probe) is the code default, Geist Mono — not Geist.
 */
export function effectiveCodeFontFamily(requested: string): UiFontChoice {
  if (requested === "geist" || requested === "geistMono" || requested === "system") {
    return requested;
  }
  return "geistMono";
}

/**
 * `MONO_FONT_SIZES` (settings/appearance.rs): the pixel ladder behind the
 * terminal and code size dropdowns. Both defaults (terminal 13, code 12.5)
 * are rungs, so today's rendering is exactly reachable.
 */
export const MONO_FONT_SIZES: readonly number[] = [10, 11, 12, 12.5, 13, 14, 15, 16, 18, 20];

/** `nearest_mono_ix`: off-ladder values (older settings, hand edits) snap. */
export function nearestMonoFontSize(size: number): number {
  let best = MONO_FONT_SIZES[0]!;
  for (const rung of MONO_FONT_SIZES) {
    if (Math.abs(rung - size) < Math.abs(best - size)) {
      best = rung;
    }
  }
  return best;
}

/** `format_px`: ladder labels read as plain pixel values ("12.5 px"). */
export function fontSizePxLabel(size: number): string {
  return `${Number.isInteger(size) ? size : size.toFixed(1)} px`;
}

/** A variant id is only valid for the appearance it was authored for. */
function variantForAppearance(id: unknown, appearance: Appearance): string | null {
  if (typeof id !== "string") {
    return null;
  }
  const variant = findVariantAnywhere(id);
  return variant !== undefined && variant.appearance === appearance ? variant.id : null;
}

export interface AppearanceStoreOptions {
  /** The settings store to read through; defaults to the app's singleton. */
  readonly settings?: UiSettingsStore;
  /** Convenience for tests: a settings store over this storage. */
  readonly storage?: StorageLike;
}

export class AppearanceStore {
  readonly #settings: UiSettingsStore;
  #preferences: AppearancePreferences;
  readonly #listeners = new Set<() => void>();

  constructor(options: AppearanceStoreOptions = {}) {
    this.#settings =
      options.settings ??
      (options.storage === undefined ? uiSettings : new UiSettingsStore({ storage: options.storage }));
    this.#preferences = project(this.#settings.getSnapshot());
    this.#settings.subscribe(() => {
      this.#apply(project(this.#settings.getSnapshot()));
    });
  }

  getSnapshot(): AppearancePreferences {
    return this.#preferences;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  setMode(mode: AppearanceMode): void {
    this.#update({ mode });
  }

  /** The desktop's set_theme: variant ids are pinned per appearance. */
  setVariant(appearance: Appearance, variantId: string): void {
    if (variantForAppearance(variantId, appearance) === null) {
      return;
    }
    this.#update(appearance === "dark" ? { darkVariant: variantId } : { lightVariant: variantId });
  }

  setAccent(accent: AccentSelection): void {
    this.#update({ accent });
  }

  setSurface(surface: SurfacePreference): void {
    this.#update({ surface });
  }

  #update(patch: Partial<AppearancePreferences>): void {
    const next: AppearancePreferences = { ...this.#preferences, ...patch };
    // Appearance is a discrete choice, never a drag — write it straight
    // through. Re-projecting afterwards keeps the validated view authoritative.
    this.#settings.update(
      {
        appearance: next.mode,
        themeSelection: { light: next.lightVariant, dark: next.darkVariant },
        accent: next.accent,
        surface: next.surface,
      },
      "immediate",
    );
    this.#apply(project(this.#settings.getSnapshot()));
  }

  #apply(next: AppearancePreferences): void {
    if (
      next.mode === this.#preferences.mode &&
      next.lightVariant === this.#preferences.lightVariant &&
      next.darkVariant === this.#preferences.darkVariant &&
      next.accent === this.#preferences.accent &&
      next.surface === this.#preferences.surface
    ) {
      return;
    }
    this.#preferences = next;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

/**
 * The settings snapshot's appearance slice, with each variant id checked
 * against the registry it has to come from — a hand-edited or stale id (a
 * removed theme, a dark variant stored as the light one) falls back to that
 * side's default without disturbing the other.
 */
function project(settings: UiSettings): AppearancePreferences {
  return {
    mode: settings.appearance,
    lightVariant:
      variantForAppearance(settings.themeSelection.light, "light") ?? DEFAULT_APPEARANCE.lightVariant,
    darkVariant:
      variantForAppearance(settings.themeSelection.dark, "dark") ?? DEFAULT_APPEARANCE.darkVariant,
    accent: settings.accent,
    surface: settings.surface,
  };
}
