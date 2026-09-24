import {
  applyThemeVariant,
  layout,
  layoutCssVars,
  motionCssVars,
  type Appearance,
} from "@zeron/theme";
import {
  DEFAULT_APPEARANCE,
  effectiveCodeFontFamily,
  effectiveUiFontFamily,
  fontFamilyStack,
  resolveAppearance,
  resolveSurfaceTreatment,
  resolveVariantId,
  type AppearancePreferences,
} from "./lib/appearance-store";
import { findVariantAnywhere } from "./lib/theme-library";

/**
 * Install the appearance preferences on the document root: the resolved
 * variant's color roles under the selected accent, the layout constants,
 * the motion catalog, and the glass alphas as `--rb-*` custom properties;
 * the resolved surface treatment lands on `data-surface` so the CSS can
 * thin floating surfaces into frosted glass. Re-called on every preference
 * or OS-appearance change — application is idempotent, so "live" costs a
 * style recalculation only.
 */
export function applyAppearanceToDocument(
  preferences: AppearancePreferences = DEFAULT_APPEARANCE,
  system: Appearance = "dark",
  root: HTMLElement = document.documentElement,
): void {
  const appearance = resolveAppearance(preferences.mode, system);
  const variantId = resolveVariantId(preferences, appearance);
  // The registry union: an installed custom-library variant resolves here
  // too, so a stored selection survives the reload it was persisted for.
  const variant = findVariantAnywhere(variantId) ?? findVariantAnywhere(DEFAULT_APPEARANCE.darkVariant);
  if (variant === undefined) {
    throw new Error(`Unknown theme variant: ${variantId}`);
  }
  applyThemeVariant(root, variant, { accent: preferences.accent });
  for (const [name, value] of Object.entries({ ...layoutCssVars(), ...motionCssVars() })) {
    root.style.setProperty(name, value);
  }
  root.style.setProperty("--rb-glass-card-alpha", String(layout.glass.cardAlpha));
  // The gradient matrix spinner's fixed "sunrise" row tints (GSPIN_ROW_TINTS,
  // proto/motion.rs:34) — the documented non-theme color exception: NOT
  // accent-derived, identical in every variant.
  root.style.setProperty("--rb-gspin-row-0", "#B6D3EF");
  root.style.setProperty("--rb-gspin-row-1", "#EDB185");
  root.style.setProperty("--rb-gspin-row-2", "#F888A0");
  const dark = variant.appearance === "dark";
  root.style.setProperty(
    "--rb-glass-overlay-alpha",
    String(dark ? layout.glass.overlayAlphaDark : layout.glass.overlayAlphaLight),
  );
  // The three derived alphas that scale a neutral rather than naming a color
  // (crates/ui/src/theme.rs: glass_selected_bg/card_selected_bg, band, scrim).
  // Selection scales `--rb-wash`; band and scrim always sit on literal black.
  root.style.setProperty(
    "--rb-selected-wash-alpha",
    String(dark ? layout.glass.selectedWashAlphaDark : layout.glass.selectedWashAlphaLight),
  );
  root.style.setProperty(
    "--rb-band-alpha",
    String(dark ? layout.glass.bandAlphaDark : layout.glass.bandAlphaLight),
  );
  root.style.setProperty(
    "--rb-scrim-alpha",
    String(dark ? layout.glass.scrimAlphaDark : layout.glass.scrimAlphaLight),
  );
  for (const [name, value] of Object.entries(inkCssVars(variant.appearance))) {
    root.style.setProperty(name, value);
  }
  root.dataset.surface = resolveSurfaceTreatment();
  root.style.colorScheme = variant.appearance;
}

/**
 * The interface typography (typography.rs:189-318): the chosen family on
 * `--rb-font-sans` (the variable `body` consumes) and the chosen size on
 * `--rb-ui-size` (the baseline text scales with it — 14px designed at the
 * 16px default, `ui_rems(14)` at a root of `size`). Called on boot and on
 * every settings write; the font-picker block itself stays on
 * `--rb-font-sans-fixed` so the control never renders in a font it just
 * broke.
 *
 * The independent code/diff slot (upstream #374) lands on `--rb-font-mono`
 * (the variable `.mono` consumes) and `--rb-code-size` (the shared code
 * size the per-surface baselines scale from, lib/typography.ts). The
 * terminal slot bypasses CSS entirely — XTerm reads options, so
 * `terminal/store.tsx` applies it.
 */
export function applyTypographyToDocument(
  typography: {
    readonly uiFontFamily: string;
    readonly uiFontSize: number;
    readonly codeFontFamily?: string;
    readonly codeFontSize?: number;
  },
  root: HTMLElement = document.documentElement,
): void {
  root.style.setProperty("--rb-font-sans", fontFamilyStack(effectiveUiFontFamily(typography.uiFontFamily)));
  root.style.setProperty("--rb-ui-size", String(typography.uiFontSize));
  if (typography.codeFontFamily !== undefined) {
    root.style.setProperty(
      "--rb-font-mono",
      fontFamilyStack(effectiveCodeFontFamily(typography.codeFontFamily)),
    );
  }
  if (typography.codeFontSize !== undefined) {
    root.style.setProperty("--rb-code-size", String(typography.codeFontSize));
  }
}

/**
 * The transcript's content column cap (`transcript_width`, upstream cbf2ad84)
 * on `--rb-transcript-width` — the variable `.trow-col` consumes. The
 * composer's 768px column stays independent, as on the desktop.
 */
export function applyConversationWidthToDocument(
  width: number,
  root: HTMLElement = document.documentElement,
): void {
  root.style.setProperty("--rb-transcript-width", `${width}px`);
}

/**
 * `INK_HAIRLINE_SCALE` — a 1px line needs *more* ink on a bright field than a
 * plate does, so hairlines scale up in light mode where fills do not.
 */
const INK_HAIRLINE_SCALE = 1.35;

/**
 * The desktop's three neutral ladders, as `rgb()` channel triples plus the
 * light-mode hairline scale (`crates/ui/src/theme.rs`):
 *
 * - `ink` — soft-white/black fills for chips and plates.
 * - `wash` — an ink softened short of pure black or white, so hover and
 *   selection read as tinted glass rather than paint.
 * - `hairline` — borders, dividers, and rings.
 *
 * They are tone-flipped, not accent-tinted: selection on the desktop is a
 * neutral wash over the vibrancy, and reaching for an accent role here is what
 * made the web's selected row read as a purple slab. Call sites write
 * `rgb(var(--rb-wash) / 0.11)`.
 */
export function inkCssVars(appearance: Appearance): Record<string, string> {
  const dark = appearance === "dark";
  return {
    "--rb-ink": dark ? "255 255 255" : "0 0 0",
    "--rb-wash": dark ? "235 235 235" : "26 26 26",
    "--rb-hairline": dark ? "255 255 255" : "0 0 0",
    // Dark fills and hairlines use the authored alpha as-is; light scales.
    "--rb-ink-scale": "1",
    "--rb-hairline-scale": dark ? "1" : String(INK_HAIRLINE_SCALE),
  };
}
