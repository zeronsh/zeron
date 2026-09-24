/**
 * `RbTooltip` — the visual-only label primitive on Base UI's Tooltip
 * (toolbar buttons, terminal chips, breadcrumb segments, queue action
 * hints). The docs' own rule is encoded here: tooltips are supplementary
 * visuals only — the trigger needs its own `aria-label`; anything that
 * carries content (the 260px Context-window card, rail preview, badge
 * card) is an `RbPopover` with `openOnHover`, not a tooltip.
 *
 * The parity rules, encoded ONCE:
 * - **280ms default show delay** (the sidebar's HOVER_DELAY family; per-
 *   ticket delays of 250–350ms override at the trigger). `RbTooltipProvider`
 *   groups tooltips that share a delay and opens adjacent ones instantly
 *   after a close within the 400ms `timeout`.
 * - **Positioning:** the same `noFlipPositionerProps` clamp-only preset as
 *   every other wrapper (default side `top`, align `center`, 6px offset —
 *   labels sit clear of the glyph).
 * - **Reduced motion / instant transitions:** Base UI marks instant popup
 *   transitions with `data-instant`; `.rb-tooltip-popup[data-instant]`
 *   snaps (CSS in app.css).
 * - **Close on click** is Base UI's default (a click dismisses the label),
 *   matching the desktop's click-away behavior.
 * - **Virtual-anchor mode** (the composer's mention tooltip): surfaces whose
 *   hover target is not a DOM element — a point in a textarea's mirror —
 *   pass `anchor` (a DOM element or `virtualAnchorAt(x, y)`) with a
 *   controlled `open` and NO trigger part. The consumer owns the hover
 *   intent (its hit-testing is manual by construction); this wrapper owns
 *   the positioning, the portal, and the popup chrome, exactly as in the
 *   trigger mode.
 */

import type { ReactNode, Ref } from "react";
import { Tooltip, type TooltipProviderProps, type TooltipTriggerProps } from "@base-ui/react/tooltip";
import { noFlipPositionerProps, type AnchorPlacement, type VirtualAnchor } from "./positioning";

export type { VirtualAnchor };

/** The tooltip family's default show delay (HOVER_DELAY). */
export const RB_TOOLTIP_HOVER_DELAY = 280;

export interface RbTooltipProviderProps extends Omit<TooltipProviderProps, "children"> {
  readonly children: ReactNode;
}

/** `RbTooltipProvider` — shared delay/timeout group for a toolbar's labels. */
export function RbTooltipProvider(props: RbTooltipProviderProps) {
  const { delay = RB_TOOLTIP_HOVER_DELAY, ...rest } = props;
  return <Tooltip.Provider delay={delay} {...rest} />;
}

export interface RbTooltipTriggerProps extends Omit<TooltipTriggerProps, "delay"> {
  /** The show delay; defaults to the family's 280ms. */
  readonly delay?: number;
}

/**
 * `RbTooltipTrigger` — the label's trigger part with the 280ms default
 * (hover/focus opens after the delay). Use `render` to merge onto an
 * existing element; the target still needs its own `aria-label`.
 */
export function RbTooltipTrigger(props: RbTooltipTriggerProps) {
  const { delay = RB_TOOLTIP_HOVER_DELAY, ...rest } = props;
  return <Tooltip.Trigger delay={delay} {...rest} />;
}

export interface RbTooltipProps {
  /** The label's visual content (a plain string in every current design). */
  readonly label: ReactNode;
  /** The trigger part — an `RbTooltipTrigger` (or raw `Tooltip.Trigger`).
   *  Omitted in the virtual-anchor mode (a controlled `open` + `anchor`). */
  readonly children?: ReactNode;
  /** Default: top/center, 6px offset. */
  readonly placement?: AnchorPlacement;
  /** Extra classes on the popup beyond `.rb-tooltip-popup`. */
  readonly popupClassName?: string;
  /** The positioning anchor — a DOM element or a `virtualAnchorAt(x, y)`
   *  point (the composer's mention tooltip). Defaults to the trigger. */
  readonly anchor?: HTMLElement | VirtualAnchor;
  /** Controlled open — the virtual-anchor consumer drives every transition. */
  readonly open?: boolean;
  /** The popup's ref — the virtual-anchor consumer reads its rect (the
   *  pointer-inside-the-popup check the hover intent needs). */
  readonly popupRef?: Ref<HTMLDivElement>;
}

/** `RbTooltip` — Root + trigger + Positioner + Popup pre-wired. */
export function RbTooltip(props: RbTooltipProps) {
  const placement: AnchorPlacement = props.placement ?? { side: "top", align: "center" };
  return (
    <Tooltip.Root open={props.open}>
      {props.children}
      <Tooltip.Portal>
        <Tooltip.Positioner className="rb-tooltip-positioner" {...noFlipPositionerProps(placement)} anchor={props.anchor}>
          <Tooltip.Popup ref={props.popupRef} className={`rb-tooltip-popup ${props.popupClassName ?? ""}`}>
            {props.label}
          </Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
