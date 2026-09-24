/**
 * The menu/picker search + navigation reducers — line-for-line ports of the
 * desktop's `popover.rs` pure helpers: `match_rank` + `filter_indices`
 * (`popover.rs:235-260`), `menu_step` (`:214-230`), and `classify_key`
 * (`:276-291`). Every picker and menu on the web builds on these.
 */

/**
 * Match rank of a label against a query: `0` prefix match, `1` substring
 * match, `null` no match. Case-insensitive; the query is trimmed, so an
 * empty or all-whitespace query matches everything at rank `1` (input order
 * preserved). All substring hits share rank `1` — ties break by input order
 * downstream, never by match position (`popover.rs:235-248`).
 */
export function matchRank(query: string, label: string): number | null {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) {
    return 1;
  }
  const haystack = label.toLowerCase();
  if (haystack.startsWith(needle)) {
    return 0;
  }
  return haystack.includes(needle) ? 1 : null;
}

/** Filter + rank labels for a query: prefix matches first, then substring
 * matches, stable within each rank (`(rank, inputIndex)` sort). Returns
 * indices into `labels` (`popover.rs:252-260`). */
export function filterIndices(query: string, labels: readonly string[]): number[] {
  const ranked: { rank: number; index: number }[] = [];
  for (let ix = 0; ix < labels.length; ix += 1) {
    const rank = matchRank(query, labels[ix] ?? "");
    if (rank !== null) {
      ranked.push({ rank, index: ix });
    }
  }
  ranked.sort((a, b) => a.rank - b.rank || a.index - b.index);
  return ranked.map((match) => match.index);
}

export interface RankedMatch {
  /** Index into the source array. */
  readonly index: number;
  /** 0 = prefix; 1 = substring. */
  readonly rank: number;
}

/** Keep only the matching items, in ranked order. Stable on ties. */
export function filterAndSort<T>(
  items: readonly T[],
  label: (item: T) => string,
  query: string,
): readonly T[] {
  const matches: { item: T; index: number; rank: number }[] = [];
  for (let ix = 0; ix < items.length; ix += 1) {
    const item = items[ix];
    if (item === undefined) {
      continue;
    }
    const rank = matchRank(query, label(item));
    if (rank !== null) {
      matches.push({ item, index: ix, rank });
    }
  }
  matches.sort((a, b) => a.rank - b.rank || a.index - b.index);
  return matches.map((match) => match.item);
}

/**
 * Step the active row of a menu: wraps at both ends; `null` enters at the
 * edge matching the direction (down enters at the top, up at the bottom).
 * Empty menus stay `null` (`popover.rs:214-230`).
 */
export function menuStep(active: number | null, count: number, delta: number): number | null {
  if (count === 0) {
    return null;
  }
  if (active === null) {
    return delta >= 0 ? 0 : count - 1;
  }
  return (((active + delta) % count) + count) % count;
}

/** Keys the pickers care about, classified from a raw keystroke (`popover.rs:263-291`). */
export type MenuKey = "up" | "down" | "enter" | "mod-enter" | "escape" | "backspace" | "other";

/**
 * Classify a keystroke. Accepts both DOM `KeyboardEvent.key` spellings
 * (`"ArrowUp"`, `"Enter"`) and the desktop's lowercase gpui names (`"up"`,
 * `"enter"`). Ctrl+N/Ctrl+P mirror ↓/↑ (readline/emacs motion); the modifier
 * must be **ctrl**, not cmd/meta. Enter with cmd or ctrl is `mod-enter`.
 */
export function classifyKey(key: string, cmd: boolean, ctrl: boolean): MenuKey {
  switch (key) {
    case "ArrowUp":
    case "up":
      return "up";
    case "ArrowDown":
    case "down":
      return "down";
    case "n":
      return ctrl ? "down" : "other";
    case "p":
      return ctrl ? "up" : "other";
    case "Enter":
    case "enter":
      return cmd || ctrl ? "mod-enter" : "enter";
    case "Escape":
    case "escape":
      return "escape";
    case "Backspace":
    case "backspace":
      return "backspace";
    default:
      return "other";
  }
}
