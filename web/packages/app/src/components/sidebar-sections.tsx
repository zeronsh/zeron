import { useRef, useState } from "react";
import { parseScopedId } from "@zeron/engine-client";
import { Icon } from "@zeron/icons";
import type { SidebarSection } from "../state/ui-settings";
import { sidebarStore, useSidebar } from "../state/sidebar";
import { sidebarNotice } from "../state/notice";
import { setChatArchived } from "../lib/chat-actions";
import { preventNativeSidebarRowDrag } from "../lib/sidebar-drag-events";
import { SIDEBAR_DISCLOSURE_BODY_INSET } from "../lib/sidebar-pins";
import { sidebarRowHeight, type ChatRow } from "../lib/view";
import type { EngineSession } from "../state/engine-session";
import {
  SidebarDisclosureBody,
  SidebarDisclosureHeader,
  useSidebarDisclosure,
} from "./sidebar-disclosure";
import {
  Dialog,
  DialogCard,
  DialogField,
  DialogTitle,
  BtnGhost,
  BtnPrimary,
} from "./ui/Dialog";
import { PickerCard } from "./ui/PickerCard";
import { MenuRow } from "./ui/MenuRows";

/** `shell.rs::SIDEBAR_LIST_GAP` — the flex gap between sidebar rows. */
const SIDEBAR_LIST_GAP = 2;
/** `spaces.rs::SIDEBAR_DISCLOSURE_HEADER_HEIGHT` — the section header row. */
const SIDEBAR_DISCLOSURE_HEADER_HEIGHT = 28;
/** `spaces.rs::SIDEBAR_SECTION_GAP` — every disclosure section's top band. */
const SIDEBAR_SECTION_GAP = 12;

/**
 * The section's keyed height for the FLIP diff —
 * `render_custom_sidebar_section`'s return: the 12px band + the 28px header
 * + the body (inset + rows + gaps, or the 40px "Drop sessions here") while
 * open, 0 while collapsed. The disclosure inside re-derives the same body
 * height for its tween; both come from here so they cannot drift.
 */
export function customSectionKeyedHeight(
  section: SidebarSection,
  rows: readonly { readonly branch: unknown; readonly changeRequest: unknown }[],
  compact: boolean,
  showLabel: boolean,
): number {
  let body = SIDEBAR_DISCLOSURE_BODY_INSET + (rows.length === 0 ? 40 : 0);
  for (const row of rows) {
    body += sidebarRowHeight(compact, showLabel, row.branch !== null, row.changeRequest !== null);
  }
  body += SIDEBAR_LIST_GAP * Math.max(rows.length - 1, 0);
  return SIDEBAR_SECTION_GAP + SIDEBAR_DISCLOSURE_HEADER_HEIGHT + (section.collapsed ? 0 : body);
}

/**
 * Custom sidebar sections — the web peer of the desktop's
 * `render_custom_sidebar_section` (upstream 86249cf0). Sections render
 * between Pinned and the regular groups: a muted name header with a hover
 * menu (Edit / Archive all / Delete) and chevron, a tweened disclosure
 * body holding the claimed rows or "Drop sessions here", and the whole
 * section is a drop target for the sidebar's session-drag gesture
 * (`data-sidebar-section-id`).
 */

/** One section with the profile bucket it lives under. */
export interface SectionEntry {
  readonly profileKey: string;
  readonly section: SidebarSection;
}

/**
 * The create-section dialog singleton (`open_section_dialog(None, ..)` from
 * the view menu's Create Section row, driven by `sectionDialogOpen` in the
 * sidebar store so the menu and the dialog never share a component).
 */
export function CreateSectionDialog({ profileKey }: { readonly profileKey: string | null }) {
  const sidebar = useSidebar();
  if (!sidebar.sectionDialogOpen || profileKey === null) {
    return null;
  }
  return (
    <SectionDialog
      sectionId={null}
      onCancel={() => sidebarStore.closeSectionDialog()}
      onSubmit={(name) => {
        sidebarStore.createSection(profileKey, name);
        sidebarStore.closeSectionDialog();
      }}
    />
  );
}

/**
 * One custom section (`render_custom_sidebar_section`): the name header
 * with its hover menu, the tweened body, and the drag/drop surface. The
 * edit dialog is local to the section (one can be open at a time — its
 * menu opens it).
 */
export function CustomSection({
  entry,
  rows,
  renderRow,
  onRowPointerDown,
  draggingChatId,
  shouldSuppressClick,
  sessions,
}: {
  readonly entry: SectionEntry;
  readonly rows: readonly ChatRow[];
  readonly renderRow: (row: ChatRow) => React.ReactNode;
  readonly onRowPointerDown: (event: React.PointerEvent, chatId: string) => void;
  readonly draggingChatId: string | null;
  readonly shouldSuppressClick: () => boolean;
  /** The fleet sessions — Archive all resolves each chat's own engine. */
  readonly sessions: ReadonlyMap<string, EngineSession>;
}) {
  const sidebar = useSidebar();
  const [menuOpen, setMenuOpen] = useState(false);
  const [headerHover, setHeaderHover] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const collapsed = entry.section.collapsed;
  const bodyHeight =
    customSectionKeyedHeight(entry.section, rows, sidebar.compact, sidebar.showProjectLabel) -
    SIDEBAR_SECTION_GAP -
    SIDEBAR_DISCLOSURE_HEADER_HEIGHT;
  const { bodyRef, chevronRef, toggle } = useSidebarDisclosure(
    `custom:${entry.section.id}`,
    !collapsed,
    bodyHeight,
  );

  function archiveAll(): void {
    // `archive_sidebar_section`: one Mutate per non-archived member — every
    // request is preserved (the single-row mutation slot cannot loop);
    // failures count into one notice. Membership is PRESERVED so
    // unarchiving restores the section.
    let failed = 0;
    let settled = 0;
    const outstanding = rows.filter((row) => !row.chat.archived).length;
    const report = (): void => {
      if (settled === outstanding && failed > 0) {
        sidebarNotice.set(`Could not archive ${failed} sessions. Try again.`);
      }
    };
    if (outstanding === 0) {
      return;
    }
    for (const row of rows) {
      if (row.chat.archived) {
        continue;
      }
      const owning = owningSession(sessions, row.chat.id);
      if (owning === null) {
        failed += 1;
        settled += 1;
        report();
        continue;
      }
      void setChatArchived(owning.client, row.chat.id, true)
        .catch(() => {
          failed += 1;
        })
        .finally(() => {
          settled += 1;
          report();
        });
    }
  }

  return (
    <section
      className="sidebar-custom-section"
      data-sidebar-section-id={entry.section.id}
    >
      <div
        className="sidebar-custom-section-header"
        onMouseEnter={() => setHeaderHover(true)}
        onMouseLeave={() => setHeaderHover(false)}
      >
        <SidebarDisclosureHeader
          id={`section-header-${entry.section.id}`}
          label={entry.section.name}
          open={!collapsed}
          withRule={false}
          chevronRef={chevronRef}
          onToggle={() => {
            toggle();
            sidebarStore.setSectionCollapsed(entry.profileKey, entry.section.id, !collapsed);
          }}
        />
        {(headerHover || menuOpen) && (
          <SectionMenu
            sectionName={entry.section.name}
            open={menuOpen}
            onOpenChange={setMenuOpen}
            onEdit={() => {
              setEditOpen(true);
            }}
            onArchiveAll={() => {
              archiveAll();
            }}
            onDelete={() => {
              sidebarStore.deleteSection(entry.profileKey, entry.section.id);
            }}
          />
        )}
      </div>
      {editOpen && (
        <SectionDialog
          sectionId={entry.section.id}
          initialName={entry.section.name}
          onCancel={() => setEditOpen(false)}
          onSubmit={(name) => {
            sidebarStore.renameSection(entry.profileKey, entry.section.id, name);
            setEditOpen(false);
          }}
        />
      )}
      <SidebarDisclosureBody bodyRef={bodyRef}>
        <div className="sidebar-custom-section-rows">
          {rows.length === 0 ? (
            <div className="sidebar-section-empty">Drop sessions here</div>
          ) : (
            rows.map((row) => (
              <div
                key={row.chat.id}
                className="regular-row"
                data-sidebar-dragging={draggingChatId === row.chat.id ? "1" : undefined}
                onPointerDown={(event) => onRowPointerDown(event, row.chat.id)}
                // The row's anchor is natively draggable; a press that moves
                // must stay OUR transfer gesture (sidebar-drag-events.ts).
                onDragStart={preventNativeSidebarRowDrag}
                onClickCapture={(event) => {
                  if (shouldSuppressClick()) {
                    event.preventDefault();
                    event.stopPropagation();
                  }
                }}
              >
                {renderRow(row)}
              </div>
            ))
          )}
        </div>
      </SidebarDisclosureBody>
    </section>
  );
}

/**
 * `render_section_overlays`' context menu: Edit / Archive all / Delete —
 * the trigger is the header's kebab (its hover reveal keeps `menuOpen` in
 * the header condition so the trigger stays mounted while the card is
 * open). Ticket 18: the card rides `PickerCard`'s body portal
 * (`anchorBelowEnd`, the card under the header's menu mark) — the old
 * inline absolute `.section-context-menu` painted inside
 * `<aside class="sidebar">` whose `overflow: hidden` clipped it, the same
 * bug class as the user menu (ticket 02); outside presses and Escape now
 * dismiss through Base UI's pipeline instead of the window listeners, and
 * the rows are the shared `MenuRow` recipe. At ≤768px the choices open as
 * the shared bottom sheet (`PickerCard`'s phone arm). Exported for the
 * mounted portal test (tests/section-menu.test.ts).
 */
export function SectionMenu({
  sectionName,
  open,
  onOpenChange,
  onEdit,
  onArchiveAll,
  onDelete,
}: {
  readonly sectionName: string;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onEdit: () => void;
  readonly onArchiveAll: () => void;
  readonly onDelete: () => void;
}) {
  const pick = (action: () => void): void => {
    onOpenChange(false);
    action();
  };
  return (
    <PickerCard
      open={open}
      onOpenChange={onOpenChange}
      placement="anchorBelowEnd"
      cardClassName="popover-card section-menu-body"
      role="menu"
      ariaLabel={`Section menu: ${sectionName}`}
      initialFocus={false}
      trigger={
        <button
          type="button"
          className="sidebar-section-menu-button"
          aria-label={`Section menu: ${sectionName}`}
          aria-haspopup="menu"
          // The press must not reach the header's drag/click surfaces; the
          // toggle itself is Base UI's `trigger-press` on the adopted element.
          onClick={(event) => event.stopPropagation()}
        >
          <Icon name="moreHorizontal" size={14} />
        </button>
      }
    >
      <MenuRow fadeKey="section-edit" onClick={() => pick(onEdit)}>
        Edit section
      </MenuRow>
      <MenuRow fadeKey="section-archive-all" onClick={() => pick(onArchiveAll)}>
        Archive all
      </MenuRow>
      <MenuRow fadeKey="section-delete" onClick={() => pick(onDelete)}>
        Delete
      </MenuRow>
    </PickerCard>
  );
}

/** `open_section_dialog` / `submit_section_dialog` — create or rename. */
export function SectionDialog({
  sectionId,
  initialName = "",
  onCancel,
  onSubmit,
}: {
  readonly sectionId: string | null;
  readonly initialName?: string;
  readonly onSubmit: (name: string) => void;
  readonly onCancel: () => void;
}) {
  const [value, setValue] = useState(initialName);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const valid = value.trim().length > 0 && [...value.trim()].length <= 120;
  return (
    <Dialog
      ariaLabel={sectionId === null ? "New section" : "Edit section"}
      onClose={onCancel}
      initialFocus={inputRef}
    >
      <DialogCard>
        <DialogTitle>{sectionId === null ? "New section" : "Edit section"}</DialogTitle>
        <p className="dialog-body">Group sessions however you like</p>
        <form
          className="dialog-form-rows"
          onSubmit={(event) => {
            event.preventDefault();
            if (valid) {
              onSubmit(value.trim());
            }
          }}
        >
          <DialogField>
            <input
              ref={inputRef}
              type="text"
              value={value}
              onChange={(event) => setValue(event.target.value)}
              placeholder="Section name"
              spellCheck={false}
              aria-label="Section name"
            />
          </DialogField>
          <div className="dialog-actions-row">
            <BtnGhost type="button" onClick={onCancel}>
              Cancel
            </BtnGhost>
            <BtnPrimary type="submit" className={valid ? undefined : "btn-primary-disabled"}>
              {sectionId === null ? "Create section" : "Save"}
            </BtnPrimary>
          </div>
        </form>
      </DialogCard>
    </Dialog>
  );
}

/** The engine session that owns a (scoped) chat id, fleet-resolved. */
function owningSession(
  sessions: ReadonlyMap<string, EngineSession>,
  chatId: string,
): EngineSession | null {
  try {
    const engine = parseScopedId(chatId).engine;
    return engine === null ? null : sessions.get(engine) ?? null;
  } catch {
    return null;
  }
}
