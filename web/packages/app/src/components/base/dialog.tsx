/**
 * `RbDialog` — the modal dialog on Base UI's Dialog, the migration target
 * for ticket 09's `Modal`/`ModalGlass` (`popover.rs:657-705`). The desktop's
 * `modal()` is a deferred, occluding, scrimmed, centered card playing
 * `dialog_in` (180ms) — Base UI's defaults (`modal: true`, portal, escape
 * handling, focus trapping) line up with all of it, so this wrapper mostly
 * wires our classes:
 *
 * - **Scrim:** `Dialog.Backdrop` wears `.modal-backdrop` —
 *   `scrim(--rb-scrim-alpha)` (0.6 dark / 0.32 light), token-based as
 *   ticket 02 settled. Backdrop and Popup render as portal SIBLINGS, so the
 *   card centers itself: `.modal-card.rb-dialog-card` is fixed at the
 *   viewport center (`top/left 50%` + `translate(-50%, -50%)`), clamped by
 *   `max-height` — the flex-overflow trap (a tall card clipping above the
 *   fold inside the old flex centering) is fixed structurally.
 * - **Plate:** `.modal-card` keeps radius 16; the 44px frost blur is gone
 *   (the web is always opaque, ticket 56) — the card content stays
 *   `DialogCard`/`DialogTitle`/… from `components/ui/Dialog.tsx`
 *   (pure styled divs, unchanged).
 * - **Motion:** `rb-dialog-in` (180ms, EASE) keys to `[data-open]` on
 *   `.rb-dialog-card`; the winning keyframe definition animates the
 *   `translate` property, which composes with (not clobbers) the centering
 *   `transform`. There is no exit animation: the desktop has no
 *   `dialog_out` in its motion catalog — modals unmount instantly — so no
 *   `[data-closed]` motion is declared and Base UI's animation-aware
 *   unmount drains immediately. `onOpenChangeComplete` and `actionsRef`
 *   are wired for future surfaces that DO add exit motion.
 * - **Scrim clicks do not dismiss:** `disablePointerDismissal: true` — the
 *   desktop's `.occlude()`d scrim swallows presses and the caller wires
 *   dismissal (popover.rs:693); same as the hand-rolled layer.
 * - **Escape:** Base UI's escape-key path routes to `onOpenChange(false)`.
 * - **Focus:** `initialFocus` (the `DialogField` input) and `finalFocus`
 *   pass through; default initial focus is the first tabbable element.
 * - **Keyboard scope:** dialogs do NOT register on `overlayKeyboard` — the
 *   desktop's `modal()` occludes without claiming the keyboard
 *   (`overlay_owns_keyboard` covers the palette and pickers only), so
 *   session-nav shortcuts stay live underneath. `overlaySource` opts a
 *   caller in if that ever changes.
 */

import type { CSSProperties, ReactNode } from "react";
import { Dialog, type DialogRootProps, type DialogPopupProps } from "@base-ui/react/dialog";
import { useOverlayKeyboardSource } from "./overlay";
import { drawerOnOpenChange, RbDrawerSheet } from "./responsive-surface";
import { useIsPhone } from "../../state/media";

export interface RbDialogProps {
  /** Controlled open. Close-on-unmount is the normal parity pattern (no exit motion). */
  readonly open: boolean;
  /** Base UI's change event verbatim (reason: `escape-key`, `close-press`, …). */
  readonly onOpenChange: NonNullable<DialogRootProps["onOpenChange"]>;
  /** Fires when the open/close animations have fully completed. */
  readonly onOpenChangeComplete?: DialogRootProps["onOpenChangeComplete"];
  /** Imperative `unmount`/`close` (externally driven exits). */
  readonly actionsRef?: DialogRootProps["actionsRef"];
  /** The dialog's accessible name (the old `Modal`'s `ariaLabel` slot). */
  readonly ariaLabel?: string;
  /** The element to focus on open — the `DialogField` input. */
  readonly initialFocus?: DialogPopupProps["initialFocus"];
  /** The element to restore focus to on close. */
  readonly finalFocus?: DialogPopupProps["finalFocus"];
  /** Registers this name on the `overlayKeyboard` registry while open. */
  readonly overlaySource?: string;
  /** Extra classes on the card (`.modal-card` is always applied). */
  readonly cardClassName?: string;
  readonly children: ReactNode;
}

/** `RbDialog` — the modal/scrim/centering contract on Base UI's Dialog. */
export function RbDialog(props: RbDialogProps) {
  useOverlayKeyboardSource(props.overlaySource, props.open);
  return (
    <Dialog.Root
      open={props.open}
      onOpenChange={props.onOpenChange}
      onOpenChangeComplete={props.onOpenChangeComplete}
      actionsRef={props.actionsRef}
      modal
      disablePointerDismissal
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="modal-backdrop" />
        <Dialog.Popup
          className={`modal-card rb-dialog-card ${props.cardClassName ?? ""}`}
          aria-label={props.ariaLabel}
          initialFocus={props.initialFocus}
          finalFocus={props.finalFocus}
        >
          {props.children}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export interface RbDialogGlassProps {
  /** Controlled open — the exit plays `[data-closed]` CSS (the palette fades). */
  readonly open: boolean;
  /**
   * Base UI's change event verbatim. A `false` from a scrim press or the
   * escape ladder routes here — the caller closes its store.
   */
  readonly onOpenChange: NonNullable<DialogRootProps["onOpenChange"]>;
  /** The exit drained — the palette's "fully closed" moment (flow dropped). */
  readonly onOpenChangeComplete?: DialogRootProps["onOpenChangeComplete"];
  /** The dialog's accessible name. */
  readonly ariaLabel?: string;
  /** Registers this name on the `overlayKeyboard` registry while claimed. */
  readonly overlaySource?: string;
  /**
   * Overrides the registry window: the palette holds its claim through the
   * exit (`status !== "closed"`) — the scrim is still up while the card
   * fades, and a jump firing under a visible modal would strand it over a
   * chat the user never picked (ticket 11's comment).
   */
  readonly overlayOpen?: boolean;
  /** Extra classes on the backdrop (`.modal-glass-backdrop` is always applied). */
  readonly backdropClassName?: string;
  /** Extra classes on the card (`.modal-card .rb-dialog-card` are applied). */
  readonly cardClassName?: string;
  /** Inline style on the card (the palette's 14px radius rides CSS instead). */
  readonly style?: CSSProperties;
  readonly children: ReactNode;
}

/**
 * `RbDialogGlass` — the `modal_glass` variant (popover.rs:684-705): the
 * lighter 0.35 scrim, and scrim presses DO dismiss (the add-space palette's
 * contract — "clicking the scrim dismisses, same as Escape"). Same card
 * self-centering, opaque plate, and `[data-open]` entrance as `RbDialog`; the
 * exit is the caller's `[data-closed]` CSS, which Base UI's animation-aware
 * unmount waits out — the palette's 100ms layer fade, replacing the old
 * layer's reap timer.
 *
 * At ≤768px (ticket 49) the glass renders through the shared phone sheet
 * (`RbDrawerSheet`): the 680px palette as a bottom sheet — the natural
 * mobile form; its own phone cap (`width: 100%; max-width: 680px`,
 * `app.css`'s `.add-space-card` block) applies inside the sheet unchanged.
 * The caller's backdrop/card classes ride along so the `[data-closed]`
 * exit CSS keeps working through the sheet exactly as through the
 * centered form. `disablePointerDismissal` is NOT passed here — the glass
 * contract's scrim press closes, same as at desktop.
 */
export function RbDialogGlass(props: RbDialogGlassProps) {
  // Registered here for the desktop arm; the phone sheet re-registers the
  // same name through its own seam (idempotent — the registry is a Set, so
  // both arms agree on the claim without fighting over it).
  useOverlayKeyboardSource(props.overlaySource, props.overlayOpen ?? props.open);
  const isPhone = useIsPhone();
  if (isPhone) {
    return (
      <RbDrawerSheet
        open={props.open}
        onOpenChange={drawerOnOpenChange(props.onOpenChange)}
        onOpenChangeComplete={props.onOpenChangeComplete}
        ariaLabel={props.ariaLabel}
        backdropClassName={`modal-glass-backdrop ${props.backdropClassName ?? ""}`}
        cardClassName={`rb-dialog-card ${props.cardClassName ?? ""}`}
        style={props.style}
        overlaySource={props.overlaySource}
        overlayOpen={props.overlayOpen}
      >
        {props.children}
      </RbDrawerSheet>
    );
  }
  return (
    <Dialog.Root
      open={props.open}
      onOpenChange={props.onOpenChange}
      onOpenChangeComplete={props.onOpenChangeComplete}
      modal
    >
      <Dialog.Portal>
        <Dialog.Backdrop className={`modal-glass-backdrop ${props.backdropClassName ?? ""}`} />
        <Dialog.Popup
          className={`modal-card rb-dialog-card ${props.cardClassName ?? ""}`}
          style={props.style}
          aria-label={props.ariaLabel}
        >
          {props.children}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
