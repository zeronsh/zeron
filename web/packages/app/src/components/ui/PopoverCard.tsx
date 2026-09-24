/**
 * The bare card frames — ports of the desktop's `popover.rs` element
 * builders: `popover_card`/`popover_card_flush` (`:306-331`) and
 * `palette_card` (`:820`). Geometry lives in `styles/app.css` next to
 * each class.
 *
 * `PickerCard` (the Base UI composite) renders `.popover-card` on the
 * popup itself via `cardClassName`/default — these plain frames are for
 * content that needs the card look WITHOUT the floating wrapper (inner
 * panes, in-place page swaps like the chat menu's Copy page). Pass no
 * role here; the floating surface owns semantics.
 */

import type { CSSProperties, ReactNode } from "react";

export interface PopoverCardProps {
  readonly children?: ReactNode;
  readonly className?: string;
  readonly style?: CSSProperties;
  readonly role?: string;
  readonly ariaLabel?: string;
  readonly onKeyDown?: (event: React.KeyboardEvent<HTMLDivElement>) => void;
}

/**
 * The floating-menu surface (`popover.rs:306-325`): radius 12, `1px
 * hairline(0.10)` border, the glass overlay tint over a 44px backdrop blur,
 * 4px inset, capped at 640px (`pickers.rs:2738`).
 */
export function PopoverCard(props: PopoverCardProps) {
  const { children, className, style, role, ariaLabel, onKeyDown } = props;
  return (
    <div
      className={`popover-card ${className ?? ""}`}
      style={style}
      role={role}
      aria-label={ariaLabel}
      onKeyDown={onKeyDown}
    >
      {children}
    </div>
  );
}

/** `popover_card_flush` (`popover.rs:329`) — no inset, for cards that own
 * their internal panes. */
export function PopoverCardFlush(props: PopoverCardProps) {
  return <PopoverCard {...props} className={`popover-card-flush ${props.className ?? ""}`} />;
}

export interface PaletteCardProps {
  /** Explicit card width (the desktop takes `Pixels`). */
  readonly width: number;
  /** The caller's corner radius (the add-space palette rounds differently). */
  readonly cornerRadius: number;
  readonly children?: ReactNode;
  readonly role?: string;
  readonly ariaLabel?: string;
}

/** `palette_card` (`popover.rs:820`) — the command-palette sibling of
 * `PopoverCard`: explicit width, caller's radius, no padding. */
export function PaletteCard(props: PaletteCardProps) {
  const { width, cornerRadius, children, role, ariaLabel } = props;
  return (
    <div
      className="palette-card"
      style={{ width: `${width}px`, borderRadius: `${cornerRadius}px` }}
      role={role}
      aria-label={ariaLabel}
    >
      {children}
    </div>
  );
}
