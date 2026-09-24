/**
 * The chip family — the small trigger buttons that open picker cards
 * (`pickers.rs:2341-2422`). `FooterChip` is the footer's ghost dropdown
 * chip; `FooterLabel` its read-only committed sibling; `openChipClass`
 * the open-snap convention every trigger carries (`base/popover.tsx`'s
 * contract: style the open state off the controlled `open` — the class
 * drops during the exit window, a ≲100ms wash-fade on close).
 */

import { Icon, type IconName } from "@zeron/icons";

export interface FooterChipProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  readonly id: string;
  readonly icon: IconName;
  readonly label: string;
  readonly open: boolean;
  readonly offline?: boolean;
  readonly title: string;
}

/**
 * The small ghost dropdown chip: 20px tall, 6px radius, 12px medium; the wash
 * is quiet until hovered, open holding the hover fill (snapped, no fade).
 * The offline device chip overrides its text to `warning @ 0.8`.
 *
 * Rendered through `PickerCard`'s trigger adoption (Base UI's Trigger
 * merges its toggling/ARIA props onto this element) — so the extra props
 * spread onto the button.
 */
export function FooterChip(props: FooterChipProps) {
  const { id, icon, label, open, offline, title, className, ...rest } = props;
  return (
    <button
      type="button"
      id={id}
      className={`footer-menu-chip ${open ? "footer-menu-chip-open" : ""} ${
        offline === true ? "footer-menu-chip-offline" : ""
      } ${className ?? ""}`}
      title={title}
      {...rest}
    >
      <Icon name={icon} size={12} className="footer-menu-chip-icon" />
      <span className="footer-menu-chip-label">{label}</span>
      <Icon name="altArrowDown" size={12} className="footer-menu-chip-caret" />
    </button>
  );
}

/** `FooterLabel` — the read-only committed variant: no chevron, no background, no hover. */
export function FooterLabel({ icon, label }: { icon: IconName; label: string }) {
  return (
    <span className="footer-menu-label" title={label}>
      <Icon name={icon} size={12} className="footer-menu-label-icon" />
      <span className="footer-menu-label-text">{label}</span>
    </span>
  );
}

/**
 * The trigger open-snap class convention — `${base}` plus `${base}-open`
 * while the controlled `open` is true (the identity chip, the spaces
 * trigger, the sort button all follow it). The `-open` class drops with
 * `open`, during the exit window, by contract.
 */
export function openChipClass(base: string, open: boolean): string {
  return `${base} ${open ? `${base}-open` : ""}`;
}
