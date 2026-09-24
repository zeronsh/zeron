import { Fragment, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Icon, type IconName } from "@zeron/icons";

/**
 * The settings pages' shared widget vocabulary — the web ports of
 * `crates/ui/src/settings/widgets.rs` builders that tickets 28/29 compose
 * into rows: `meta_line`'s fragment joiner (`:269-293`), `row_tile`
 * (`:237-253`), and `compact_action` (settings/appearance.rs:749-767, the
 * trailing ghost-button recipe every theme-library/background action and
 * the import dialog's footer buttons reuse). The card/row/badge shells
 * stay plain `div`s with `.settings-*` classes at the page (the pages
 * already carry them); these are the three shapes that carry behavior or
 * repeat per-fragment structure.
 */

/** `meta_line` (widgets.rs:269-293): fragments joined by a dimmed `·`. */
export function MetaLine(props: { readonly fragments: readonly ReactNode[] }) {
  return (
    <span className="settings-meta-line">
      {props.fragments.map((fragment, ix) => (
        <Fragment key={ix}>
          {ix > 0 && <span className="settings-meta-dot" aria-hidden="true">·</span>}
          {fragment}
        </Fragment>
      ))}
    </span>
  );
}

/** `row_tile` (widgets.rs:237-253): the 36×10px identity tile around a 16px icon. */
export function RowTile(props: { readonly icon: IconName }) {
  return (
    <div className="row-tile" aria-hidden="true">
      <Icon name={props.icon} size={16} className="row-tile-icon" />
    </div>
  );
}

export interface CompactActionProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  readonly children: ReactNode;
}

/**
 * `compact_action` (appearance.rs:749-767): the 28px quiet ghost button.
 * Callers add their own `id`/`onClick`; the danger variant colors the label
 * (the library Remove / background Remove rows).
 */
export function CompactAction(props: CompactActionProps) {
  const { children, className, ...rest } = props;
  return (
    <button type="button" className={`compact-action ${className ?? ""}`} {...rest}>
      {children}
    </button>
  );
}

/** `compact_action(...).text_color(theme.danger)` — the destructive label. */
export function CompactActionDanger(props: CompactActionProps) {
  const { children, className, ...rest } = props;
  return (
    <button type="button" className={`compact-action compact-action-danger ${className ?? ""}`} {...rest}>
      {children}
    </button>
  );
}
