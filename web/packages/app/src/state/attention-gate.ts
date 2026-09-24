import { AttentionSoundGate } from "../lib/notifications";

/**
 * The ONE app-global attention gate — the web peer of the desktop shell's
 * single `attention_sound_gate` field (shell.rs:1147, constructed once at
 * :1482, consulted by BOTH the session-status loop :1725 and the
 * connectivity loop :1751). Every `SessionNotificationDriver`
 * (state/session-provider.tsx — one per paired engine) consults this one
 * instance, so attention events arriving on DIFFERENT engines in one
 * burst coalesce into one chime, exactly like a session error and a
 * connectivity drop on the same engine do (250ms window).
 *
 * Module-scoped like `echoStore` so it outlives every driver remount
 * (engine switch, re-pair, runtime replacement): one gate per app
 * instance — a page load — never one per driver.
 */
export const appAttentionGate = new AttentionSoundGate();

/** Test seam — forget the last chime, a fresh app instance. */
export function resetAppAttentionGate(): void {
  appAttentionGate.reset();
}
