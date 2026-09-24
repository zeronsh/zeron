/**
 * `@zeron/theme` — the Zeron design tokens for the web client.
 *
 * The data under `./generated/` is produced by `zeron-theme-export`
 * (`cargo run -p zeron-theme --bin zeron-theme-export`) from the same Rust
 * sources the desktop compiles: the builtin theme registry, the accent preset
 * derivations, the layout constants (`zeron_proto::layout`), and the motion
 * catalog (`zeron_proto::motion`). A CI gate (`theme-artifact.yml`) fails
 * when the artifact is stale.
 */

import { themeArtifact } from "./generated/index";
import type { AccentPresetId, AccentRoles, Appearance, ThemeColors, ThemeFamily, ThemeVariant } from "./types";

export * from "./types";
export { themeArtifact };

/** Every builtin family (19), in registry order. */
export const themeFamilies: readonly ThemeFamily[] = themeArtifact.families;

/** Every builtin variant (30), in registry order. */
export const themeVariants: readonly ThemeVariant[] = themeArtifact.families.flatMap(
  (family) => family.variants,
);

/** The selectable accent presets (7) with their authored dark/light bases. */
export const accentPresets = themeArtifact.accentPresets;

/** Spacing ladder, radii, chrome heights, glass alphas (px / unit interval). */
export const layout = themeArtifact.layout;

/** The motion catalog: named curves, specs, and the two springs' parameters. */
export const motion = themeArtifact.motion;

/** Manifest of the bundled Geist/Geist Mono faces (see `../fonts.css`). */
export const fontFaces = themeArtifact.fonts;

/** Look up a builtin variant by id. */
export function findVariant(id: string): ThemeVariant | undefined {
  return themeVariants.find((variant) => variant.id === id);
}

/**
 * The accent roles a variant installs under a selection: the theme-authored
 * accent for `"themeDefault"`, otherwise the precomputed preset derivation
 * (identical to the desktop's `ThemeVariant::accent_for`). A variant the
 * artifact does not know (an imported custom-library family, ticket 28)
 * derives at runtime via `AccentRoles::derive` — the same math, so an
 * imported theme under a preset accent matches what the export would have
 * precomputed had the family been builtin.
 */
export function accentForVariant(
  variant: ThemeVariant,
  accent: AccentPresetId | "themeDefault" = "themeDefault",
): AccentRoles {
  if (accent === "themeDefault") {
    return variant.accent;
  }
  const roles = themeArtifact.accents[variant.id]?.[accent];
  if (roles) {
    return roles;
  }
  const preset = themeArtifact.accentPresets.find((entry) => entry.id === accent);
  if (!preset) {
    throw new Error(`no exported accent derivation for ${variant.id} + ${accent}`);
  }
  return deriveAccentRoles(
    variant.appearance === "dark" ? preset.dark : preset.light,
    variant.appearance,
    variant.colors.background,
  );
}

// ---------------------------------------------------------------------------
// Accent derivation (crates/theme/src/lib.rs AccentRoles::derive + the Color
// math it rides: mix/with_alpha/contrast/ensure_contrast/best_on_color)
// ---------------------------------------------------------------------------

interface Rgba {
  readonly r: number;
  readonly g: number;
  readonly b: number;
  readonly a: number;
}

function rgbaOf(hex: string): Rgba | null {
  if (!isHexColor(hex)) {
    return null;
  }
  return {
    r: Number.parseInt(hex.slice(1, 3), 16),
    g: Number.parseInt(hex.slice(3, 5), 16),
    b: Number.parseInt(hex.slice(5, 7), 16),
    a: hex.length === 9 ? Number.parseInt(hex.slice(7, 9), 16) : 255,
  };
}

/** A `#rrggbb` / `#rrggbbaa` string (the artifact's whole color vocabulary). */
export function isHexColor(hex: string): boolean {
  return /^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(hex);
}

/** `Color::contrast` for two hex colors; null when either fails to parse. */
export function colorContrast(foreground: string, background: string): number | null {
  const front = rgbaOf(foreground);
  const back = rgbaOf(background);
  if (front === null || back === null) {
    return null;
  }
  return contrastOf(front, back);
}

function hexOf(color: Rgba): string {
  const channel = (value: number): string => value.toString(16).padStart(2, "0");
  return `#${channel(color.r)}${channel(color.g)}${channel(color.b)}${channel(color.a)}`;
}

/**
 * `Theme::flatten` (crates/ui/src/theme.rs:1785): the opaque composite of a
 * possibly-translucent `fg` over an opaque `bg` — the color the eye actually
 * receives, returned at alpha 1. Ticket 79: the desktop's opaque-mode
 * `input_glass_bg()` is `flatten(input, bg)`; the web's former
 * `color-mix(input 82%, bg)` interpolated alpha instead of compositing,
 * leaving the composer pill ~23% see-through in every dark variant (their
 * `input` role carries authored alpha, e.g. `#343438b8`). Unparseable input
 * falls back to the raw `fg` string.
 */
function flattenHex(fg: string, bg: string): string {
  const front = rgbaOf(fg);
  const back = rgbaOf(bg);
  if (front === null || back === null) {
    return fg;
  }
  const a = front.a / 255;
  const channel = (f: number, b: number): number => Math.round(f * a + b * (1 - a));
  // Opaque by construction — emit the 6-digit form like the artifact's
  // opaque roles (bg is `#060606`, not `#060606ff`).
  return hexOf({
    r: channel(front.r, back.r),
    g: channel(front.g, back.g),
    b: channel(front.b, back.b),
    a: 255,
  }).slice(0, 7);
}

function mix(a: Rgba, b: Rgba, amount: number): Rgba {
  const t = Math.min(Math.max(amount, 0), 1);
  const channel = (front: number, back: number): number => Math.round(front + (back - front) * t);
  return { r: channel(a.r, b.r), g: channel(a.g, b.g), b: channel(a.b, b.b), a: channel(a.a, b.a) };
}

const WHITE: Rgba = { r: 255, g: 255, b: 255, a: 255 };
const BLACK: Rgba = { r: 0, g: 0, b: 0, a: 255 };

function luminanceOf(color: Rgba): number {
  const linear = (channel: number): number => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b);
}

function contrastOf(foreground: Rgba, background: Rgba): number {
  const blended = foreground.a === 255 ? foreground : mix(foreground, background, foreground.a / 255);
  const a = luminanceOf(blended);
  const b = luminanceOf(background);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function ensureContrast(color: Rgba, background: Rgba, minimum: number): Rgba {
  if (contrastOf(color, background) >= minimum) {
    return color;
  }
  const target = contrastOf(BLACK, background) >= contrastOf(WHITE, background) ? BLACK : WHITE;
  for (let step = 1; step <= 20; step += 1) {
    const candidate = mix(color, target, step / 20);
    if (contrastOf(candidate, background) >= minimum) {
      return candidate;
    }
  }
  return target;
}

function bestOnColor(color: Rgba): Rgba {
  return contrastOf(WHITE, color) >= contrastOf(BLACK, color) ? WHITE : BLACK;
}

/**
 * `AccentRoles::derive` (lib.rs:413-448): contrast-secured primary, a
 * same-family strong, translucent wash/selection, and the glyph ladder the
 * animated activity pixel paints.
 */
export function deriveAccentRoles(
  primary: string,
  appearance: Appearance,
  background: string,
): AccentRoles {
  const primaryColor = ensureContrast(rgbaOf(primary) ?? { ...WHITE }, rgbaOf(background) ?? { ...WHITE }, 3);
  const on = bestOnColor(primaryColor);
  let strong = primaryColor;
  if (contrastOf(on, strong) < 4.5) {
    strong = ensureContrast(strong, on, 4.5);
  }
  const dark = appearance === "dark";
  const light = mix(primaryColor, dark ? WHITE : rgbaOf(background) ?? WHITE, dark ? 0.28 : 0.18);
  const deep = mix(primaryColor, BLACK, dark ? 0.18 : 0.26);
  const withAlpha = (color: Rgba, alpha: number): string => hexOf({ ...color, a: Math.round(Math.min(Math.max(alpha, 0), 1) * 255) });
  return {
    primary: hexOf(primaryColor),
    strong: hexOf(strong),
    wash: withAlpha(primaryColor, dark ? 0.22 : 0.12),
    on: hexOf(on),
    selection: withAlpha(primaryColor, dark ? 0.35 : 0.24),
    caret: hexOf(primaryColor),
    activity: hexOf(primaryColor),
    glyph: [hexOf(light), hexOf(primaryColor), hexOf(deep)],
  };
}

const kebab = (name: string): string =>
  name.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();

/**
 * `ThemeColors` key -> CSS custom property suffix. `background` shortens to
 * `--rb-bg`; everything else is the kebab-cased role name.
 */
const COLOR_VARS: Record<keyof ThemeColors, string> = {
  background: "bg",
  shell: "shell",
  raised: "raised",
  raisedHover: "raised-hover",
  card: "card",
  dialog: "dialog",
  overlay: "overlay",
  hover: "hover",
  active: "active",
  border: "border",
  borderStrong: "border-strong",
  text: "text",
  textMuted: "text-muted",
  textFaint: "text-faint",
  textDim: "text-dim",
  solid: "solid",
  onSolid: "on-solid",
  danger: "danger",
  dangerStrong: "danger-strong",
  dangerMuted: "danger-muted",
  warning: "warning",
  warningMuted: "warning-muted",
  success: "success",
  successMuted: "success-muted",
  input: "input",
  cursor: "cursor",
  diffAdd: "diff-add",
  diffDelete: "diff-delete",
  diffHunk: "diff-hunk",
};

export interface VariantCssOptions {
  /** Accent selection; defaults to the theme-authored accent. */
  accent?: AccentPresetId | "themeDefault";
}

/**
 * A variant as CSS custom properties. Naming is systematic: `--rb-<role>`
 * for UI colors (`--rb-bg`, `--rb-text-muted`), `--rb-accent*` for the accent
 * roles, `--rb-syntax-<key>` for syntax, `--rb-term-*` for the terminal
 * palette. Colors are `#rrggbb`/`#rrggbbaa` strings, ready to assign.
 */
export function variantCssVars(
  variant: ThemeVariant,
  options: VariantCssOptions = {},
): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const [key, suffix] of Object.entries(COLOR_VARS) as [keyof ThemeColors, string][]) {
    vars[`--rb-${suffix}`] = variant.colors[key];
  }
  // Ticket 79 — the opaque input plate: the web port of the desktop's
  // `input_glass_bg()` (`flatten(input, bg)`, theme.rs:968-980). Dark
  // variants author `input` with alpha (`#343438b8`); flattened over the
  // variant background it yields the opaque plate the desktop paints in
  // opaque mode — the composer pill, the wizard, form inputs, and the
  // comment draft consume it.
  vars["--rb-input-plate"] = flattenHex(variant.colors.input, variant.colors.background);
  const accent = accentForVariant(variant, options.accent);
  vars["--rb-accent"] = accent.primary;
  vars["--rb-accent-strong"] = accent.strong;
  vars["--rb-accent-wash"] = accent.wash;
  vars["--rb-on-accent"] = accent.on;
  vars["--rb-selection"] = accent.selection;
  vars["--rb-caret"] = accent.caret;
  vars["--rb-activity"] = accent.activity;
  vars["--rb-glyph-light"] = accent.glyph[0];
  vars["--rb-glyph-mid"] = accent.glyph[1];
  vars["--rb-glyph-deep"] = accent.glyph[2];
  for (const [key, color] of Object.entries(variant.syntax)) {
    vars[`--rb-syntax-${kebab(key)}`] = color;
  }
  vars["--rb-term-bg"] = variant.terminal.background;
  vars["--rb-term-fg"] = variant.terminal.foreground;
  vars["--rb-term-selection"] = variant.terminal.selection;
  variant.terminal.ansi.forEach((color, index) => {
    vars[`--rb-term-ansi-${index}`] = color;
  });
  return vars;
}

/** Apply a variant's CSS custom properties to an element's inline style. */
export function applyThemeVariant(
  element: HTMLElement,
  variant: ThemeVariant,
  options: VariantCssOptions = {},
): void {
  for (const [name, value] of Object.entries(variantCssVars(variant, options))) {
    element.style.setProperty(name, value);
  }
}

/** Layout constants as px-valued CSS custom properties. */
export function layoutCssVars(): Record<string, string> {
  const { space, radius, chrome } = layout;
  const px = (value: number): string => `${value}px`;
  return {
    "--rb-space-xs": px(space.xs),
    "--rb-space-sm": px(space.sm),
    "--rb-space-md": px(space.md),
    "--rb-space-lg": px(space.lg),
    "--rb-text-stack-gap": px(space.textStackGap),
    "--rb-radius-bubble": px(radius.bubble),
    "--rb-radius-panel": px(radius.panel),
    "--rb-radius-control": px(radius.control),
    "--rb-header-height": px(chrome.headerHeight),
    "--rb-titlebar-height": px(chrome.titlebarHeight),
    "--rb-titlebar-top-pad": px(chrome.titlebarTopPad),
    "--rb-status-strip-height": px(chrome.statusStripHeight),
    "--rb-transcript-fade-band": px(chrome.transcriptFadeBand),
  };
}

/**
 * The motion catalog as CSS custom properties: `--rb-ease-<curve>` holds the
 * `cubic-bezier(...)` value, `--rb-motion-<spec>` the duration in ms, and
 * delayed specs additionally expose `--rb-motion-<spec>-delay`.
 */
export function motionCssVars(): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const [name, [x1, y1, x2, y2]] of Object.entries(motion.curves)) {
    vars[`--rb-ease-${kebab(name)}`] = `cubic-bezier(${x1}, ${y1}, ${x2}, ${y2})`;
  }
  for (const spec of motion.specs) {
    vars[`--rb-motion-${kebab(spec.name)}`] = `${spec.durationMs}ms`;
    if (spec.delayMs > 0) {
      vars[`--rb-motion-${kebab(spec.name)}-delay`] = `${spec.delayMs}ms`;
    }
  }
  return vars;
}
