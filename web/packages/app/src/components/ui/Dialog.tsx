/**
 * The dialog family — `base/responsive-surface.tsx`'s `RbResponsiveDialog`
 * (`RbDialog`'s composite at ≥769px, the Drawer bottom sheet at ≤768px)
 * plus the card chrome the desktop's `popover.rs:657-1076` dialog builders
 * define (`dialog_card` `:978-990`, `dialog_title` `:993`, `dialog_body`
 * `:1002`, `dialog_field` `:1012-1023`, the `btn_*` row `:1028-1076`).
 *
 * `Dialog` wires the mount-while-open pattern every consumer repeats
 * (the rename/delete confirmations in the chat and space menus): `open`
 * is always true while mounted, Base UI's Escape / close-press routes
 * `onOpenChange(false)` to `onClose`, and the caller unmounts. There is
 * no exit motion — the desktop's modals unmount instantly
 * (`base/dialog.tsx`'s contract).
 */

import type { ButtonHTMLAttributes, ReactNode } from "react";
import { RbResponsiveDialog } from "../base/responsive-surface";
import type { RbDialogProps } from "../base/dialog";

export interface DialogProps {
  /** The dialog's accessible name. */
  readonly ariaLabel: string;
  /** Escape / close-press — the caller unmounts (no exit motion). */
  readonly onClose: () => void;
  /** The element to focus on open — the `DialogField` input. */
  readonly initialFocus?: RbDialogProps["initialFocus"];
  readonly children: ReactNode;
}

/** `Dialog` — the open-while-mounted modal shell (scrim, trap, Escape). */
export function Dialog(props: DialogProps) {
  return (
    <RbResponsiveDialog
      open
      onOpenChange={(next) => {
        if (!next) {
          props.onClose();
        }
      }}
      ariaLabel={props.ariaLabel}
      initialFocus={props.initialFocus}
    >
      {props.children}
    </RbResponsiveDialog>
  );
}

/** `dialog_card` (popover.rs:978-990) — the centered 360px card. */
export function DialogCard(props: { children?: ReactNode }) {
  return <section className="dialog-card">{props.children}</section>;
}

/** `dialog_title` (`popover.rs:993`) — 15px semibold. */
export function DialogTitle(props: { children: ReactNode }) {
  return <h2 className="dialog-card-title">{props.children}</h2>;
}

/** `dialog_body` (`popover.rs:1002`) — 13px/19px muted copy. */
export function DialogBody(props: { children: ReactNode }) {
  return <p className="dialog-card-body">{props.children}</p>;
}

/** `dialog_field` (`popover.rs:1012-1023`) — the text-field frame; put the
 * input inside (`.dialog-field input` styles it). */
export function DialogField(props: { children: ReactNode }) {
  return <div className="dialog-field">{props.children}</div>;
}

type DialogButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { children: ReactNode };

/** `btn_ghost` (`popover.rs:1028-1046`) — quiet text fading up on hover. */
export function BtnGhost(props: DialogButtonProps) {
  const { children, className, ...rest } = props;
  return (
    <button type="button" className={`dialog-btn-ghost ${className ?? ""}`} {...rest}>
      {children}
    </button>
  );
}

/** `btn_primary` (`popover.rs:1049-1061`) — the text-colored fill. */
export function BtnPrimary(props: DialogButtonProps) {
  const { children, className, ...rest } = props;
  return (
    <button type="button" className={`dialog-btn-primary ${className ?? ""}`} {...rest}>
      {children}
    </button>
  );
}

/** `btn_danger` (`popover.rs:1064-1076`) — the muted red fill. */
export function BtnDanger(props: DialogButtonProps) {
  const { children, className, ...rest } = props;
  return (
    <button type="button" className={`dialog-btn-danger ${className ?? ""}`} {...rest}>
      {children}
    </button>
  );
}
