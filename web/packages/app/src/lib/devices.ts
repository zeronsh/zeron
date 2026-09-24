import type { IconName } from "@zeron/icons";
import { DEVICE_ONLINE_WINDOW_SECS } from "./view";

/**
 * Devices-page pure logic — the web peer of `crates/ui/src/settings/devices.rs`'s
 * helpers: the 70s presence window, the corner presence dot, the compact
 * last-seen wording, platform labels, and the click-to-copy short id. The
 * sidebar's `deviceOnline` (`lib/view.ts`, `state.rs::device_online`) is a
 * DIFFERENT function with different None semantics (unknown rows read
 * online there, offline here) — both exist on the desktop too; do not merge.
 * This one is named `lastSeenOnline` (the settings-page semantic: the raw
 * last-seen window over a device row) so the two same-named desktop
 * `device_online` functions never collide at a web call site again.
 */

export { DEVICE_ONLINE_WINDOW_SECS };

/**
 * The engine connection state behind a device row's presence dot
 * (`EngineConnectionState`, verbatim). `null` — the row is not backed by a
 * connection this client holds; the last-seen window decides.
 */
export type EngineConnection = "connected" | "reconnecting" | "off";

/** `PresenceDot` (devices.rs:35-43). */
export type PresenceDot = "connected" | "reconnecting" | "off";

/**
 * `device_online` (devices.rs:27-30), web-named `lastSeenOnline` for the
 * settings-page semantic so it never collides with `lib/view.ts`'s
 * engine-state-aware `deviceOnline` (`state.rs::device_online`): last-seen
 * within the window, future timestamps (clock skew) counting as online; a
 * null/absent last-seen reads offline. `lastSeenAt` is the wire's RFC 3339
 * string.
 */
export function lastSeenOnline(lastSeenAt: string | null | undefined, now: number): boolean {
  if (lastSeenAt === null || lastSeenAt === undefined) {
    return false;
  }
  const at = Date.parse(lastSeenAt);
  if (!Number.isFinite(at)) {
    return false;
  }
  return now - at <= DEVICE_ONLINE_WINDOW_SECS * 1000;
}

/**
 * `presence_dot` (devices.rs:45-52): engine-backed rows report the owning
 * engine's connection verbatim; rows with no engine key fall back to the
 * last-seen window.
 */
export function presenceDot(connection: EngineConnection | null, online: boolean): PresenceDot {
  if (connection !== null) {
    return connection;
  }
  return online ? "connected" : "off";
}

/**
 * `format_last_seen` (devices.rs:56-70) — the wording the Devices rows
 * use for a device's last heartbeat. `at`/`now` are epoch millis.
 */
export function formatLastSeen(at: number | null, now: number): string {
  if (at === null) {
    return "never seen";
  }
  const seconds = Math.floor((now - at) / 1000);
  if (seconds < 60) {
    return "just now";
  }
  if (seconds < 3600) {
    return `${Math.floor(seconds / 60)}m ago`;
  }
  if (seconds < 86_400) {
    return `${Math.floor(seconds / 3600)}h ago`;
  }
  return `${Math.floor(seconds / 86_400)}d ago`;
}

/** `format_last_seen` over the wire's RFC 3339 device timestamps. */
export function formatLastSeenAt(at: string | null | undefined, now: number): string {
  if (at === null || at === undefined) {
    return "never seen";
  }
  const parsed = Date.parse(at);
  return formatLastSeen(Number.isFinite(parsed) ? parsed : null, now);
}

/** `platform_label` (devices.rs:286-296): known platforms, else verbatim. */
export function platformLabel(platform: string): string {
  switch (platform) {
    case "macos":
    case "darwin":
      return "macOS";
    case "linux":
      return "Linux";
    case "windows":
      return "Windows";
    case "web":
      return "Web";
    case "ios":
      return "iOS";
    case "android":
      return "Android";
    default:
      return platform;
  }
}

/** `short_id` (devices.rs:299-305): `abcd1234…wxyz` when longer than 12. */
export function shortId(id: string): string {
  if (id.length > 12) {
    return `${id.slice(0, 8)}…${id.slice(id.length - 4)}`;
  }
  return id;
}

/**
 * The platform → tile glyph (devices.rs:348-353): LAPTOP for macos/darwin,
 * GLOBAL for web, SMARTPHONE for ios/android, MONITOR otherwise — the same
 * mapping `add-space-palette.ts` already carries for the same reason.
 */
export function platformGlyph(platform: string): IconName {
  switch (platform) {
    case "macos":
    case "darwin":
      return "laptop";
    case "web":
      return "global";
    case "ios":
    case "android":
      return "smartphone";
    default:
      return "monitor";
  }
}
