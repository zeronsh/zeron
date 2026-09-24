/**
 * Types for the Zeron web theme artifact. Hand-written; the shape mirrors
 * the serde output of `zeron_theme::artifact::ThemeArtifact` (camelCase),
 * which in turn embeds the desktop's `ThemeVariant` model verbatim.
 */

/** A CSS hex color: `#rrggbb` (opaque) or `#rrggbbaa`. */
export type HexColor = string;

export type Appearance = "dark" | "light";

/** The theme author's recommended surface treatment for a variant. */
export type SurfaceTreatment = "opaque" | "frosted";

/** UI roles + status + diff — the post-hardening color role set. */
export interface ThemeColors {
  background: HexColor;
  shell: HexColor;
  raised: HexColor;
  /** Hover tone for an OPAQUE raised pill — never the translucent `hover`. */
  raisedHover: HexColor;
  card: HexColor;
  dialog: HexColor;
  overlay: HexColor;
  hover: HexColor;
  active: HexColor;
  border: HexColor;
  borderStrong: HexColor;
  text: HexColor;
  textMuted: HexColor;
  textFaint: HexColor;
  /** One notch below `textMuted`: the diff/file-path tone. */
  textDim: HexColor;
  solid: HexColor;
  onSolid: HexColor;
  danger: HexColor;
  /** The destructive-action button plate; carries `onAccent`, not `danger`. */
  dangerStrong: HexColor;
  dangerMuted: HexColor;
  warning: HexColor;
  warningMuted: HexColor;
  success: HexColor;
  successMuted: HexColor;
  input: HexColor;
  cursor: HexColor;
  diffAdd: HexColor;
  diffDelete: HexColor;
  diffHunk: HexColor;
}

/** Interaction roles — the only colors an accent preset override touches. */
export interface AccentRoles {
  primary: HexColor;
  strong: HexColor;
  wash: HexColor;
  on: HexColor;
  selection: HexColor;
  caret: HexColor;
  activity: HexColor;
  /** Light/mid/deep rows of the animated 2×3 pixel glyph. */
  glyph: [HexColor, HexColor, HexColor];
}

export interface TerminalPalette {
  background: HexColor;
  foreground: HexColor;
  selection: HexColor;
  /** Slots 0–7 normal, 8–15 bright. */
  ansi: HexColor[];
}

export interface ThemeSource {
  format: string;
  url: string;
  revision: string;
  license: string;
  /** `sha256:…` of the resolved variant definition. */
  assetHash: string;
}

/** A fully resolved theme variant, exactly as the desktop installs it. */
export interface ThemeVariant {
  id: string;
  familyId: string;
  name: string;
  appearance: Appearance;
  recommendedSurfaceTreatment: SurfaceTreatment;
  colors: ThemeColors;
  /** Theme-authored accent (used when the selection is `themeDefault`). */
  accent: AccentRoles;
  /** Syntax highlighting roles (25 keys, e.g. `keyword`, `stringSpecial`). */
  syntax: Record<string, HexColor>;
  terminal: TerminalPalette;
  source: ThemeSource;
}

export interface ThemeFamily {
  id: string;
  name: string;
  variants: ThemeVariant[];
}

/** Serde names of `AccentPreset` — the same strings settings carry. */
export type AccentPresetId =
  | "zeron"
  | "orange"
  | "amber"
  | "green"
  | "cyan"
  | "blue"
  | "pink";

/** A selectable accent preset and its authored dark/light base colors. */
export interface AccentPresetTokens {
  id: AccentPresetId;
  label: string;
  dark: HexColor;
  light: HexColor;
}

export interface SpaceTokens {
  xs: number;
  sm: number;
  md: number;
  lg: number;
  textStackGap: number;
}

export interface RadiusTokens {
  bubble: number;
  panel: number;
  control: number;
}

export interface ChromeTokens {
  headerHeight: number;
  titlebarHeight: number;
  titlebarTopPad: number;
  statusStripHeight: number;
  transcriptFadeBand: number;
}

/** Frosted-surface coverage (unit-interval alphas). */
export interface GlassTokens {
  /** Window chrome tint on compositor-blur platforms (macOS/Windows). */
  windowAlpha: number;
  /** Window chrome tint where compositor blur is unavailable (Linux). */
  windowAlphaOpaque: number;
  overlayAlphaDark: number;
  overlayAlphaLight: number;
  inputAlphaLight: number;
  cardAlpha: number;
  /**
   * Selected row/chip/tab wash over `--rb-wash` (`glass_selected_bg()`).
   * Light runs at half — 11% black read too dark over the bright frost.
   */
  selectedWashAlphaDark: number;
  selectedWashAlphaLight: number;
  /** Recessed picker header/footer strip over literal black (`band()`). */
  bandAlphaDark: number;
  bandAlphaLight: number;
  /** Modal/drawer backdrop over literal black (`scrim()`). */
  scrimAlphaDark: number;
  scrimAlphaLight: number;
}

export interface LayoutTokens {
  space: SpaceTokens;
  radius: RadiusTokens;
  chrome: ChromeTokens;
  glass: GlassTokens;
}

/** `[x1, y1, x2, y2]` of a CSS `cubic-bezier()`. */
export type CubicBezierTokens = [number, number, number, number];

export interface MotionSpecTokens {
  name: string;
  durationMs: number;
  delayMs: number;
  /** Name of the entry's curve in `MotionTokens.curves`. */
  curve: string;
}

export interface ResizeBounceTokens {
  nudgePx: number;
  durationMs: number;
  outFraction: number;
}

/** The transcript's stick-to-bottom spring (mugen DEFAULT_SPRING shape). */
export interface StickSpringTokens {
  damping: number;
  stiffness: number;
  mass: number;
  frameMs: number;
  maxCatchupFrames: number;
  growthEma: number;
  chaseMaxLeadPx: number;
  atBottomPx: number;
  stickThresholdPx: number;
  settleGraceMs: number;
  glideMaxViewports: number;
}

/** The composer's critically damped dock glide (new-thread handoff). */
export interface DockGlideTokens {
  /** Omega per second of requested duration. */
  timeConstants: number;
  dockSeconds: number;
  undockSeconds: number;
  settlePosition: number;
  settleVelocity: number;
}

export interface MotionTokens {
  curves: Record<string, CubicBezierTokens>;
  specs: MotionSpecTokens[];
  resizeBounce: ResizeBounceTokens;
  stickSpring: StickSpringTokens;
  dockGlide: DockGlideTokens;
}

export interface FontFace {
  family: string;
  /** File name under `web/packages/theme/fonts/`. */
  file: string;
  weight: number;
  style: "normal" | "italic";
}

export interface ThemeArtifact {
  schemaVersion: number;
  generator: string;
  /** The builtin registry exactly as the desktop resolves it. */
  families: ThemeFamily[];
  accentPresets: AccentPresetTokens[];
  /** `variantId -> presetId -> derived roles`, precomputed for every
   * variant × preset so the web needs zero color math. */
  accents: Record<string, Partial<Record<AccentPresetId, AccentRoles>>>;
  layout: LayoutTokens;
  motion: MotionTokens;
  fonts: FontFace[];
}
