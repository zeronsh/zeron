/**
 * Pure terminal-panel logic, ported verbatim from the desktop's
 * `crates/ui/src/terminal/panel.rs` / `view.rs` so tab behavior matches by
 * construction. Everything here is DOM- and React-free; the dock component
 * and session controller consume it.
 */

/** Fixed tab width — drag-reorder math stays analytic (desktop TAB_WIDTH). */
export const TAB_WIDTH = 118;
/** Tab strip height (desktop TAB_BAR_HEIGHT). */
export const TAB_BAR_HEIGHT = 40;
/** Keyboard input coalescing window before a `WriteTerminal` flush (desktop COALESCE_MS). */
export const COALESCE_MS = 12;
/** Debounce for `ResizeTerminal` after viewport-driven size changes (desktop RESIZE_DEBOUNCE_MS). */
export const RESIZE_DEBOUNCE_MS = 80;
/** Panel height clamp: 160 px … 55 % of the viewport (desktop TERMINAL_MIN_HEIGHT / TERMINAL_MAX_VH). */
export const TERMINAL_MIN_HEIGHT = 160;
export const TERMINAL_MAX_VH = 0.55;
/** Default dock height (desktop ui-settings default). */
export const TERMINAL_DEFAULT_HEIGHT = 280;

/** Panel height clamp: 160 px … 55 % of the viewport (§1.10). */
export function clampTerminalHeight(height: number, viewportH: number): number {
  const max = Math.max(viewportH * TERMINAL_MAX_VH, TERMINAL_MIN_HEIGHT);
  if (Number.isFinite(height)) {
    return Math.min(Math.max(height, TERMINAL_MIN_HEIGHT), max);
  }
  return TERMINAL_MIN_HEIGHT;
}

/** Reconnect backoff: 500 ms doubling to an 8 s ceiling (desktop backoff_ms). */
export function backoffMs(attempt: number): number {
  return Math.min(500 * 2 ** Math.min(attempt, 4), 8_000);
}

/** Move a tab from `from` to `to` (indices into the same array). */
export function reorderTabs<T>(tabs: T[], from: number, to: number): void {
  if (from >= tabs.length || to >= tabs.length || from === to) {
    return;
  }
  const [tab] = tabs.splice(from, 1);
  tabs.splice(to, 0, tab!);
}

/** Where a drag hovering at `relX` inside the tab strip would land. */
export function dropIndex(relX: number, tabW: number, count: number): number {
  if (count === 0 || tabW <= 0) {
    return 0;
  }
  return Math.min(Math.max(Math.floor(relX / tabW), 0), count - 1);
}

/**
 * Sliding transform (in tab-width units) for tab `ix` while `from` is dragged
 * over `over`: tabs between the two shift one slot toward the vacated gap.
 */
export function slideOffset(ix: number, from: number, over: number): number {
  if (from < over && ix > from && ix <= over) {
    return -1;
  }
  if (over < from && ix >= over && ix < from) {
    return 1;
  }
  return 0;
}

/** Active index after a reorder commit. */
export function activeAfterReorder(active: number, from: number, to: number): number {
  if (active === from) {
    return to;
  }
  if (from < active && to >= active) {
    return active - 1;
  }
  if (from > active && to <= active) {
    return active + 1;
  }
  return active;
}

/** Active index after closing `closed` (given the new, shorter length). */
export function activeAfterClose(active: number, closed: number, lenAfter: number): number {
  const shifted = closed < active ? active - 1 : active;
  if (lenAfter === 0) {
    return 0;
  }
  return Math.min(shifted, lenAfter - 1);
}

/** The `[process exited N]` trailer, dimmed (desktop exit_message). */
export function exitMessage(code: number): string {
  return `\r\n\x1b[90m[process exited ${code}]\x1b[0m\r\n`;
}

/**
 * Wrap pasted text for the PTY (desktop `paste_bytes`, view.rs:309-319):
 * bracketed-paste aware, and strips the one control sequence a paste could
 * inject (the end-marker) before anything is sent.
 */
export function pasteBytes(text: string, bracketed: boolean): string {
  const sanitized = text.replaceAll("\x1b[201~", "");
  if (bracketed) {
    return `\x1b[200~${sanitized}\x1b[201~`;
  }
  return sanitized;
}

/** Tab title from the session's shell path ("/bin/zsh" → "zsh"). */
export function shellTitle(shell: string): string {
  const name = (shell.split(/[/\\]/).pop() ?? shell).trim();
  return name.length === 0 ? "terminal" : name;
}

/** Base64 of raw bytes — the `WriteTerminal` / `TerminalEvent.data` wire shape. */
export function encodeBase64(bytes: Uint8Array): string {
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/**
 * Decode a base64 `Data` frame. Lenient like the desktop (`decode_base64`):
 * standard alphabet with optional padding; undecodable input yields an empty
 * array (the frame is dropped, never fatal).
 */
export function decodeBase64(data: string): Uint8Array {
  try {
    const binary = atob(data);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes;
  } catch {
    return new Uint8Array(0);
  }
}
