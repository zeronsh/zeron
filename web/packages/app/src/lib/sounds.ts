import { uiSettings } from "../state/ui-settings";

/**
 * Session notification chimes — the web port of `crates/ui/src/sound.rs`'s
 * delivery half. The decision engine lives in `lib/notifications.ts`; this
 * module owns the `Sound` vocabulary, the settings gate
 * (`settings.rs:1138 session_sound_enabled`), the query-flag kill-switch
 * (`sound.rs:19 ZERON_DISABLE_SOUND` → `?disableSound=1`), and playback
 * through a pooled `<audio>` element per kind (the browser replaces the
 * desktop's per-OS system players; failures are logged and swallowed — a
 * missing player must never bother the session flow).
 *
 * Autoplay policy: browsers refuse `Audio.play()` before the page has seen
 * a user gesture, so the pool stays muted until the first `pointerdown`/
 * `keydown` unlocks it (the web's "mute-until-interaction" pattern, the
 * closest equivalent of the desktop's always-available audio path). Chimes
 * that arrive before any interaction are dropped silently.
 */

/** Which notification chime to play (`sound.rs Sound`). */
export type Sound = "done" | "request" | "attention";

const SOUND_SRC: Record<Sound, string> = {
  done: "/sounds/done.wav",
  request: "/sounds/request.wav",
  attention: "/sounds/attention.wav",
};

/** The six-field sound half of the client settings store (ticket 03). */
export interface SessionSoundSettings {
  readonly soundEnabled: boolean;
  readonly soundCompletionEnabled: boolean;
  readonly soundInputEnabled: boolean;
  readonly soundAttentionEnabled: boolean;
}

/**
 * Whether this session event may produce audio (`settings.rs:1138`): the
 * master toggle AND the matching per-kind toggle. Pure.
 */
export function sessionSoundEnabled(settings: SessionSoundSettings, sound: Sound): boolean {
  if (!settings.soundEnabled) {
    return false;
  }
  switch (sound) {
    case "done":
      return settings.soundCompletionEnabled;
    case "request":
      return settings.soundInputEnabled;
    case "attention":
      return settings.soundAttentionEnabled;
  }
}

// ── Kill-switch (sound.rs:19 / notify.rs:27 env equivalents) ────────────────

/** Flag values that leave the feature enabled; anything else disables it. */
const FLAG_OFF = new Set(["", "0", "false"]);

/**
 * The `ZERON_DISABLE_SOUND` / `ZERON_DISABLE_NOTIFICATIONS` web
 * equivalents, from a `location.search` string: `?disableSound=1` and
 * `?disableNotifications=1` (present and not "0"/"false"). A query flag is
 * explicit — it must be typed into the URL — so it stays safe in
 * production builds. Pure.
 */
export function parseKillSwitch(search: string): { sound: boolean; notifications: boolean } {
  const params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const flag = (name: string): boolean => params.has(name) && !FLAG_OFF.has(params.get(name) ?? "");
  return { sound: flag("disableSound"), notifications: flag("disableNotifications") };
}

const KILL_SWITCH = parseKillSwitch(
  typeof location === "undefined" ? "" : location.search,
);

/** `ZERON_DISABLE_SOUND` equivalent — checked before any playback. */
export function soundDisabled(): boolean {
  return KILL_SWITCH.sound;
}

/** `ZERON_DISABLE_NOTIFICATIONS` equivalent — checked before any banner. */
export function notificationsDisabled(): boolean {
  return KILL_SWITCH.notifications;
}

// ── Playback ───────────────────────────────────────────────────────────────

/**
 * Autoplay unlock: one pooled element per sound, created lazily; the first
 * user gesture on the page unlocks the pool (module-level, installed once).
 */
let unlocked = false;

function installUnlockListeners(): void {
  if (typeof document === "undefined" || typeof document.addEventListener !== "function") {
    return;
  }
  const unlock = (): void => {
    unlocked = true;
  };
  document.addEventListener("pointerdown", unlock, { once: true, passive: true });
  document.addEventListener("keydown", unlock, { once: true, passive: true });
}

installUnlockListeners();

/** Whether a user gesture has unlocked audio playback on this page. */
export function audioUnlocked(): boolean {
  return unlocked;
}

const pool = new Map<Sound, HTMLAudioElement>();

function elementFor(sound: Sound): HTMLAudioElement | null {
  if (typeof Audio !== "function") {
    return null;
  }
  const existing = pool.get(sound);
  if (existing !== undefined) {
    return existing;
  }
  const element = new Audio(SOUND_SRC[sound]);
  element.preload = "auto";
  pool.set(sound, element);
  return element;
}

/**
 * Fire-and-forget chime (`sound.rs::play`): pooled `<audio>` element,
 * restart-from-zero, promise rejections and constructor failures logged and
 * swallowed — never awaited, never surfaced. Checks the kill-switch and
 * (defensively, mirroring the file-table contract) the settings toggles;
 * the call site applies the same gates before calling.
 */
export function playSound(sound: Sound): void {
  if (soundDisabled() || !sessionSoundEnabled(uiSettings.getSnapshot(), sound)) {
    return;
  }
  if (!audioUnlocked()) {
    // Autoplay policy: no user gesture yet — the chime would be refused.
    return;
  }
  const element = elementFor(sound);
  if (element === null) {
    return;
  }
  try {
    element.currentTime = 0;
    element.play()?.catch((error: unknown) => {
      console.debug("[sound] playback failed", error);
    });
  } catch (error) {
    console.debug("[sound] playback failed", error);
  }
}
