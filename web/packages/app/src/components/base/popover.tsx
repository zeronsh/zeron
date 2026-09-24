/**
 * `RbPopover` — the web's anchored floating card on Base UI's Popover,
 * pre-wired to the desktop's parity contract. This file is where the
 * port citations live now (the map from desktop contract → Base UI prop →
 * our CSS), replacing `components/popover/popup.tsx`'s hand-rolled layer
 * one consumer at a time (Phase 2 of the adoption blueprint).
 *
 * The parity rules, encoded ONCE:
 * - **Positioning (popover.rs:420-638):** fixed side, clamp-only, never
 *   flip — the Positioner gets `noFlipPositionerProps` (`side: 'shift'`,
 *   `fallbackAxisSide: 'none'`, `collisionPadding: 8`, `positionMethod:
 *   'fixed'`). The caller picks side/align/offset; `anchorHelperPlacement`
 *   maps the old `popover-anchor.ts` helper names. Floating UI tracks the
 *   live anchor (layout shift/resize/scroll), killing the measure-then-place
 *   0,0 race class outright.
 * - **Non-modal (popover.rs has no occluding backdrop for menus):**
 *   `modal: false` — outside presses dismiss, no focus trap, no scroll lock.
 * - **Escape vs dismissal focus (pickers.rs:871-890):** `finalFocus`
 *   consults the recorded dismissal `reason` through
 *   `escapeFinalFocusTarget` — `escape-key` returns focus to
 *   `escapeFocusTarget` (the composer), everything else leaves focus where
 *   it landed. `initialFocus` is the caller's (the search input ref,
 *   spaces.rs:1207, or `false`).
 * - **Exit window (popover.rs:346-411):** CSS owns the motion —
 *   `.rb-popover-popup[data-open]` plays `rb-menu-in`, `[data-closed]`
 *   plays `rb-menu-out` and turns the card dead to hit-testing with a
 *   full-bleed `::after` occluder (popover.rs:406's contract as pure CSS).
 *   Base UI keeps the popup mounted and painting until `getAnimations()`
 *   drains — the keep-painting-through-exit contract, structurally.
 *   `motionSpeed` (the desktop's `motion::speed_scale`) rescales
 *   `--rb-motion-menu-out`; the unmount follows whatever duration results.
 *   Deviation from the desktop's reap timer, accepted per the blueprint:
 *   the +20ms grace is gone (unmount lands at animation end — the timer's
 *   intent).
 * - **Keyboard scope (shell.rs:3681-3683):** `overlaySource` registers on
 *   the `overlayKeyboard` registry while open, so session-nav shortcuts go
 *   quiet under the popover exactly as they do under today's hand-rolled
 *   `Popup`.
 *
 * Search-driven pickers use THIS part (Popover has no item semantics); action
 * menus without a search field use `RbMenu` instead (see menu.tsx). The
 * cursor keyboard model (`menuStep`/`classifyKey`) stays consumer-side and
 * never interacts with anything here.
 */

import { useRef, type CSSProperties, type ReactNode } from "react";
import {
  Popover,
  type PopoverPositionerProps,
  type PopoverPopupProps,
  type PopoverRootProps,
} from "@base-ui/react/popover";
import {
  anchorHelperPlacement,
  escapeFinalFocusTarget,
  exitMotionMs,
  nestedMenuPlacement,
  noFlipPositionerProps,
  shouldVetoDismissal,
  virtualAnchorAt,
  type AnchorHelperId,
  type AnchorPlacement,
  type NestedMenuSide,
  type VirtualAnchor,
} from "./positioning";
import { useOverlayKeyboardSource } from "./overlay";

export { anchorHelperPlacement, nestedMenuPlacement, noFlipPositionerProps, shouldVetoDismissal, virtualAnchorAt };
export type { AnchorHelperId, AnchorPlacement, NestedMenuSide, VirtualAnchor };

/**
 * `RbPopoverTrigger` — Base UI's `Popover.Trigger` re-exported under the
 * wrapper's namespace (blueprint §6.2): the trigger's click toggles with the
 * `trigger-press` reason, which replaces the old layer's
 * `noteTriggerPress`/`takePressWasOpen` dance — a press on the trigger that
 * found the popup open leaves it closed (no close-then-reopen flicker), and
 * a press on a DIFFERENT chip's trigger dismisses the first popup and opens
 * that chip's own (menus switch). While open the element carries
 * `data-popup-open`; style the open-snap off it or off the controlled
 * `open` state, whichever the consumer already keys its CSS classes on.
 */
export const RbPopoverTrigger = Popover.Trigger;

/** The detached-trigger handle type (`Popover.createHandle`'s return). */
export type RbPopoverHandle = ReturnType<typeof Popover.createHandle>;

/**
 * `createRbPopoverHandle` — Base UI's `Popover.createHandle` re-exported
 * for wave-2 surfaces that want one popover shared by multiple triggers
 * with per-trigger payloads (the blueprint's §6.2 multi-chip shape).
 */
export const createRbPopoverHandle = Popover.createHandle;

export interface RbPopoverProps {
  /**
   * The detached-trigger handle (Base UI's detached pattern): the consumer
   * renders the trigger as a sibling via `createRbPopoverHandle()` and passes
   * the same handle to both the `RbPopoverTrigger` and here — a Trigger
   * outside the Root without a handle is a Base UI invariant error.
   */
  readonly handle?: RbPopoverHandle;
  /** Controlled open — every parity consumer is controlled. */
  readonly open: boolean;
  /**
   * Base UI's change event verbatim (reason: `trigger-press`,
   * `outside-press`, `escape-key`, `focus-out`, …, plus `cancel()` /
   * `allowPropagation()` / `preventUnmountOnClose()`). The wrapper records
   * the reason for the finalFocus decision before forwarding.
   */
  readonly onOpenChange: NonNullable<PopoverRootProps["onOpenChange"]>;
  /** Fires when the open/close animations have fully completed. */
  readonly onOpenChangeComplete?: PopoverRootProps["onOpenChangeComplete"];
  /** Imperative `unmount`/`close` (externally driven exits). */
  readonly actionsRef?: PopoverRootProps["actionsRef"];
  /**
   * The placement, or an old `popover-anchor.ts` helper name. Defaults to
   * `anchorBelow` (side bottom, align start, gap 6).
   */
  readonly placement?: AnchorPlacement | AnchorHelperId;
  /** The caller's gap for `anchorBelowGap`. */
  readonly gap?: number;
  /**
   * The anchor to position against — a trigger `ref`, an `Element`, or a
   * `virtualAnchorAt(x, y)` point (caret menus, pointer context menus).
   * Defaults to Base UI's trigger.
   */
  readonly anchor?: PopoverPositionerProps["anchor"];
  /** The card's class list beyond the base `.popover-card` (flush/menu variants pass extra classes). */
  readonly cardClassName?: string;
  readonly style?: CSSProperties;
  readonly role?: string;
  readonly ariaLabel?: string;
  /** Per-surface: the search input ref, or `false` to keep focus put. */
  readonly initialFocus?: PopoverPopupProps["initialFocus"];
  /** Where Escape returns focus (`pickers.rs:866` — the composer input). */
  readonly escapeFocusTarget?: HTMLElement | (() => HTMLElement | null);
  /** Registers this name on the `overlayKeyboard` registry while open. */
  readonly overlaySource?: string;
  /** The desktop's `motion::speed_scale`; rescales the exit duration. */
  readonly motionSpeed?: number;
  /**
   * The card frame's key handler — keys bubbling from the focused search
   * input land here (the consumer's cursor keyboard model, §6.5). Escape is
   * NOT the consumer's job: Base UI's dismiss pipeline owns it, which is
   * what feeds the `escape-key` reason the finalFocus decision reads.
   */
  readonly onKeyDown?: (event: React.KeyboardEvent<HTMLDivElement>) => void;
  readonly children: ReactNode;
}

/** `RbPopover` — Root+Portal+Positioner+Popup pre-wired (blueprint §2). */
export function RbPopover(props: RbPopoverProps) {
  const lastReasonRef = useRef<string | null>(null);
  useOverlayKeyboardSource(props.overlaySource, props.open);

  const placement: AnchorPlacement =
    props.placement === undefined || typeof props.placement === "string"
      ? anchorHelperPlacement((props.placement ?? "anchorBelow") as AnchorHelperId, props.gap)
      : props.placement;
  const positionerProps = noFlipPositionerProps(placement);

  const motionStyle =
    props.motionSpeed === undefined
      ? undefined
      : ({ "--rb-motion-menu-out": `${exitMotionMs(props.motionSpeed)}ms` } as CSSProperties);

  return (
    <Popover.Root
      handle={props.handle}
      open={props.open}
      onOpenChange={(open, details) => {
        if (!open && shouldVetoDismissal(details.reason)) {
          // Parity veto (ticket 09 gap rows 29/87, `popover.rs` has no Tab
          // handling and no focus trap): Base UI's non-modal popovers close
          // when focus moves out — the desktop's stay open, and the old
          // hand-rolled layer matched the desktop. Cancel the dismissal
          // before the store ever sees it; outside presses and Escape still
          // dismiss normally.
          details.cancel();
          return;
        }
        lastReasonRef.current = details.reason;
        props.onOpenChange(open, details);
      }}
      onOpenChangeComplete={props.onOpenChangeComplete}
      actionsRef={props.actionsRef}
      modal={false}
    >
      <Popover.Portal>
        <Popover.Positioner
          className="rb-popover-positioner"
          {...positionerProps}
          anchor={props.anchor}
        >
          <Popover.Popup
            className={`rb-popover-popup ${props.cardClassName ?? "popover-card"}`}
            style={{ ...props.style, ...motionStyle }}
            role={props.role}
            aria-label={props.ariaLabel}
            initialFocus={props.initialFocus}
            onKeyDown={props.onKeyDown}
            finalFocus={(closeType) => {
              const target = escapeFinalFocusTarget(lastReasonRef.current, props.escapeFocusTarget);
              if (target === false) {
                return false;
              }
              return typeof target === "function" ? target() : target;
            }}
          >
            {props.children}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}
