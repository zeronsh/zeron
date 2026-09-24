import { encodeScopedId, type ChatStatus } from "@zeron/engine-client";
import type { ConnectivityState } from "@zeron/proto";
import { pendingSendStatus, type EchoStore } from "../state/transcript-store";
import { effectiveIndicator, SESSION_STALE_MS, type Indicator } from "./view";
import { notificationsDisabled, type Sound } from "./sounds";

/**
 * Session notification decision engine — the web port of the pure half of
 * `crates/ui/src/sound.rs` (the `SessionNotificationState` /
 * `ConnectivityNotificationState` / `AttentionSoundGate` machines) plus
 * `notify.rs`'s banner delivery through the Web `Notification` API. Ports
 * the Rust rule table and its unit tests 1:1, the `lib/view.ts` pattern:
 * the logic is pure and independently testable; the call site (the session
 * notification driver in `state/session-provider.tsx`, the peer of
 * `shell.rs:1677-1766`) turns decisions into chimes and banners.
 *
 * "Session" in the desktop's state-machine names means a CHAT's run state,
 * never the pairing credential (CONTEXT.md vocabulary) — user-facing
 * strings never say "session".
 */

// ---------------------------------------------------------------------------
// Decision engine (sound.rs:311-429)
// ---------------------------------------------------------------------------

/**
 * Notification baseline, separate from the visual activity indicator: going
 * idle can mean cancellation, expiry, or an internal handoff (`sound.rs`
 * `SessionNotificationState`).
 */
export interface SessionNotificationState {
  readonly indicator: Indicator;
  readonly lastCompletedTurn: string | null;
  readonly fresh: boolean;
}

/**
 * `SessionNotificationState::new` — the staleness-gated indicator, the
 * completion marker, and the freshness flag (`view::SESSION_STALE_MS`).
 */
export function sessionNotificationState(session: ChatStatus, now: number): SessionNotificationState {
  const updated = Date.parse(session.updatedAt);
  return {
    indicator: effectiveIndicator(session, now),
    lastCompletedTurn: session.lastCompletedTurn,
    fresh: Number.isFinite(updated) && now - updated <= SESSION_STALE_MS,
  };
}

/**
 * `SessionNotificationState::sound_since`: which chime (if any) the
 * transition from `prev` to `current` earns. Call after saving the new
 * baseline — suppressed pings must never be replayed later. Error first
 * (an error never masquerades as a completion), then input, then the
 * completion marker — only fresh, only un-masked by a pending send.
 */
export function soundSince(
  current: SessionNotificationState,
  prev: SessionNotificationState,
  sendPending: boolean,
): Sound | null {
  if (current.indicator === "errored" && prev.indicator !== "errored") {
    return "attention";
  }
  if (current.indicator === "awaitingInput" && prev.indicator !== "awaitingInput") {
    return "request";
  }
  if (
    !sendPending &&
    current.fresh &&
    current.lastCompletedTurn !== null &&
    current.lastCompletedTurn !== prev.lastCompletedTurn
  ) {
    return "done";
  }
  return null;
}

/**
 * The session notification driver's `send_pending` probe, namespace-corrected:
 * the echo overlay is keyed by the PAGE id — the scoped form the composer
 * publishes under (`pageChatId`) and the transcript route reads — while the
 * driver's status rows carry the engine's RAW chat ids. Scope the row's id to
 * its engine (the registry key, `engine.baseUrl`) before the lookup, exactly
 * as the registry's projection scopes its rows, or the probe never matches
 * and the app chimes and banners for its own sends.
 */
export function echoSendPending(
  echoes: EchoStore,
  engineKey: string,
  chatId: string,
  nowMs: number,
): boolean {
  return echoes
    .forChat(encodeScopedId(engineKey, chatId))
    .some((send) => pendingSendStatus(send, nowMs) === "pending");
}

/**
 * `connectivity_sound_since` — fires `Attention` exactly on the
 * not-degraded → degraded (Offline/Reconnecting) edge. The engine already
 * holds raw transport degradation for four seconds before exposing these,
 * so this is the durable edge.
 */
export function connectivitySoundSince(
  current: ConnectivityState,
  previous: ConnectivityState,
): Sound | null {
  const degraded = current === "offline" || current === "reconnecting";
  const wasDegraded = previous === "offline" || previous === "reconnecting";
  return degraded && !wasDegraded ? "attention" : null;
}

/** The boot grace before connectivity alerts arm (`STARTUP_QUIET`, 5s). */
export const STARTUP_QUIET_MS = 5_000;

/**
 * `ConnectivityNotificationState` — a newly attached engine can report
 * `connected` while its four-second degradation grace is still measuring an
 * outage that predates the UI. Arm alerts only after a longer healthy
 * observation, or after recovery from a boot-time outage; an unobserved
 * stream (engine switch, re-attach) resets to bootstrap semantics.
 */
export class ConnectivityNotificationState {
  #previous: ConnectivityState | null = null;
  #firstObservedAt: number | null = null;
  #armed = false;

  /** Feed one observation; the chime its transition earned, or null. */
  update(current: ConnectivityState, observed: boolean, now: number): Sound | null {
    if (!observed) {
      this.#previous = null;
      this.#firstObservedAt = null;
      this.#armed = false;
      return null;
    }
    if (this.#firstObservedAt === null) {
      this.#firstObservedAt = now;
    }
    const previous = this.#previous;
    this.#previous = current;
    if (!this.#armed) {
      if (now - this.#firstObservedAt >= STARTUP_QUIET_MS) {
        this.#armed = true;
        return previous === null ? null : connectivitySoundSince(current, previous);
      }
      return null;
    }
    return previous === null ? null : connectivitySoundSince(current, previous);
  }
}

/** The attention chime coalescing window (`COALESCE`, 250ms). */
export const COALESCE_MS = 250;

/**
 * `AttentionSoundGate` — coalesces attention requests delivered by
 * independent state watches (a session error and a connectivity drop in
 * one burst) into one chime. The ONE instance every watch consults lives
 * app-globally (`state/attention-gate.ts`), the peer of the desktop's
 * shell field (shell.rs:1147): one gate per app, never one per engine.
 */
export class AttentionSoundGate {
  #lastPlayed: number | null = null;

  shouldPlay(now: number): boolean {
    if (this.#lastPlayed !== null && now - this.#lastPlayed < COALESCE_MS) {
      return false;
    }
    this.#lastPlayed = now;
    return true;
  }

  /** Test seam — forget the last chime (a fresh app instance). */
  reset(): void {
    this.#lastPlayed = null;
  }
}

// ---------------------------------------------------------------------------
// Banner texts (notify.rs's payloads, shell.rs:1731-1763 — verbatim §2.6)
// ---------------------------------------------------------------------------

export interface BannerTexts {
  readonly title: string;
  readonly body: string;
}

/**
 * A chat event's banner: the chat's own title or the "New session"
 * fallback, and the per-kind body.
 */
export function chatBannerTexts(sound: Sound, chatTitle: string | null): BannerTexts {
  const body =
    sound === "done"
      ? "Run finished"
      : sound === "request"
        ? "Waiting on your input"
        : "Run failed";
  return { title: chatTitle ?? "New session", body };
}

/** A connectivity degradation's banner — no chat target. */
export function connectivityBannerTexts(state: ConnectivityState): BannerTexts {
  return {
    title: "Connection unavailable",
    body: state === "offline" ? "Your device is offline" : "Zeron is trying to reconnect",
  };
}

// ---------------------------------------------------------------------------
// Notification API wrapper (notify.rs — the delivery half)
// ---------------------------------------------------------------------------

type NotificationPermission = "default" | "granted" | "denied";

interface NotificationLike {
  close(): void;
  onclick: ((event: Event) => void) | null;
}

/**
 * The global `Notification`: a constructor whose static side carries
 * `permission` and `requestPermission` (the browser shape — there is no
 * separate namespace object).
 */
type NotificationApi = (new (title: string, options: { body?: string; data?: unknown }) => NotificationLike) & {
  permission: NotificationPermission;
  requestPermission(): Promise<NotificationPermission>;
};

function notificationGlobal(): NotificationApi | null {
  const candidate = (globalThis as { Notification?: unknown }).Notification;
  if (typeof candidate !== "function") {
    return null;
  }
  return candidate as unknown as NotificationApi;
}

/** Current browser permission, or null where the API is unavailable. */
export function notificationPermission(): NotificationPermission | null {
  return notificationGlobal()?.permission ?? null;
}

/**
 * §2.7 permission-request dedupe, pure: ask only while the answer is
 * genuinely undecided (`default`) and this page load has not asked yet.
 * Granted and denied are both settled — the browser never re-prompts for
 * them, and a second `requestPermission()` call would be noise.
 */
export function shouldRequestPermission(permission: NotificationPermission | null, askedThisSession: boolean): boolean {
  return permission === "default" && !askedThisSession;
}

let permissionAsked = false;

/**
 * The §2.7 flow's one entry point, for the Notifications settings toggle
 * (ticket 29) to call when `notificationsEnabled` turns on: exactly one
 * `Notification.requestPermission()` per page load, never on load, never
 * re-prompted after a settled answer. Resolves "default" where the API is
 * missing or the answer is already settled.
 */
export async function requestNotificationPermission(): Promise<NotificationPermission> {
  const api = notificationGlobal();
  if (api === null || !shouldRequestPermission(api.permission, permissionAsked)) {
    return api?.permission ?? "default";
  }
  permissionAsked = true;
  try {
    return await api.requestPermission();
  } catch {
    return notificationPermission() ?? "default";
  }
}/** Test seam: forget this session's permission request. */
export function resetPermissionRequestState(): void {
  permissionAsked = false;
}

/** The chat id a banner's `data` payload carries (notify.rs `CHAT_ID_KEY`). */
export const CHAT_ID_KEY = "chatId";

type ChatClickHandler = (chatId: string) => void;

let chatClickHandler: ChatClickHandler | null = null;

/**
 * Route banner clicks (`notify.rs::on_click`): `handler` receives a clicked
 * banner's chat id. Replaces any previous handler; returns its uninstall.
 * Connectivity banners carry no chat id and only focus the app.
 */
export function onChatNotificationClick(handler: ChatClickHandler): () => void {
  chatClickHandler = handler;
  return () => {
    if (chatClickHandler === handler) {
      chatClickHandler = null;
    }
  };
}

/**
 * Post a browser banner (`notify.rs::post`), optionally linked to a chat.
 * Silently a no-op when the kill-switch is set, the API is unavailable, or
 * permission was not granted (a preference is not a permission mirror —
 * chimes are independent). Best-effort: constructor failures are logged
 * and swallowed. A chat-tagged banner focuses the app and routes to the
 * chat on click; a connectivity banner only focuses the app.
 */
export function postBanner(title: string, body: string, chatId?: string): void {
  if (notificationsDisabled()) {
    return;
  }
  const api = notificationGlobal();
  if (api === null || api.permission !== "granted") {
    return;
  }
  let notification: NotificationLike;
  try {
    notification = new api(title, { body, data: chatId === undefined ? undefined : { [CHAT_ID_KEY]: chatId } });
  } catch (error) {
    console.debug("[notify] banner failed", error);
    return;
  }
  notification.onclick = (event) => {
    (event.target as NotificationLike | null)?.close?.();
    window.focus();
    if (chatId !== undefined) {
      chatClickHandler?.(chatId);
    }
  };
}
