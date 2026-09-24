/**
 * The cursor keyboard model, composed once — what every cursor-walked
 * picker used to hand-roll (the device/project/checkout/ref cards, the
 * spaces menu, the identity card, the sidebar view menu). The pure
 * reducers live in `lib/picker-search.ts` (`menuStep`/`classifyKey`,
 * `popover.rs:214-291` — they also serve store-level consumers, so they
 * stay in `lib/`); this file wires them into React:
 *
 * - **The walk** (popover.rs:214-230): ↑/↓ wrap at both ends, the first
 *   press enters at the edge matching the direction. `mode: "anchored"`
 *   (default) keeps the cursor a row index — an empty walk lands on 0;
 *   `mode: "nullable"` is the desktop's `Option<usize>` cursor
 *   (spaces.rs:871-973 — the view menu starts at null and a click
 *   clears it).
 * - **Activation** (pickers.rs's `on_key_down`): Enter / Cmd+Enter fire
 *   `onActivate(index)` for a real row index only — the `row !==
 *   undefined` guard every card carried.
 * - **Escape is never here** — Base UI's dismiss pipeline owns it, which
 *   is what records the `escape-key` reason `RbPopover`'s finalFocus
 *   decision reads (blueprint §6.5). The cursor model stays a card-level
 *   concern, exactly where the base wrapper's header says it lives.
 * - **scrollIntoView** (`scroll_to_item`): with `listRef` +
 *   `rowAttribute`, the cursor row scrolls into view as the cursor moves
 *   (the effect keyed on `[cursor, count]`, as every list carried); the
 *   rows must tag `data-<rowAttribute>={ix}`.
 *
 * Rows render at the call site — every surface's row content differs
 * (icons, tags, virtualization), so the hook owns cursor state and the
 * key dispatch only. `PickerSearchField` is the search half of the
 * shape: `search_input_frame` (popover.rs:946-955) wrapping the
 * controlled input; typing fires `onQuery`, where the cursor-walked
 * surfaces reset their cursor (gap row 32).
 */

import { useEffect, useState, type Dispatch, type ReactNode, type Ref, type RefObject, type SetStateAction } from "react";
import { classifyKey, menuStep } from "../../lib/picker-search";

/** The event surface `cursorListKeyAction` reads — both DOM and React events satisfy it. */
export interface CursorKeyEvent {
  readonly key: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly preventDefault: () => void;
}

/** What the key dispatch decided: step the cursor, or activate a row. */
export type CursorListAction =
  | { readonly kind: "step"; readonly delta: number }
  | { readonly kind: "activate"; readonly index: number };

export interface CursorListKeyOptions {
  /** The walkable row count (trailing sentinel rows included). */
  readonly count: number;
  readonly cursor: number | null;
}

/**
 * The dispatch's pure core: ↑/↓ (and their Ctrl+N/P mirrors via
 * `classifyKey`) become a step; Enter/Cmd+Enter become an activation of
 * the cursor row when a row exists there; everything else — and a cursor
 * past the end or null — is a no-op. `null` return = not the model's key.
 */
export function cursorListKeyAction(event: CursorKeyEvent, options: CursorListKeyOptions): CursorListAction | null {
  const key = classifyKey(event.key, event.metaKey, event.ctrlKey);
  if (key === "down" || key === "up") {
    return { kind: "step", delta: key === "down" ? 1 : -1 };
  }
  if (key === "enter" || key === "mod-enter") {
    if (options.cursor !== null && options.cursor < options.count) {
      return { kind: "activate", index: options.cursor };
    }
  }
  return null;
}

/**
 * The walk's landing: `nullable` keeps `menuStep`'s null (the
 * `Option<usize>` cursor), `anchored` floors an empty walk at 0.
 */
export function cursorStepLanding(next: number | null, mode: "anchored" | "nullable"): number | null {
  return mode === "nullable" ? next : next ?? 0;
}

export interface CursorListOptions {
  /** The card is open — a closed card ignores keys. */
  readonly enabled: boolean;
  /**
   * The walkable row count, including any trailing rows the cursor may
   * land on (the project card's "Don't work in a project", §2.5).
   */
  readonly count: number;
  /** Enter/Mod+Enter on row `index`. */
  readonly onActivate: (index: number) => void;
  /**
   * `anchored` (default): the cursor is a row index; `nullable`: the
   * desktop's `Option<usize>` (spaces.rs:871-973's view menu).
   */
  readonly mode?: "anchored" | "nullable";
  /**
   * The cursor at mount (the card components mount per open); default 0
   * for `anchored`, null for `nullable`.
   */
  readonly initial?: number | null;
  /** Provide both to scroll the cursor row into view as the cursor moves. */
  readonly listRef?: RefObject<HTMLElement | null>;
  /** The rows must tag `data-<rowAttribute>={ix}` for the scroll effect to find them. */
  readonly rowAttribute?: string;
}

export interface CursorListState {
  readonly cursor: number | null;
  readonly setCursor: Dispatch<SetStateAction<number | null>>;
  /**
   * The key handler for the card's frame — a React `onKeyDown`, or the
   * native event a capture-phase listener hands it (the identity card's
   * window listener). Escape is never handled here.
   */
  readonly onKeyDown: (event: CursorKeyEvent) => void;
}

/** `useCursorList` — the cursor state + key dispatch every card shares. */
export function useCursorList(options: CursorListOptions): CursorListState {
  const mode = options.mode ?? "anchored";
  const [cursor, setCursor] = useState<number | null>(
    options.initial ?? (mode === "nullable" ? null : 0),
  );

  const onKeyDown = (event: CursorKeyEvent): void => {
    if (!options.enabled) {
      return;
    }
    const action = cursorListKeyAction(event, { count: options.count, cursor });
    if (action === null) {
      return;
    }
    event.preventDefault();
    if (action.kind === "step") {
      setCursor((current) => cursorStepLanding(menuStep(current, options.count, action.delta), mode));
      return;
    }
    options.onActivate(action.index);
  };

  useEffect(() => {
    if (options.listRef === undefined || options.rowAttribute === undefined || cursor === null) {
      return;
    }
    const row = options.listRef.current?.querySelector<HTMLElement>(`[data-${options.rowAttribute}="${cursor}"]`);
    row?.scrollIntoView({ block: "nearest" });
    // The count in the deps: a re-anchored, shorter list re-scrolls (the
    // cards keyed their effects on the row count).
  }, [cursor, options.count, options.listRef, options.rowAttribute]);

  return { cursor, setCursor, onKeyDown };
}

/**
 * `search_input_frame` (`popover.rs:946-955`) — the borderless recessed
 * frame at the top of a picker popover. Put the input inside; the frame
 * styles it (`.search-input-frame input`).
 */
export function SearchInputFrame(props: { children: ReactNode }) {
  return <div className="search-input-frame">{props.children}</div>;
}

export interface PickerSearchFieldProps {
  /** The card's search input ref — focused on open (spaces.rs:1207). */
  readonly inputRef?: Ref<HTMLInputElement>;
  readonly value: string;
  /**
   * Fires with the typed value; the cursor-walked surfaces reset their
   * cursor here (typing clears the highlight, gap row 32).
   */
  readonly onQuery: (value: string) => void;
  readonly placeholder: string;
  readonly ariaLabel: string;
}

/** The search field every search-driven picker tops its card with. */
export function PickerSearchField(props: PickerSearchFieldProps) {
  return (
    <SearchInputFrame>
      <input
        ref={props.inputRef}
        type="text"
        value={props.value}
        onChange={(event) => props.onQuery(event.target.value)}
        placeholder={props.placeholder}
        spellCheck={false}
        autoComplete="off"
        aria-label={props.ariaLabel}
      />
    </SearchInputFrame>
  );
}
