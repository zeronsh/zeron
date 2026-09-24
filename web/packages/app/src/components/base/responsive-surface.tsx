/**
 * `RbResponsiveDialog` — the responsive dialog surface (ticket 49, research
 * M8 §(b)): at `useIsPhone()` the dialog renders as a bottom SHEET on Base
 * UI's Drawer, at every other width it is `RbDialog`'s exact tree. The
 * desktop has no phone analog for the sheet form — its minimum window
 * geometry (`CHAT_PANEL_MIN` 300 + `SIDEBAR_MIN` 224) makes a 375px window
 * impossible — so the sheet is mobile-native spec, not a port.
 *
 * - **One breakpoint, both clients:** the arm flips on `useIsPhone()`
 *   (`state/media.ts`) — the same `(max-width: 768px)` query every phone
 *   block in `app.css` keys, so JS and CSS agree by construction. The node
 *   environment and the pre-measure render resolve `false` (desktop) — the
 *   no-op arm, per the spec amendment's "everything changes nothing at
 *   ≥769px".
 * - **Desktop arm:** `<RbDialog {...props} />` — byte-identical to ticket
 *   09's contract because it IS that component; this wrapper adds no tree,
 *   no classes, no registration of its own.
 * - **Phone arm (`RbDrawerSheet`, shared):** `Drawer.Root {open,
 *   onOpenChange, modal}` → `Drawer.Portal` → `Backdrop` + `Viewport` →
 *   `Popup className="modal-card rb-drawer-card …"` — `swipeDirection`
 *   `'down'` (dismiss toward the bottom edge; the sheet tracks the finger)
 *   and NO snap points (the research: "the dialogs are small"). The
 *   `Viewport` is Base UI's swipe/scroll-lock host: a `Drawer.Popup`
 *   outside one disables the gesture outright (the library's own
 *   invariant), so the sheet body always renders through it. `modal: true`
 *   gives the sheet the same semantics the Dialog provides — focus
 *   trapped, document scroll locked, pointer interactions outside
 *   disabled; Escape routes to `onOpenChange(false)`.
 * - **Scrim contract per wrapper:** the dialog sheet carries
 *   `disablePointerDismissal` (mirroring `RbDialog`'s scrim —
 *   `base/dialog.tsx:76` — so only Escape/Cancel close the rename/delete
 *   dialogs); the glass and picker sheets do not (their scrim presses
 *   close, matching `RbDialogGlass` and the pickers' outside-press).
 * - **Sheet geometry (`.rb-drawer-card`, `app.css`):** fixed at
 *   left/right/bottom 0, radius `16px 16px 0 0`, `max-height:
 *   calc(100dvh - var(--rb-space-lg))`, z-index via `.modal-backdrop` =
 *   `--rb-z-modal` (70) — the ladder's modal tier, no new tiers. The sheet
 *   replaces PLACEMENT, not the card's inner layout — and (ticket 15,
 *   research W4) every drawer CARD reads as the sheet itself: the dialog
 *   arm's inner `.dialog-card` stretches to the full width (one phone
 *   rule, `.rb-drawer-card .dialog-card { width: 100% }` — the old 360px
 *   child painted ~90% of a 390px viewport flush-left and read as a
 *   "weird dialog"), and each arm's own bottom padding composes with
 *   `env(safe-area-inset-bottom)` so the content clears the home
 *   indicator. The popover rows and the add-space 600px cap keep their
 *   values inside the sheet.
 * - **Motion:** the entrance reuses `rb-dialog-in` keyed to `[data-open]`
 *   (the `.rb-dialog-card` pattern, `app.css:4846-4848`); reduced motion
 *   snaps it off. The swipe-dismiss is gesture-driven, not an animation.
 * - **Keyboard scope:** the sheet registers on `overlayKeyboard` through
 *   the same `useOverlayKeyboardSource` seam the floating forms use, so
 *   session-nav shortcuts stay quiet under it (`base/overlay.ts`).
 */

import type { CSSProperties, KeyboardEvent, ReactElement, ReactNode } from "react";
import { Drawer, type DrawerPopupProps, type DrawerRootProps } from "@base-ui/react/drawer";
import { useOverlayKeyboardSource } from "./overlay";
import { useIsPhone } from "../../state/media";
import { RbDialog, type RbDialogProps } from "./dialog";

/** `RbResponsiveDialog` — `RbDialogProps` verbatim; a drop-in for `RbDialog`. */
export function RbResponsiveDialog(props: RbDialogProps) {
  const isPhone = useIsPhone();
  if (!isPhone) {
    // The exact `RbDialog` tree — rendered as that component, so ticket 09's
    // contract holds byte-for-byte at ≥769px.
    return <RbDialog {...props} />;
  }
  return (
    <RbDrawerSheet
      open={props.open}
      onOpenChange={drawerOnOpenChange(props.onOpenChange)}
      onOpenChangeComplete={props.onOpenChangeComplete}
      actionsRef={props.actionsRef}
      // The dialog sheet mirrors RbDialog's scrim contract: presses on the
      // backdrop do not dismiss (base/dialog.tsx:76) — Escape and the
      // card's Cancel/primary buttons close it.
      disablePointerDismissal
      ariaLabel={props.ariaLabel}
      initialFocus={props.initialFocus}
      finalFocus={props.finalFocus}
      cardClassName={props.cardClassName}
      overlaySource={props.overlaySource}
    >
      {props.children}
    </RbDrawerSheet>
  );
}

export interface RbDrawerSheetProps {
  /** Controlled open — the caller keeps every open/close transition. */
  readonly open: boolean;
  /**
   * Base UI's Drawer change event, verbatim through the sheet. Each wrapper
   * adapts its own `onOpenChange` through `drawerOnOpenChange` (the reason
   * unions differ; see that helper).
   */
  readonly onOpenChange: NonNullable<DrawerRootProps["onOpenChange"]>;
  /** Fires when the open/close animations have fully completed. */
  readonly onOpenChangeComplete?: DrawerRootProps["onOpenChangeComplete"];
  /** Imperative `unmount`/`close` (externally driven exits). */
  readonly actionsRef?: DrawerRootProps["actionsRef"];
  /**
   * The trigger element, adopted via `Drawer.Trigger`'s `render` — the
   * element keeps its own className/id/handlers/children, the press toggles
   * the sheet. The picker phone arm passes the caller's chip here; the
   * dialog arms pass nothing (controlled open, no trigger).
   */
  readonly trigger?: ReactElement;
  /**
   * Whether presses outside the sheet dismiss it. The dialog sheet mirrors
   * `RbDialog`'s scrim contract (`true`); the glass and picker sheets leave
   * it off (scrim presses close).
   */
  readonly disablePointerDismissal?: boolean;
  /** The sheet's ARIA role (the picker cards pass their listbox/dialog role). */
  readonly role?: string;
  /** The sheet's accessible name. */
  readonly ariaLabel?: string;
  /** The element to focus on open — the search input, or `false` to keep focus put. */
  readonly initialFocus?: DrawerPopupProps["initialFocus"];
  /** The element to restore focus to on close. */
  readonly finalFocus?: DrawerPopupProps["finalFocus"];
  /**
   * The backdrop's FULL class list. Defaults to `.modal-backdrop`; the glass
   * arm passes `.modal-glass-backdrop …` (both share the fixed inset-0
   * modal-tier base, `app.css:4783-4792`).
   */
  readonly backdropClassName?: string;
  /** The card's class list beyond `.modal-card .rb-drawer-card`. */
  readonly cardClassName?: string;
  /** Inline style on the card. */
  readonly style?: CSSProperties;
  /** Registers this name on the `overlayKeyboard` registry while open. */
  readonly overlaySource?: string;
  /**
   * Overrides the registry window — the glass arm holds its claim through
   * the `[data-closed]` exit, same as `RbDialogGlassProps.overlayOpen`.
   */
  readonly overlayOpen?: boolean;
  /** The card frame's key handler (the picker's cursor keyboard model). */
  readonly onKeyDown?: (event: KeyboardEvent<HTMLDivElement>) => void;
  readonly children: ReactNode;
}

/**
 * `RbDrawerSheet` — the shared phone sheet body: the Drawer tree every
 * phone arm renders (the dialogs, the glass palette, every `PickerCard`
 * menu). Modal, swipe-down-to-dismiss, no snap points, riding the modal
 * z-tier through `.modal-backdrop`.
 */
export function RbDrawerSheet(props: RbDrawerSheetProps) {
  useOverlayKeyboardSource(props.overlaySource, props.overlayOpen ?? props.open);
  return (
    <Drawer.Root
      open={props.open}
      onOpenChange={props.onOpenChange}
      onOpenChangeComplete={props.onOpenChangeComplete}
      actionsRef={props.actionsRef}
      modal
      disablePointerDismissal={props.disablePointerDismissal}
      swipeDirection="down"
    >
      {props.trigger !== undefined && <Drawer.Trigger render={props.trigger} />}
      <Drawer.Portal>
        <Drawer.Backdrop className={props.backdropClassName ?? "modal-backdrop"} />
        <Drawer.Viewport>
          <Drawer.Popup
            className={`modal-card rb-drawer-card ${props.cardClassName ?? ""}`}
            style={props.style}
            role={props.role}
            aria-label={props.ariaLabel}
            initialFocus={props.initialFocus}
            finalFocus={props.finalFocus}
            onKeyDown={props.onKeyDown}
          >
            {props.children}
          </Drawer.Popup>
        </Drawer.Viewport>
      </Drawer.Portal>
    </Drawer.Root>
  );
}

/**
 * The Drawer-vs-Dialog (and Drawer-vs-Popover) change-event seam: Base UI's
 * components type `onOpenChange` with their own reason union, and the
 * Drawer's adds `swipe`/`close-watcher` on top of the shared core. Every
 * dismissal path the adopted surfaces use (escape, outside/close press,
 * swipe) lands in `onOpenChange(false)`, and every consumer of the shared
 * wrappers reads only the `open` flag (chat-menu, space-filter,
 * settings-appearance, add-space, the picker cards all ignore the details),
 * so the one narrowing cast per wrapper is safe — and keeps each wrapper's
 * public `onOpenChange` type byte-compatible with its desktop arm.
 */
export function drawerOnOpenChange<TDetails>(
  onOpenChange: (open: boolean, eventDetails: TDetails) => void,
): NonNullable<DrawerRootProps["onOpenChange"]> {
  return onOpenChange as NonNullable<DrawerRootProps["onOpenChange"]>;
}
