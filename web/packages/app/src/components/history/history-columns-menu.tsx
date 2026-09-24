import { Icon } from "@zeron/icons";
import { RbPopover, virtualAnchorAt } from "../base/popover";
import { MenuRow, MenuSeparator } from "../ui/MenuRows";
import {
  DEFAULT_HISTORY_COLUMN_WIDTHS,
  HISTORY_COLUMN_LABELS,
  type HistoryColumn,
  type HistoryColumnVisibility,
  type HistoryColumnWidths,
} from "../../lib/git-history";

/**
 * The column-picker and author-display menus (§2.10/§2.11, history.rs's
 * `render_column_menu`/`render_author_menu`): 132px and 116px `popover_card`
 * popovers opened AT the click point (the columns button / the Author
 * header's right-click). Rows ride `MenuRow`; the row geometry overrides
 * the family default per the ticket (gap 0, 4/7 padding, radius 6, 11.5px).
 *
 * Every choice persists immediately through ticket 03's settings store; the
 * caller passes the write through (the pane owns the settings snapshot).
 */

export interface HistoryMenuPoint {
  readonly x: number;
  readonly y: number;
}

export interface ColumnMenuProps {
  readonly anchor: HistoryMenuPoint;
  readonly columns: HistoryColumnVisibility;
  readonly widths: HistoryColumnWidths;
  readonly order: readonly HistoryColumn[];
  readonly onToggleColumn: (column: HistoryColumn) => void;
  readonly onReset: () => void;
  readonly onClose: () => void;
}

/** `render_column_menu` — Author/Date/SHA checkboxes + conditional Reset. */
export function ColumnMenu(props: ColumnMenuProps) {
  const { anchor, columns, widths, order, onToggleColumn, onReset, onClose } = props;
  const differsFromDefaults =
    columns.author !== true ||
    columns.date !== true ||
    columns.sha !== true ||
    widths.author !== DEFAULT_HISTORY_COLUMN_WIDTHS.author ||
    widths.date !== DEFAULT_HISTORY_COLUMN_WIDTHS.date ||
    widths.sha !== DEFAULT_HISTORY_COLUMN_WIDTHS.sha ||
    order.join(",") !== "author,date,sha";
  const row = (column: HistoryColumn): boolean =>
    column === "author" ? columns.author : column === "date" ? columns.date : columns.sha;

  return (
    <RbPopover
      open
      onOpenChange={(next) => {
        if (!next) {
          onClose();
        }
      }}
      anchor={virtualAnchorAt(anchor.x, anchor.y)}
      placement="menuAt"
      cardClassName="popover-card history-menu"
      role="menu"
      ariaLabel="History columns"
      style={{ width: 132 }}
    >
      <div className="history-menu-inner">
        {(["author", "date", "sha"] as const).map((column) => (
          <MenuRow
            key={column}
            fadeKey={`history-column-${column}`}
            role="menuitemcheckbox"
            aria-checked={row(column)}
            onClick={() => onToggleColumn(column)}
          >
            <span className="history-menu-check" aria-hidden>
              {row(column) ? <Icon name="check" size={12} /> : null}
            </span>
            <span className="history-menu-label">{HISTORY_COLUMN_LABELS[column]}</span>
          </MenuRow>
        ))}
        {differsFromDefaults ? (
          <>
            <MenuSeparator />
            <MenuRow fadeKey="history-column-reset" className="history-menu-reset" onClick={onReset}>
              <span className="history-menu-check" aria-hidden />
              <span className="history-menu-label">Reset</span>
            </MenuRow>
          </>
        ) : null}
      </div>
    </RbPopover>
  );
}

export interface AuthorMenuProps {
  readonly anchor: HistoryMenuPoint;
  readonly display: "avatar" | "name";
  readonly onToggle: () => void;
  readonly onClose: () => void;
}

/**
 * `render_author_menu` — a two-state toggle presented as ONE togglable
 * "Name" row (Avatar is the unchecked default), not a radio pair.
 */
export function AuthorMenu({ anchor, display, onToggle, onClose }: AuthorMenuProps) {
  return (
    <RbPopover
      open
      onOpenChange={(next) => {
        if (!next) {
          onClose();
        }
      }}
      anchor={virtualAnchorAt(anchor.x, anchor.y)}
      placement="menuAt"
      cardClassName="popover-card history-menu"
      role="menu"
      ariaLabel="Author display"
      style={{ width: 116 }}
    >
      <div className="history-menu-inner">
        <MenuRow
          fadeKey="history-author-display-name"
          role="menuitemcheckbox"
          aria-checked={display === "name"}
          onClick={() => {
            onToggle();
            onClose();
          }}
        >
          <span className="history-menu-check" aria-hidden>
            {display === "name" ? <Icon name="check" size={12} /> : null}
          </span>
          <span className="history-menu-label">Name</span>
        </MenuRow>
      </div>
    </RbPopover>
  );
}
