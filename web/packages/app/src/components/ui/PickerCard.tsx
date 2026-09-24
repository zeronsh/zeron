/**
 * `PickerCard` — the popover card shell every picker/menu surface
 * composes (the composer's four footer chips, the identity card, the
 * spaces menu, the sidebar view menu — the wiring wave 2-4 tickets were
 * re-inventing one surface at a time). It owns ONLY the assembly: the
 * detached-trigger handle (Base UI's Trigger renders as a sibling of the
 * Root, so `createRbPopoverHandle` binds them), the `RbPopoverTrigger`
 * that adopts the caller's trigger element via `render` (the element's
 * className/id/handlers/children stay its own, the trigger's toggling
 * and ARIA props merge onto it, and its `ref` composes with Base UI's),
 * and the `RbPopover` prop stack — width, role, aria, `overlaySource`,
 * the escape/focus contract. The parity rules themselves live in
 * `base/popover.tsx` (no-flip, `modal: false`, overlayKeyboard, exit
 * window, `pickers.rs:871-890`'s focus return); this file composes that
 * wrapper and re-implements nothing.
 *
 * The caller keeps: the `open` state and every dismissal path (a pick
 * handler inside the card body closes through its own callback), the
 * trigger's content and open-state styling (`ui/Chip.tsx`'s
 * `openChipClass`), the card body, and the cursor keyboard model
 * (`ui/CursorList.tsx`). Pressing a different card's trigger dismisses
 * this one and opens that one — Base UI's `trigger-press` reason, the
 * four-chip switching behavior (pickers.rs's menu switching) the wrapper
 * already carries.
 *
 * At ≤768px (ticket 49) the card body opens as the shared bottom sheet
 * instead of the floating card — see the branch in `PickerCard` below.
 */

import { useState, type CSSProperties, type ReactElement, type ReactNode } from "react";
import {
  createRbPopoverHandle,
  RbPopover,
  RbPopoverTrigger,
  type AnchorHelperId,
  type AnchorPlacement,
  type RbPopoverProps,
} from "../base/popover";
import { drawerOnOpenChange, RbDrawerSheet } from "../base/responsive-surface";
import { useIsPhone } from "../../state/media";

export interface PickerCardProps {
  /** Controlled open — the caller owns every open/close transition. */
  readonly open: boolean;
  /** Base UI's change event, verbatim through `RbPopover`. */
  readonly onOpenChange: RbPopoverProps["onOpenChange"];
  /**
   * The trigger element, adopted via `RbPopoverTrigger`'s `render` — the
   * element carries its own className/id/handlers/children/ref, the
   * trigger's toggling and ARIA props merge onto it.
   */
  readonly trigger: ReactElement;
  /**
   * Also open on hover (Base UI `openOnHover`): the content-card shape
   * `base/tooltip.tsx` routes here instead of the label tooltip — the
   * context-meter card (500ms) is the current design.
   */
  readonly openOnHover?: boolean;
  /** The hover-open delay in ms; requires `openOnHover`. */
  readonly hoverDelayMs?: number;
  /** The placement, or an old `popover-anchor.ts` helper name. Defaults to `anchorBelow`. */
  readonly placement?: AnchorPlacement | AnchorHelperId;
  /** The caller's gap for `anchorBelowGap`. */
  readonly gap?: number;
  /** The card's class list beyond the base `.popover-card` (flush/menu variants pass extra classes). */
  readonly cardClassName?: string;
  readonly role?: string;
  readonly ariaLabel?: string;
  /**
   * The card's width (the desktop's `Pixels`) — folded into the popup's
   * inline style; `style` carries anything beyond it (maxHeight…).
   */
  readonly width?: number;
  readonly style?: CSSProperties;
  /** Registers this name on the `overlayKeyboard` registry while open (shell.rs:3681-3683). */
  readonly overlaySource?: string;
  /** Per-surface: the search input ref, or `false` to keep focus put. */
  readonly initialFocus?: RbPopoverProps["initialFocus"];
  /** Where Escape returns focus (`pickers.rs:866` — the composer input). */
  readonly escapeFocusTarget?: RbPopoverProps["escapeFocusTarget"];
  /** The desktop's `motion::speed_scale`; rescales the exit duration. */
  readonly motionSpeed?: number;
  /**
   * The card frame's key handler — the cursor keyboard model's sink
   * (`ui/CursorList.tsx`'s `useCursorList`). Escape is NOT the caller's
   * job: Base UI's dismiss pipeline owns it.
   */
  readonly onKeyDown?: RbPopoverProps["onKeyDown"];
  readonly children: ReactNode;
}

/** `PickerCard` — trigger + popover card, pre-wired as one unit. */
export function PickerCard(props: PickerCardProps) {
  const [handle] = useState(() => createRbPopoverHandle());
  // The responsive branch lives HERE (one place — every consumer converts
  // at once): at ≤768px the card body opens as the shared bottom sheet
  // (`base/responsive-surface.tsx`'s `RbDrawerSheet`, ticket 49) with the
  // trigger unchanged — same element, same classes; its press toggles the
  // sheet through `Drawer.Trigger`'s `render` adoption exactly as the
  // popover form adopts it, and the pressed/expanded styling keeps
  // following the caller's controlled `open` flag. `placement`, `gap`, and
  // `width` are ignored (the sheet spans the viewport), as are
  // `openOnHover`/`hoverDelayMs` (no hover layer to arm under a modal
  // sheet) and `escapeFocusTarget`/`motionSpeed` (the sheet's modal
  // contract: Base UI's default focus return, no popover exit window).
  // The card body itself renders unchanged — the sheet replaces
  // placement, not the card's inner layout.
  const isPhone = useIsPhone();
  if (isPhone) {
    return (
      <RbDrawerSheet
        open={props.open}
        onOpenChange={drawerOnOpenChange(props.onOpenChange)}
        trigger={props.trigger}
        role={props.role}
        ariaLabel={props.ariaLabel}
        initialFocus={props.initialFocus}
        cardClassName={props.cardClassName ?? "popover-card"}
        overlaySource={props.overlaySource}
        onKeyDown={props.onKeyDown}
      >
        {props.children}
      </RbDrawerSheet>
    );
  }
  const style: CSSProperties | undefined =
    props.width === undefined ? props.style : { ...props.style, width: props.width };
  return (
    <>
      <RbPopoverTrigger
        handle={handle}
        render={props.trigger}
        openOnHover={props.openOnHover}
        delay={props.hoverDelayMs}
      />
      <RbPopover
        handle={handle}
        open={props.open}
        onOpenChange={props.onOpenChange}
        placement={props.placement}
        gap={props.gap}
        cardClassName={props.cardClassName}
        role={props.role}
        ariaLabel={props.ariaLabel}
        style={style}
        overlaySource={props.overlaySource}
        initialFocus={props.initialFocus}
        escapeFocusTarget={props.escapeFocusTarget}
        motionSpeed={props.motionSpeed}
        onKeyDown={props.onKeyDown}
      >
        {props.children}
      </RbPopover>
    </>
  );
}
