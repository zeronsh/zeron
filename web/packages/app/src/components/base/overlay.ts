/**
 * The `overlayKeyboard` wiring for Base UI surfaces (ticket 12's
 * `overlay_owns_keyboard`, shell.rs:3681-3683). Session-nav shortcuts go
 * quiet under an overlay that owns the keyboard; the flag is OUR app state,
 * not the library's, so the wrappers register on the same registry the
 * hand-rolled layer did (`composer-pickers.tsx`/`add-space-palette.tsx`
 * pattern) — the quiet semantics survive the migration unchanged.
 */

import { useEffect, useRef } from "react";
import { overlayKeyboard } from "../../state/keymap";

/** One registry call: `[source, owns]`. */
export type OverlayKeyboardCall = readonly [source: string, owns: boolean];

/**
 * The registry calls an open transition must make: register on close→open,
 * unregister on open→close, nothing when the state did not change. Pure, so
 * the wiring is unit-testable without rendering.
 */
export function overlayKeyboardTransition(
  source: string,
  previousOpen: boolean,
  nextOpen: boolean,
): readonly OverlayKeyboardCall[] {
  if (previousOpen === nextOpen) {
    return [];
  }
  return [[source, nextOpen]];
}

/**
 * Registers `source` on the overlayKeyboard registry while `open` — and only
 * while open — including the unmount-while-open case. Pass `undefined` to
 * opt out (modal dialogs: the desktop's `modal()` occludes without claiming
 * the keyboard, so session-nav shortcuts stay live under them).
 */
export function useOverlayKeyboardSource(source: string | undefined, open: boolean): void {
  const previousOpenRef = useRef(false);
  useEffect(() => {
    if (source === undefined) {
      previousOpenRef.current = false;
      return;
    }
    const calls = overlayKeyboardTransition(source, previousOpenRef.current, open);
    for (const [name, owns] of calls) {
      overlayKeyboard.set(name, owns);
    }
    previousOpenRef.current = open;
    return () => {
      for (const [name, owns] of overlayKeyboardTransition(source, open, false)) {
        overlayKeyboard.set(name, owns);
      }
      previousOpenRef.current = false;
    };
  }, [source, open]);
}
