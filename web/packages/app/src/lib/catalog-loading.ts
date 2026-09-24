import type { LoadableList } from "../state/picker-catalog";

/**
 * The chip's loading classification — ports of the desktop's
 * `catalog_loading` / `models_loading` / `ensure_harnesses` reload table
 * (pickers.rs:4207-4221, 1037-1041).
 *
 * Loading is **Idle/Loading-only, never Error**: `LoadableList.loaded` is
 * only set by a successful fetch, so `!loaded` alone spans the Error slot
 * too — the eternal-spinner bug. These predicates treat an errored slot
 * as settled: the chip then renders its real label (remembered label,
 * else the configured/raw model id, `model_label` at pickers.rs:4186-4206)
 * and the failure surfaces through the card's ErrorRow instead.
 */

/** `catalog_loading` (pickers.rs:4207): the slot is Idle or Loading. */
export function catalogLoading(slot: LoadableList<unknown>): boolean {
  return !slot.loaded && slot.error === null;
}

/**
 * `models_loading` (pickers.rs:4208-4213): the harness's model slot is
 * neither Ready nor Error — absent (never requested), Idle, or Loading.
 * An errored slot reads as settled, exactly like a loaded one.
 */
export function modelsLoading(slot: LoadableList<unknown> | undefined): boolean {
  if (slot === undefined) {
    return true;
  }
  return !slot.loaded && slot.error === null;
}

/**
 * `shouldReload` — `ensure_harnesses`'s Idle/Loading/Ready|Error+force
 * table (pickers.rs:1037-1041), the discipline every non-forced re-kick
 * routes through (the composer's per-commit cadence, a window-focus
 * re-arm, the status heal). Non-forced loads fire from Idle only — an
 * Error that could re-trigger from the render loop would flip back to
 * Loading before the retry row ever painted; Loading never re-fires (the
 * in-flight guard stands behind it); Ready and Error reload only when
 * forced (stale-while-revalidate, the retry row, the card-open force).
 *
 * A failed forced revalidation keeps its stale rows (`loaded` stays set),
 * so it lands on the same force-only row as a Ready slot.
 */
export function shouldReload(slot: LoadableList<unknown>, force: boolean): boolean {
  if (slot.loading) {
    return false;
  }
  if (catalogLoading(slot)) {
    return true;
  }
  return force;
}

/**
 * The card-open force's re-fire key (ticket 61, hole 1). The open path
 * forces on every open, and — matching the desktop's per-render
 * `ensure_harnesses` cadence (pickers.rs:4164-4168: the client never
 * waits for an event to re-kick) — the force RE-FIRES while the card
 * stays open when the slot lands `Error`: a load that fails on an open
 * card swaps the body to the ErrorRow with nothing scheduled otherwise.
 * The key is the slot's error arm alone, so a warm Ready catalog never
 * re-fires (each landed reload would otherwise produce a fresh slot
 * object and loop); a re-arm that fails again leaves the key set — one
 * re-force per error landing, never a per-render storm.
 */
export function openForceRefire(slot: LoadableList<unknown>): boolean {
  return slot.error !== null;
}

/**
 * The in-flight guard's lifetime ceiling (ticket 61, hole 2): a harness
 * load in flight longer than this reads as lost and the slot becomes
 * re-kickable — the unary call timeout (30s, client.ts) stops gating the
 * re-trigger lattice. The bound follows the registry's identity-call cap
 * family (client.ts `IDENTITY_TIMEOUT_MS`: 10s).
 */
export const HARNESS_IN_FLIGHT_MS = 10_000;

/**
 * `inFlightLost` — is a harness flight that started at `inFlightSince`
 * still alive at `now`? A young flight owns the slot (single-flight: the
 * open force, the cadence, and the heal are all swallowed); one older
 * than `HARNESS_IN_FLIGHT_MS` is considered lost — the next kick
 * supersedes it, and its late landing is dropped by the flight token.
 */
export function inFlightLost(inFlightSince: number, now: number): boolean {
  return now - inFlightSince >= HARNESS_IN_FLIGHT_MS;
}
