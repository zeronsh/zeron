/**
 * `RbSelect` — the settings dropdown primitive on Base UI's Select (font
 * size, font family, title harness/model pickers — small fixed option
 * sets). Their desktop equivalents are plain dropdowns, so Select's
 * built-in focus-walking keyboard is the closest match and the least
 * motion/geometry-sensitive surface in the app (blueprint §6.5).
 *
 * The parity rules, encoded ONCE:
 * - **`alignItemWithTrigger: false` (hard-coded):** Select's default `true`
 *   slides the list under the selected row (native-menu geometry); `false`
 *   gives the popover-style below placement our menus use. This is the
 *   Select-family prop a minor could silently regress; the wrapper owns it.
 * - **Positioning:** the shared `noFlipPositionerProps` clamp-only preset
 *   (fixed side, 8px window margin, never flip).
 * - **Keyboard scope:** `overlaySource` registers on the `overlayKeyboard`
 *   registry while open, same as every wrapper.
 * - **Phone arm (ticket 54, 49's explicit deferral):** at `useIsPhone()`
 *   the popup opens as 49's bottom sheet instead of the floating card —
 *   `RbSelectPositioner` renders its tree inside `RbDrawerSheet`
 *   (`.rb-drawer-card`, swipe-down dismiss, scrim press), the trigger
 *   unchanged (its press still toggles the Select, whose shadowed open
 *   state drives the sheet). The parts are consumer JSX, so the arm hands
 *   the open state from `RbSelect` down through `RbSelectPhoneContext` —
 *   provided only at ≤768, leaving the ≥769 tree byte-identical.
 * - The trigger/portal/popup/item parts carry no parity rules of their
 *   own, so they ship as verbatim re-exports (`RbSelectTrigger` and
 *   friends below) — the README rule-5 import boundary without inventing
 *   wrappers; `RbSelect` owns exactly the Root/Positioner behavior.
 */

import { createContext, useContext, useState, type ComponentProps, type ReactNode } from "react";
import { Select, type SelectRootChangeEventDetails, type SelectRootProps } from "@base-ui/react/select";
import { noFlipPositionerProps } from "./positioning";
import { useOverlayKeyboardSource } from "./overlay";
import { drawerOnOpenChange, RbDrawerSheet } from "./responsive-surface";
import { useIsPhone } from "../../state/media";

export interface RbSelectProps<Value, Multiple extends boolean | undefined = false>
  extends Omit<SelectRootProps<Value, Multiple>, "children"> {
  /** Registers this name on the `overlayKeyboard` registry while open. */
  readonly overlaySource?: string;
  /**
   * The Select children: `RbSelectTrigger` (with the value/caret markup)
   * plus `RbSelectPositioner` + `RbSelectPopup` carrying the list.
   */
  readonly children: ReactNode;
}

/**
 * The phone arm's state seam (ticket 54): `RbSelect` owns the shadowed open
 * state and `RbSelectPositioner`'s phone arm needs it to drive the sheet.
 * Select's own parts compose through the library's internal context; this
 * one carries only what the sheet consumes — the open flag and the SAME
 * change path Select's `onOpenChange` takes, so every dismissal route
 * (trigger toggle, item pick, Escape, sheet swipe/scrim) lands in the one
 * state transition.
 */
interface RbSelectPhoneState {
  readonly open: boolean;
  readonly change: (open: boolean, details: SelectRootChangeEventDetails) => void;
}

const RbSelectPhoneContext = createContext<RbSelectPhoneState | null>(null);

/**
 * `RbSelect` — Root with the open state shadowed so the overlayKeyboard
 * wiring can observe it without stealing the consumer's control: a
 * consumer-supplied `open` wins; uncontrolled consumers ride the shadow.
 * At phone the shadow (or controlled value) also feeds the sheet through
 * `RbSelectPhoneContext` — the Select stays the single source of truth.
 */
export function RbSelect<Value, Multiple extends boolean | undefined = false>(props: RbSelectProps<Value, Multiple>) {
  const { overlaySource, open, onOpenChange, children, ...root } = props;
  const [shadowOpen, setShadowOpen] = useState(false);
  const openState = open ?? shadowOpen;
  useOverlayKeyboardSource(overlaySource, openState);
  // The one change path every open/close route shares — Select's own
  // `onOpenChange` events and the sheet's dismissal events both flow
  // through here, so the shadow, the consumer, and (at phone) the sheet
  // can never disagree.
  const change = (next: boolean, details: SelectRootChangeEventDetails): void => {
    setShadowOpen(next);
    onOpenChange?.(next, details);
  };
  const phone = useIsPhone() ? { open: openState, change } : null;
  return (
    <Select.Root<Value, Multiple>
      {...root}
      open={openState}
      onOpenChange={change}
    >
      {phone !== null ? (
        <RbSelectPhoneContext.Provider value={phone}>{children}</RbSelectPhoneContext.Provider>
      ) : (
        children
      )}
    </Select.Root>
  );
}

/**
 * `RbSelectPositioner` — the dropdown's placement: `alignItemWithTrigger:
 * false` (popover-style below placement) on the shared clamp-only preset.
 * At ≤768 (ticket 54) the placement is 49's bottom sheet instead: the
 * positioner tree renders inside `RbDrawerSheet` pinned to the sheet's
 * flow (the engine's computed coordinates are neutralized per-key — Base
 * UI merges the consumer's `style` after its own), and the sheet's open
 * rides the Select's own state through `RbSelectPhoneContext`, so the
 * popup, items, keyboard model, and every Base UI dismissal path stay the
 * Select's own, untouched. The desktop arm is byte-identical to the
 * pre-arm tree.
 */
export function RbSelectPositioner(
  props: Omit<
    ComponentProps<typeof Select.Positioner>,
    "alignItemWithTrigger" | "side" | "align" | "collisionAvoidance" | "collisionPadding" | "positionMethod"
  >,
) {
  const isPhone = useIsPhone();
  const phone = useContext(RbSelectPhoneContext);
  if (isPhone && phone !== null) {
    const { style, ...rest } = props;
    return (
      <RbDrawerSheet
        open={phone.open}
        onOpenChange={drawerOnOpenChange(phone.change)}
      >
        <Select.Positioner
          {...noFlipPositionerProps({ side: "bottom", align: "start" })}
          alignItemWithTrigger={false}
          {...rest}
          style={{ position: "static", top: "auto", right: "auto", bottom: "auto", left: "auto", ...style }}
        />
      </RbDrawerSheet>
    );
  }
  return (
    <Select.Positioner
      {...noFlipPositionerProps({ side: "bottom", align: "start" })}
      alignItemWithTrigger={false}
      {...props}
    />
  );
}

// ---------------------------------------------------------------------------
// Verbatim part re-exports (components/README.md rule 5)
// ---------------------------------------------------------------------------

/**
 * Raw Select parts, re-exported so `@base-ui` never leaks past `base/`
 * (README rule 5): the styled Trigger, the Portal transport, the Popup
 * list, and the option Item. Pure presentation with no parity rules to
 * encode — `RbSelect` and `RbSelectPositioner` own the contract.
 */
export const RbSelectTrigger = Select.Trigger;
export const RbSelectPortal = Select.Portal;
export const RbSelectPopup = Select.Popup;
export const RbSelectItem = Select.Item;
