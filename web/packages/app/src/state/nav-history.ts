import { useSyncExternalStore } from "react";

/**
 * The titlebar's back/forward stack — a verbatim port of the desktop's
 * `NavHistory` (`crates/ui/src/shell.rs:548-613`).
 *
 * The browser's own history is NOT this: it records every router navigation,
 * including redirects and the ones the back button itself performs, it has no
 * notion of "the same route twice", and it cannot answer `canForward` at all
 * (which is why the titlebar hard-coded `canForward` to `true`). The desktop's
 * model is a plain cursor over a list of visited ROUTES:
 *
 *   - `push` dedups against the current entry and TRUNCATES the forward branch,
 *     so walking back and then opening a different chat drops the stale forward
 *     target instead of leaving a button that jumps somewhere the user never
 *     asked to go.
 *   - `replace` swaps the entry under the cursor without changing depth — the
 *     boot canvas uses it so the first chat the user picks does not leave a
 *     dead Back target pointing at an empty canvas they never visited.
 *   - `back`/`forward` only move the cursor; the caller turns the returned
 *     entry into a navigation. That navigation must NOT push, or every
 *     back-step would immediately be overwritten by a forward-looking push
 *     (`apply_nav`).
 */

export type NavEntry =
  /** `chatId === ""` is the new-chat canvas — the desktop's boot route. */
  | { readonly kind: "chat"; readonly chatId: string }
  | { readonly kind: "settings"; readonly section: string };

/** Entry equality is BY VALUE — `push`'s dedup compares routes, not objects. */
export function sameNavEntry(a: NavEntry, b: NavEntry): boolean {
  if (a.kind === "chat") {
    return b.kind === "chat" && a.chatId === b.chatId;
  }
  return b.kind === "settings" && a.section === b.section;
}

export class NavHistory {
  #entries: NavEntry[];
  #index = 0;

  constructor(initial: NavEntry) {
    this.#entries = [initial];
  }

  current(): NavEntry {
    return this.#entries[this.#index]!;
  }

  len(): number {
    return this.#entries.length;
  }

  canBack(): boolean {
    return this.#index > 0;
  }

  canForward(): boolean {
    return this.#index + 1 < this.#entries.length;
  }

  /** Visit a route: no-op on the current one, else truncate forward + append. */
  push(entry: NavEntry): void {
    if (sameNavEntry(entry, this.current())) {
      return;
    }
    this.#entries.length = this.#index + 1;
    this.#entries.push(entry);
    this.#index += 1;
  }

  /** Swap the entry under the cursor. Depth is untouched. */
  replace(entry: NavEntry): void {
    this.#entries[this.#index] = entry;
  }

  back(): NavEntry | null {
    if (!this.canBack()) {
      return null;
    }
    this.#index -= 1;
    return this.current();
  }

  /** Read-only entry access for history walks; never moves the cursor. */
  entryAt(ix: number): NavEntry {
    return this.#entries[ix]!;
  }

  /** The cursor's index into the entry list. */
  cursor(): number {
    return this.#index;
  }

  forward(): NavEntry | null {
    if (!this.canForward()) {
      return null;
    }
    this.#index += 1;
    return this.current();
  }
}

/**
 * The app's single `NavHistory`, plus the subscription the shell binds to.
 *
 * The shell re-renders on navigation anyway, but the enablement flags have to
 * be right on the frame the cursor moves — a `back()` that only changes the
 * cursor (never the URL, e.g. a deduped route) would otherwise leave the
 * buttons a frame stale. Keeping the mutations behind this thin store makes
 * every one of them a render trigger, the same as any other shell state.
 */
export interface NavSnapshot {
  readonly canBack: boolean;
  readonly canForward: boolean;
  readonly current: NavEntry;
}

/** The desktop's boot route: the untouched new-chat canvas. */
export const NAV_BOOT_ENTRY: NavEntry = { kind: "chat", chatId: "" };

export class NavHistoryStore {
  readonly #history: NavHistory;
  readonly #listeners = new Set<() => void>();
  #snapshot: NavSnapshot;

  constructor(initial: NavEntry = NAV_BOOT_ENTRY) {
    this.#history = new NavHistory(initial);
    this.#snapshot = this.#take();
  }

  getSnapshot(): NavSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  current(): NavEntry {
    return this.#history.current();
  }

  len(): number {
    return this.#history.len();
  }

  /**
   * The most recently visited chat entry, walking back from the CURSOR
   * without moving it — the web stand-in for the desktop's `active_chat`
   * (the chat `close_settings` returns to, shell.rs:3281-3286). The nav
   * stack records every chat the user actually opened, so the nearest chat
   * entry at or behind the cursor is the cheapest faithful source; the
   * forward branch past the cursor is history the user backed out of, not
   * "active".
   */
  nearestChat(): NavEntry | null {
    for (let ix = this.#history.cursor(); ix >= 0; ix--) {
      const entry = this.#history.entryAt(ix);
      if (entry.kind === "chat") {
        return entry;
      }
    }
    return null;
  }

  push(entry: NavEntry): void {
    this.#history.push(entry);
    this.#commit();
  }

  replace(entry: NavEntry): void {
    this.#history.replace(entry);
    this.#commit();
  }

  back(): NavEntry | null {
    const entry = this.#history.back();
    this.#commit();
    return entry;
  }

  forward(): NavEntry | null {
    const entry = this.#history.forward();
    this.#commit();
    return entry;
  }

  /**
   * A route the user navigated to themselves (a chat row, a settings item, a
   * link). The very first selection off the untouched boot canvas REPLACES
   * rather than pushes, so no dead Back target is left pointing at an empty
   * canvas (`shell.rs:1832-1838`).
   */
  visit(entry: NavEntry): void {
    const current = this.#history.current();
    const untouchedCanvas =
      this.#history.len() === 1 && current.kind === "chat" && current.chatId === "";
    if (untouchedCanvas && !sameNavEntry(entry, current)) {
      this.replace(entry);
      return;
    }
    this.push(entry);
  }

  #take(): NavSnapshot {
    return {
      canBack: this.#history.canBack(),
      canForward: this.#history.canForward(),
      current: this.#history.current(),
    };
  }

  #commit(): void {
    this.#snapshot = this.#take();
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

export const navHistory = new NavHistoryStore();

const subscribe = (listener: () => void): (() => void) => navHistory.subscribe(listener);
const getSnapshot = (): NavSnapshot => navHistory.getSnapshot();

export function useNavHistory(): NavSnapshot {
  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * The route a pathname names, or `null` for paths outside the nav model
 * (`/pair`, `/files`) — those neither push nor disturb the stack.
 *
 * `/chat/$id/changes` resolves to its owning chat: the changes view is a pane
 * surface on the desktop, not a route of its own.
 */
export function navEntryForPath(pathname: string): NavEntry | null {
  if (pathname === "/" || pathname === "") {
    return NAV_BOOT_ENTRY;
  }
  const settings = /^\/settings\/([^/]+)\/?$/.exec(pathname);
  if (settings !== null && settings[1] !== undefined) {
    return { kind: "settings", section: settings[1] };
  }
  const chat = /^\/chat\/([^/]+)(?:\/changes)?\/?$/.exec(pathname);
  if (chat !== null && chat[1] !== undefined) {
    return { kind: "chat", chatId: decodeSegment(chat[1]) };
  }
  return null;
}

/** The router path a `NavEntry` navigates to — the inverse of the matcher. */
export function navEntryPath(entry: NavEntry): string {
  if (entry.kind === "settings") {
    return `/settings/${entry.section}`;
  }
  return entry.chatId === "" ? "/" : `/chat/${encodeURIComponent(entry.chatId)}`;
}

function decodeSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}
