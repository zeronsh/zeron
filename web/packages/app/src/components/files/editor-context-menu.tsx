import { useRef, useState, type ReactElement, type RefObject } from "react";
import {
  RbContextMenu,
  RbContextMenuPopup,
  RbContextMenuPortal,
  RbContextMenuPositioner,
  RbContextMenuTrigger,
} from "../base/menu";
import { MenuRow, MenuSeparator } from "../ui/MenuRows";

/**
 * `EditorContextMenu` — the editable buffer's right-click menu
 * (mod.rs:626-687 `render_editor_context_menu`, availability from
 * editor.rs:16-33 `EditorMenuAvailability`): Cut / Copy / Paste, a
 * separator, Select All, in a 170px `menu_at` card. The ContextMenu
 * trigger adopts the editor's element via `render`; the library owns
 * right-click open, pointer positioning, Escape/outside dismissal, and the
 * exit window.
 *
 * Web note: `paste` availability cannot be probed synchronously (reading
 * the clipboard would prompt), so it tracks the editable state and the
 * click attempts the read, no-oping on denial — the one honest divergence
 * from the desktop's `clipboard_has_text` probe.
 */

/** `popover_card(theme).w(px(170.0))` (mod.rs:641). */
const EDITOR_MENU_WIDTH = 170;

export function EditorContextMenu({
  textareaRef,
  editable,
  children,
}: {
  /** The code view's input layer (cut/copy/paste/select-all act on it). */
  readonly textareaRef: RefObject<HTMLTextAreaElement | null>;
  readonly editable: boolean;
  /** The element the right-click opens on (the editor body). */
  readonly children: ReactElement;
}) {
  const [open, setOpen] = useState(false);
  const [availability, setAvailability] = useState({ cut: false, copy: false, paste: false });

  const readAvailability = (): void => {
    const element = textareaRef.current;
    if (element === null) {
      setAvailability({ cut: false, copy: false, paste: false });
      return;
    }
    const hasSelection = element.selectionEnd > element.selectionStart;
    setAvailability({
      cut: editable && hasSelection,
      copy: hasSelection,
      paste: editable,
    });
  };

  const run = (action: "cut" | "copy" | "paste" | "selectAll"): void => {
    const element = textareaRef.current;
    if (element === null) {
      return;
    }
    element.focus();
    if (action === "selectAll") {
      element.select();
      return;
    }
    if (action === "cut" || action === "copy") {
      // execCommand on a focused textarea is still the one API that honors
      // the current selection without re-implementing clipboard writes.
      document.execCommand(action);
      readAvailability();
      return;
    }
    void navigator.clipboard
      .readText()
      .then((text) => {
        element.setRangeText(text, element.selectionStart, element.selectionEnd, "end");
        element.dispatchEvent(new Event("input", { bubbles: true }));
      })
      .catch(() => {
        // Permission denied — the row's action quietly no-ops.
      });
  };

  return (
    <RbContextMenu
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) {
          readAvailability();
        }
      }}
    >
      <RbContextMenuTrigger
        render={children}
        onContextMenu={() => {
          // Availability reads the selection at open time (editor.rs:23-30).
          readAvailability();
        }}
      />
      <RbContextMenuPortal>
        <RbContextMenuPositioner>
          <RbContextMenuPopup
            className="rb-popover-popup popover-card files-editor-menu"
            role="menu"
            aria-label="Editor actions"
            style={{ width: EDITOR_MENU_WIDTH }}
          >
            <MenuRow fadeKey="files-editor-context-cut" disabled={!availability.cut} onClick={() => run("cut")}>
              Cut
            </MenuRow>
            <MenuRow fadeKey="files-editor-context-copy" disabled={!availability.copy} onClick={() => run("copy")}>
              Copy
            </MenuRow>
            <MenuRow fadeKey="files-editor-context-paste" disabled={!availability.paste} onClick={() => run("paste")}>
              Paste
            </MenuRow>
            <MenuSeparator />
            <MenuRow fadeKey="files-editor-context-select-all" onClick={() => run("selectAll")}>
              Select All
            </MenuRow>
          </RbContextMenuPopup>
        </RbContextMenuPositioner>
      </RbContextMenuPortal>
    </RbContextMenu>
  );
}
