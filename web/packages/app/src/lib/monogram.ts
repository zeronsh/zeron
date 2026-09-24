/**
 * Project monograms — the web peer of `crates/ui/src/shell/project_icon.rs`'s
 * `monogram`/`MONOGRAM_PALETTE` (upstream 1ec74e40, 82cdfe90, 2f40dfad):
 * a project's initial on a small frosted tile whose tone comes from a fixed
 * curated palette, chosen by a stable FNV-1a hash of the project path so a
 * project keeps its color across reloads and platforms.
 */

/**
 * Curated badge tones: (dark appearance, light appearance), as 0xRRGGBB.
 * Keep the ordering stable so projects retain their assigned color. These
 * are explicit colors, independent of the selected theme accent; only their
 * appearance variant changes.
 */
export const MONOGRAM_PALETTE: readonly (readonly [number, number])[] = [
  [0x94a3b8, 0x475569], // slate
  [0x93c5fd, 0x2563eb], // blue
  [0xc4b5fd, 0x7c3aed], // violet
  [0xfda4af, 0xbe123c], // rose
  [0xfcd34d, 0xa16207], // amber
  [0x6ee7b7, 0x047857], // emerald
  [0x5eead4, 0x0f766e], // teal
  [0xfdba74, 0xc2410c], // orange
];

/** FNV-1a (project_icon.rs) — stable across processes and platforms. */
export function fnv1a(seed: string): number {
  let hash = 2166136261;
  for (let ix = 0; ix < seed.length; ix += 1) {
    hash ^= seed.charCodeAt(ix);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

/** The project's monogram initial: first char, uppercased, "?" fallback. */
export function monogramLetter(name: string): string {
  const trimmed = name.trim();
  const first = trimmed.charAt(0);
  return first === "" ? "?" : first.toUpperCase();
}

/** "dark" lightness is the palette's dark-mode variant. */
export interface MonogramTone {
  readonly dark: number;
  readonly light: number;
}

/**
 * The palette entry a project path selects — `MONOGRAM_PALETTE[hash %
 * len]`, the same arithmetic as `project_icon.rs`'s
 * `MONOGRAM_PALETTE[hash as usize % MONOGRAM_PALETTE.len()]`.
 */
export function monogramTone(seed: string): MonogramTone {
  const [dark, light] = MONOGRAM_PALETTE[fnv1a(seed) % MONOGRAM_PALETTE.length]!;
  return { dark, light };
}

/**
 * The CSS color for a tone under an appearance: `rgb(r g b / alpha)`
 * compositing keeps the desktop's `tone.opacity(...)` arithmetic (8% plate,
 * 85% letter; 24% plate + full hover letter on hover/selected).
 */
export function monogramCss(
  tone: MonogramTone,
  appearance: "light" | "dark",
  alpha: number,
): string {
  const rgb = appearance === "dark" ? tone.dark : tone.light;
  const r = (rgb >> 16) & 0xff;
  const g = (rgb >> 8) & 0xff;
  const b = rgb & 0xff;
  return `rgb(${r} ${g} ${b} / ${alpha})`;
}
