/**
 * `Tooltip` — the app's label tooltip over `base/tooltip.tsx`'s
 * `RbTooltip` (which owns the parity contract: no-flip clamp-only
 * positioning, the visual-only rule — triggers carry their own
 * `aria-label`; content cards are `PickerCard`s, not tooltips). This
 * thin layer adds the app's delay conventions so surfaces stop
 * hand-rolling timers:
 *
 * - **280ms** — the family default (`RbTooltip`'s HOVER_DELAY).
 * - **350ms** — the view-options label (`spaces.rs:960-966`), the lone
 *   tooltip in the popover family with its own delay.
 * - **500ms** — the context-meter card's hover (gpui
 *   `DEFAULT_TOOLTIP_SHOW_DELAY`, zui `div.rs:49`).
 *
 * The one inline-span exception: the view-options label renders as a
 * positioned span INSIDE its trigger (suppressed while that trigger's
 * popover is open, spaces.rs:966-973) — not a portal popup, so it keeps
 * its hand-rolled span and imports the delay constant. Surfaces with
 * that exact suppression shape follow it; everything else lands here.
 *
 * The virtual-anchor mode (the composer's mention tooltip, ticket 18):
 * pass `anchor` + a controlled `open` and NO trigger — the consumer owns
 * the hover intent (its hit-testing is manual by construction: the chips
 * live in a mirror under the textarea), the family owns the positioning,
 * the portal, and the popup chrome. `virtualAnchorAt` is re-exported so
 * those consumers never reach past this layer.
 */

import type { ReactElement, ReactNode, Ref } from "react";
import type { AnchorPlacement } from "../base/positioning";
import { RbTooltip, RbTooltipTrigger, type VirtualAnchor } from "../base/tooltip";

export type { VirtualAnchor };
export { virtualAnchorAt } from "../base/positioning";

/** The view-options label's 350ms show delay (`spaces.rs:960`). */
export const TOOLTIP_VIEW_OPTIONS_MS = 350;
/** The context-meter card's 500ms hover (gpui `DEFAULT_TOOLTIP_SHOW_DELAY`). */
export const TOOLTIP_CONTEXT_METER_MS = 500;

export interface TooltipProps {
  /** The label's visual content (a plain string in every current design). */
  readonly label: ReactNode;
  /** The show delay; defaults to the family's 280ms. */
  readonly delay?: number;
  /** Default: top/center, 6px offset. */
  readonly placement?: AnchorPlacement;
  /** Extra classes on the popup beyond `.rb-tooltip-popup`. */
  readonly popupClassName?: string;
  /** The trigger element, adopted via `RbTooltipTrigger`'s `render`.
   *  Omitted in the virtual-anchor mode (`anchor` + controlled `open`). */
  readonly trigger?: ReactElement;
  /** The positioning anchor — a DOM element or a `virtualAnchorAt(x, y)`
   *  point; replaces the trigger as the positioner's anchor. */
  readonly anchor?: HTMLElement | VirtualAnchor;
  /** Controlled open — the virtual-anchor consumer drives every transition. */
  readonly open?: boolean;
  /** The popup's ref — the virtual-anchor consumer reads its rect (the
   *  pointer-inside-the-popup check its hover intent needs). */
  readonly popupRef?: Ref<HTMLDivElement>;
}

/** `Tooltip` — label + trigger, pre-wired with the family's delay. */
export function Tooltip(props: TooltipProps) {
  return (
    <RbTooltip
      label={props.label}
      placement={props.placement}
      popupClassName={props.popupClassName}
      anchor={props.anchor}
      open={props.open}
      popupRef={props.popupRef}
    >
      {props.trigger !== undefined && <RbTooltipTrigger delay={props.delay} render={props.trigger} />}
    </RbTooltip>
  );
}
