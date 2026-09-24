/**
 * The activity glyphs — the desktop's `loaders.rs` spinners, in CSS.
 *
 * `GlyphSpinner` is `mini_glyph_spinner`: a 2×3 grid of round cells whose
 * brightness chases clockwise around the perimeter. `MatrixSpinner` is
 * `gradient_spinner` (the WorkingIndicator): a 3×3 grid whose pulse enters at
 * the bottom edge and converges on the top-centre cell, so the wave reads as
 * travelling upward.
 *
 * Both share one brightness curve and one period, straight from
 * `zeron_proto::motion`: `gspin_opacity` — full, then a linear fall to
 * `GSPIN_DIM` over the first 45% of the cycle, a hold to 92%, and a fast
 * return — over `GRADIENT_SPIN`'s 750ms. Per-cell phase becomes a negative
 * animation delay, which is how CSS says "start this cell further along".
 *
 * Row tints: the MINI spinner takes the accent's glyph roles (`--rb-glyph-*`,
 * `GlyphPalette::rows`); the 3×3 matrix takes the FIXED "sunrise" gradient
 * (`GSPIN_ROW_TINTS`, proto/motion.rs:34 — `--rb-gspin-row-*`), which is not
 * accent-derived. `MonoSpinner` tints every row with the caller's current
 * color.
 *
 * The pulse loaders (`ZeronMarkLoader`, `ZeronLoader`) share the 2.4s
 * `ZERON_PULSE` cosine (opacity 0.08→1, scale 0.9→1) with per-cell stagger
 * as an animation delay; reduced motion stops every animation (the cells sit
 * at their rest styles, the desktop's `pulse_delta` returning 0).
 */

import type { CSSProperties } from "react";

/** `motion::MINI_RING` — clockwise ring position of each (row, col) cell. */
export const MINI_RING: readonly (readonly number[])[] = [
  [0, 1],
  [5, 2],
  [4, 3],
];
/** `motion::MINI_RING_LEN`. */
export const MINI_RING_LEN = 6;

/** `motion::MATRIX_SIDE` — the gradient spinner is a square 3×3 grid. */
export const MATRIX_SIDE = 3;

/** `motion::GRADIENT_SPIN_MS` — the full-speed wave period. */
const GRADIENT_SPIN_MS = 750;
/** Half speed: the 3×3 matrix runs the same wave at 1500ms (`pulse_delta_slow`). */
const GRADIENT_SPIN_SLOW_MS = 1500;

export interface GlyphSpinnerProps {
  /** Cell edge in px — the desktop's `cell_px` (2.0 in sidebar corners). */
  readonly size?: number;
  /** Tint every row the current text color instead of the accent's glyph roles. */
  readonly mono?: boolean;
  readonly className?: string;
}

export function GlyphSpinner({ size = 11, mono = false, className }: GlyphSpinnerProps) {
  // The desktop sizes by cell; callers here size by the slot the glyph sits
  // in, so derive the cell from the box: h = cell * 4 (3 cells + 2 half-gaps).
  const cell = size / 4;
  return (
    <span
      className={`glyph-spinner ${mono ? "glyph-spinner-mono" : ""} ${className ?? ""}`}
      style={{ "--rb-cell": `${cell}px` } as CSSProperties}
      aria-hidden="true"
    >
      {MINI_RING.map((cols, row) => (
        <span className="glyph-spinner-row" data-row={row} key={row}>
          {cols.map((ring, col) => (
            <span
              className="glyph-spinner-cell"
              key={col}
              style={{ animationDelay: `${(-ring / MINI_RING_LEN) * GRADIENT_SPIN_MS}ms` }}
            />
          ))}
        </span>
      ))}
    </span>
  );
}

/**
 * `mini_mono_spinner` (loaders.rs:165): the same 2×3 grid, snake, and timing,
 * tinted by the caller's `color` — for surfaces where an accent would pull
 * focus.
 */
export function MonoSpinner({ size, className }: { size?: number; className?: string }) {
  return <GlyphSpinner size={size} mono className={className} />;
}

/**
 * `gspin_cell_phase` (proto/motion.rs:88): distance from the bottom edge plus
 * the horizontal distance from centre, normalised over `max + 1` — the pulse
 * converges on the top-centre. The denominator is `max + 1` (proto test
 * `the_gradient_wave_travels_upward`), NOT `max`.
 */
function matrixPhase(row: number, col: number): number {
  const centre = (MATRIX_SIDE - 1) / 2;
  const max = MATRIX_SIDE - 1 + centre;
  const d = MATRIX_SIDE - 1 - row + Math.abs(col - centre);
  return d / (max + 1);
}

export function MatrixSpinner({ size = 24, className }: { size?: number; className?: string }) {
  // w = h = cell * 5 (3 cells + 2 half-gaps horizontally and vertically).
  const cell = size / 5;
  return (
    <span
      className={`matrix-spinner ${className ?? ""}`}
      style={{ "--rb-cell": `${cell}px` } as CSSProperties}
      aria-hidden="true"
    >
      {Array.from({ length: MATRIX_SIDE }, (_, row) => (
        <span className="matrix-spinner-row" data-row={row} key={row}>
          {Array.from({ length: MATRIX_SIDE }, (_, col) => (
            <span
              className="glyph-spinner-cell"
              key={col}
              // The matrix runs at half speed on the desktop (`pulse_delta_slow`).
              style={{ animationDelay: `${-matrixPhase(row, col) * GRADIENT_SPIN_SLOW_MS}ms`, animationDuration: `${GRADIENT_SPIN_SLOW_MS}ms` }}
            />
          ))}
        </span>
      ))}
    </span>
  );
}

// ---------------------------------------------------------------------------
// The zeron pulse loaders (`ZERON_PULSE` = 2400ms, proto/motion.rs:16)
// ---------------------------------------------------------------------------

/** `ZERON_PULSE_MS` (proto/motion.rs:16). */
const ZERON_PULSE_MS = 2400;
/** `PULSE_MIN_OPACITY` / `PULSE_MIN_SCALE` (proto/motion.rs:26-28) — the rest state. */
const PULSE_MIN_OPACITY = 0.08;
const PULSE_MIN_SCALE = 0.9;
/** `PULSE_STAGGER` (proto/motion.rs:30): 0.15s of the 2.4s period, per cell. */
export const PULSE_STAGGER = 0.15 / 2.4;
/** `ZERON_CELLS` (proto/motion.rs:21) — the wave loader's cell count. */
export const ZERON_CELLS = 5;
/** `MARK_SPREAD` (proto/motion.rs:108) — the mark's sweep window. */
export const MARK_SPREAD = 0.55;

/**
 * `MARK_CELLS` (proto/motion.rs:99-105) — the zeron mark's pixels, `[x, y]`
 * of each 100×100 cell on the 820×940 canvas. Geometry, not style: copied
 * verbatim so the wave sweeps the same flight axis.
 */
export const MARK_CELLS: readonly (readonly [number, number])[] = [
  [0, 600], [0, 720], [240, 840], [240, 720], [120, 840], [120, 600], [240, 600],
  [0, 480], [0, 360], [480, 840], [480, 720], [120, 360], [120, 240], [240, 360],
  [600, 720], [480, 600], [360, 360], [240, 240], [600, 600], [720, 600], [720, 480],
  [240, 120], [600, 380], [720, 240], [720, 0], [480, 240], [480, 0], [120, 480],
  [240, 480], [360, 840], [360, 720], [360, 600], [360, 480], [120, 720],
];

/**
 * `mark_cell_stagger` (proto/motion.rs:114-117): per-cell stagger along the
 * flight axis — the tail tip leads, the head anchors the sweep. The stagger
 * ADDS phase, so it becomes a negative CSS delay.
 */
export function markCellStagger(x: number, y: number): number {
  const t = (820 - x + y) / 1660;
  return (1 - t) * MARK_SPREAD;
}

export interface ZeronMarkLoaderProps {
  /** The mark's height in px; the width follows the 820:940 canvas (loaders.rs:32-33). */
  readonly height?: number;
  readonly className?: string;
}

/**
 * `zeron_mark_loader` (loaders.rs:30-67): the full logo pixel grid with a
 * light wave sweeping tail→head. Each cell rests dim (0.08, scale 0.9) and
 * flares to full as the crest passes; the animated cell breathes inside its
 * fixed slot, so layout never moves.
 */
export function ZeronMarkLoader({ height = 140, className }: ZeronMarkLoaderProps) {
  const scale = height / 940;
  const cell = 100 * scale;
  return (
    <span
      className={`mark-loader ${className ?? ""}`}
      style={{ width: `${820 * scale}px`, height: `${height}px` }}
      aria-hidden="true"
    >
      {MARK_CELLS.map(([x, y], ix) => (
        <span
          key={ix}
          className="mark-loader-slot"
          style={{ left: `${x * scale}px`, top: `${y * scale}px`, width: `${cell}px`, height: `${cell}px` }}
        >
          <span
            className="mark-loader-cell"
            style={{
              borderRadius: `${16 * scale}px`,
              animationDelay: `${-markCellStagger(x, y) * ZERON_PULSE_MS}ms`,
            }}
          />
        </span>
      ))}
    </span>
  );
}

/**
 * `zeron_loader` (loaders.rs:74-105): the wave loader — 5 cells pulsing
 * opacity 0.08→1 / scale 0.9→1 over 2.4s, each cell 0.15s behind the one
 * before it (a POSITIVE delay: the desktop's `staggered_phase` subtracts).
 */
export function ZeronLoader({ cell = 8, className }: { cell?: number; className?: string }) {
  return (
    <span
      className={`zeron-loader ${className ?? ""}`}
      style={{ "--rb-cell": `${cell}px`, gap: `${cell / 2}px` } as CSSProperties}
      aria-hidden="true"
    >
      {Array.from({ length: ZERON_CELLS }, (_, i) => (
        <span
          key={i}
          className="zeron-loader-slot"
          style={{ width: `${cell}px`, height: `${cell}px`, borderRadius: `${cell / 4}px` }}
        >
          <span
            className="zeron-loader-cell"
            style={{ animationDelay: `${i * PULSE_STAGGER * ZERON_PULSE_MS}ms` }}
          />
        </span>
      ))}
    </span>
  );
}

// ---------------------------------------------------------------------------
// `upload_progress_ring` (loaders.rs:283-334)
// ---------------------------------------------------------------------------

/** `RING_STROKE` (loaders.rs:272). */
const RING_STROKE = 2.5;

export interface UploadProgressRingProps {
  /** 0–100. */
  readonly percent: number;
  /** The ring's outer diameter; the sending thumbnail overlay uses 34. */
  readonly diameter?: number;
  readonly className?: string;
}

/**
 * `upload_progress_ring` (loaders.rs:283-334): a faint full track plus a
 * bright arc growing clockwise from 12 o'clock, percent centered. Fixed
 * white-on-wash palette (`hsla(0,0,1,·)`, loaders.rs:313-314 — NOT theme
 * roles): the caller dims the image behind it, which reads in both themes.
 * Ticket 17 wires it onto the sending thumbnail.
 */
export function UploadProgressRing({ percent, diameter = 34, className }: UploadProgressRingProps) {
  const frac = Math.min(Math.max(percent, 0), 100) / 100;
  const radius = diameter / 2 - RING_STROKE;
  const circumference = 2 * Math.PI * radius;
  return (
    <span
      className={`upload-ring ${className ?? ""}`}
      style={{ width: `${diameter}px`, height: `${diameter}px` }}
      role="status"
      aria-label={`${percent}% uploaded`}
    >
      <svg viewBox={`0 0 ${diameter} ${diameter}`} aria-hidden="true">
        {/* Rotated so the arc starts at 12 o'clock, like the desktop's paths. */}
        <g transform={`rotate(-90 ${diameter / 2} ${diameter / 2})`} fill="none" strokeWidth={RING_STROKE}>
          <circle cx={diameter / 2} cy={diameter / 2} r={radius} className="upload-ring-track" />
          {frac > 0 && (
            <circle
              cx={diameter / 2}
              cy={diameter / 2}
              r={radius}
              className="upload-ring-arc"
              strokeDasharray={`${circumference * frac} ${circumference}`}
              strokeLinecap="butt"
            />
          )}
        </g>
      </svg>
      {/* The label carries the RAW percent (loaders.rs:331); only the arc clamps. */}
      <span className="upload-ring-label">{`${percent}%`}</span>
    </span>
  );
}
