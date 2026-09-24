import { useSyncExternalStore } from "react";
import { addSpaceStore } from "./add-space";
import type { EngineSession } from "./engine-session";

/**
 * The command palette's state machine — the web port of the state half of
 * `crates/ui/src/shell/command_palette.rs` (`CommandPalette`,
 * `toggle_command_palette`, `close_command_palette`, `activate_command`):
 * the open flag with its exit window, the search query, and the keyboard
 * highlight. The entry derivations (actions + the global chat history)
 * live in `lib/command-palette.ts`; the mounted card
 * (`components/command-palette.tsx`) computes them from the live session
 * and resolves activation through the context attached here.
 *
 * The mount lifecycle rides `RbDialogGlass` exactly like the add-space
 * palette: `open` is the dialog's open flag, the exit window
 * (open=false while the layer still paints its `[data-closed]` fade) ends
 * when the component reports `onOpenChangeComplete(false)` (`unmounted()`).
 *
 * `commandPaletteStore.open()` is the hook other tickets call: the fixed
 * `Mod+K` binding (the desktop's `ToggleCommandPalette`) toggles through
 * `toggleCommandPalette`.
 */

/** The palette's mount phases, as the component and CSS read them. */
export type CommandPaletteStatus = "closed" | "open" | "closing";

export interface CommandPaletteSnapshot {
  readonly status: CommandPaletteStatus;
  readonly query: string;
  /** Keyboard highlight within the filtered entries (actions + chats). */
  readonly active: number;
}

/** What the mounted palette component supplies each session. */
export interface CommandPaletteContext {
  readonly session: EngineSession | null;
  /** New chat — the desktop's `open_new_session` (route to the canvas). */
  readonly goToCanvas: () => void;
  /** Open a chat row — `open_chat` (the id is engine-scoped already). */
  readonly openChat: (chatId: string) => void;
  /** Open settings — `open_settings` (the desktop lands on Devices). */
  readonly openSettings: () => void;
}

export class CommandPaletteStore {
  /** The dialog's open flag — `false` through the exit window. */
  #open = false;
  /** False only once the exit has drained (the old "closed" state). */
  #mounted = false;
  #query = "";
  #active = 0;
  #context: CommandPaletteContext | null = null;
  #snapshot: CommandPaletteSnapshot = { status: "closed", query: "", active: 0 };
  readonly #listeners = new Set<() => void>();

  getSnapshot(): CommandPaletteSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * `toggle_command_palette`: mint a fresh query and highlight. Opening
   * replaces the add-space palette (the desktop's `self.add_space = None`)
   * — ⌘K summons the command palette, never two cards.
   */
  open(): void {
    addSpaceStore.close();
    this.#query = "";
    this.#active = 0;
    this.#open = true;
    this.#mounted = true;
    this.#commit();
  }

  /** Every close path funnels here: the exit window, then the drain. */
  close(): void {
    if (this.#open) {
      this.#open = false;
      this.#commit();
    }
  }

  /**
   * The exit drained (`RbDialogGlass`'s `onOpenChangeComplete(false)`): the
   * layer is gone, so the query and highlight go with it.
   */
  unmounted(): void {
    if (!this.#mounted) {
      return;
    }
    this.#open = false;
    this.#mounted = false;
    this.#query = "";
    this.#active = 0;
    this.#commit();
  }

  /**
   * Hard close for a host that is unmounting: nothing is left to paint, so
   * no exit window either.
   */
  forceClose(): void {
    this.close();
    this.unmounted();
  }

  /** The component's session binding — re-called on engine switches. */
  attach(context: CommandPaletteContext): void {
    this.#context = context;
  }

  /**
   * A search edit (`ComposerInputEvent::Edited`): reset the highlight to
   * the first row and scroll the list back to the top (the component's
   * scroll-reset effect reads the query).
   */
  setQuery(text: string): void {
    if (!this.#open || this.#query === text) {
      return;
    }
    this.#query = text;
    this.#active = 0;
    this.#commit();
  }

  /**
   * The keyboard highlight step (up/down in the card's key handler): wraps
   * at both ends over the current entry count.
   */
  move(delta: number, count: number): void {
    if (!this.#open || count === 0) {
      return;
    }
    const next = (((this.#active + delta) % count) + count) % count;
    if (next !== this.#active) {
      this.#active = next;
      this.#commit();
    }
  }

  /**
   * `activate_command`: close first, then run the entry's intent — except
   * the theme action (upstream b4dd24d7), which keeps the palette open so
   * it updates to its next state.
   */
  activateEntry(entry: CommandEntryIntent): void {
    if (entry.kind !== "theme") {
      this.close();
    }
    switch (entry.kind) {
      case "new-chat":
        this.#context?.goToCanvas();
        return;
      case "new-project":
        // The desktop's `open_add_space` clears the palette first; the
        // add-space store's open() does the same on the web.
        addSpaceStore.open();
        return;
      case "settings":
        this.#context?.openSettings();
        return;
      case "chat":
        this.#context?.openChat(entry.chatId);
        return;
      case "theme":
        entry.setTheme();
        return;
    }
  }

  #commit(): void {
    this.#snapshot = {
      status: !this.#mounted ? "closed" : this.#open ? "open" : "closing",
      query: this.#query,
      active: this.#active,
    };
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

/** One activation intent, as the component resolves it from an entry. */
export type CommandEntryIntent =
  | { readonly kind: "new-chat" }
  | { readonly kind: "new-project" }
  | { readonly kind: "settings" }
  | { readonly kind: "chat"; readonly chatId: string }
  /** b4dd24d7's quick theme action — the palette stays open. */
  | { readonly kind: "theme"; readonly setTheme: () => void };

/**
 * The palette's module-level singleton — same shape as `addSpaceStore`.
 * The fixed `mod-k` binding's toggle subscribes through
 * `toggleCommandPalette`.
 */
export const commandPaletteStore = new CommandPaletteStore();

/**
 * `toggle_command_palette`: the palette mounted → close it; otherwise open
 * it. The fixed `Mod+K` binding dispatches here (the desktop's
 * `ToggleCommandPalette` action handler).
 */
export function toggleCommandPalette(): void {
  if (commandPaletteStore.getSnapshot().status !== "closed") {
    commandPaletteStore.close();
  } else {
    commandPaletteStore.open();
  }
}

const subscribe = (listener: () => void) => commandPaletteStore.subscribe(listener);
const getSnapshot = () => commandPaletteStore.getSnapshot();

/** The mount phases plus the query and highlight (closed reads as gone). */
export function useCommandPaletteSnapshot(): CommandPaletteSnapshot {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
