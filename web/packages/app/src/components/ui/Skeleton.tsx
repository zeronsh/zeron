/**
 * Loading and error states — ports of `popover.rs:1080-1163`: the pulsing
 * skeleton rows, ghost menu-row bars (deterministic width ladder), the
 * single skeleton bar, and the inline error row with its Retry affordance.
 *
 * Every skeleton pulses on the shared 2.4s `ZERON_PULSE` clock with a 0.08
 * per-row stagger, phase-locked across the app: each row's CSS animation
 * gets a negative delay computed from one module-level epoch, so mounted-at-
 * different-times loaders still breathe in lockstep. Reduced motion freezes
 * them at the pulse floor via the `rb-skeleton-pulse` media query.
 */

import type { CSSProperties } from "react";

/** `motion::ZERON_PULSE` period (2400ms). */
export const PULSE_MS = 2400;
/** `staggered_phase`'s per-index stagger (`motion.rs:51-57`). */
export const PULSE_STAGGER = 0.08;

let epoch: number | null = null;

function sharedEpoch(): number {
  if (epoch === null) {
    epoch = performance.now();
  }
  return epoch;
}

/**
 * The negative animation delay that phase-locks row `index` to the shared
 * clock: `-(phase_at_mount - index * stagger) * PULSE_MS`, matching
 * `staggeredPhase(raw, i, stagger) = (raw - i*stagger).rem_euclid(1)`.
 */
export function pulseDelayMs(index: number, stagger: number = PULSE_STAGGER): number {
  const raw = (((performance.now() - sharedEpoch()) / PULSE_MS) % 1 + 1) % 1;
  const phase = (((raw - index * stagger) % 1) + 1) % 1;
  return -phase * PULSE_MS;
}

function pulseStyle(index: number, stagger: number = PULSE_STAGGER): CSSProperties {
  return { animationDelay: `${pulseDelayMs(index, stagger)}ms` };
}

/** `skeleton_rows` (`popover.rs:1080-1103`) — full-width 28px slabs. Used
 * by the ref popover body and (outside this surface) the composer and
 * Settings pages. */
export function SkeletonRows({ count }: { count: number }) {
  return (
    <div className="skeleton-rows" aria-hidden>
      {Array.from({ length: count }, (_, ix) => (
        <div key={ix} className="skeleton-row" style={pulseStyle(ix)} />
      ))}
    </div>
  );
}

/**
 * `skeleton_menu_rows` (`popover.rs:1124-1150`) — 14px ghost labels whose
 * widths cycle the deterministic `WIDTHS` ladder (42/58/48/66%), so the
 * stagger reads organic without randomness.
 */
const WIDTHS = [42, 58, 48, 66] as const;

export function SkeletonMenuRows({ count }: { count: number }) {
  return (
    <div className="skeleton-menu-rows" aria-hidden>
      {Array.from({ length: count }, (_, ix) => (
        <div
          key={ix}
          className="skeleton-menu-row"
          style={{ width: `${WIDTHS[ix % WIDTHS.length]}%`, ...pulseStyle(ix) }}
        />
      ))}
    </div>
  );
}

/** `skeleton_bar` (`popover.rs:1108-1117`) — one ghost label of an explicit
 * pixel width (the trigger chip's label slot). */
export function SkeletonBar({ width }: { width: number }) {
  return <div className="skeleton-bar" style={{ width: `${width}px`, ...pulseStyle(0, 0) }} aria-hidden />;
}

export interface ErrorRowProps {
  readonly message: string;
  /** When provided, the inline Retry affordance renders inside the row
   * (`pickers.rs:2804-2838`) — one component, positioned by the card. */
  readonly onRetry?: () => void;
}

/** `error_row` (`popover.rs:1154-1163`) — the inline error + Retry. */
export function ErrorRow(props: ErrorRowProps) {
  return (
    <div className="error-row" role="alert">
      <span>{props.message}</span>
      {props.onRetry !== undefined && (
        <button type="button" className="error-row-retry" onClick={props.onRetry}>
          Retry
        </button>
      )}
    </div>
  );
}
