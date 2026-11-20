// Which build a visitor should be offered, and where it lives. No DOM here:
// src/scripts/downloads.ts applies these to the page.

import { ANDROID_APK, DESKTOP_BUILDS, FALLBACK_VERSION, RELEASES_URL, type BuildId } from "../data/releases.ts";

/** The parts of `navigator` that say what a visitor is running. */
export interface Client {
  userAgent?: string;
  platform?: string;
  maxTouchPoints?: number;
  userAgentData?: { platform?: string; mobile?: boolean };
}

export type Target =
  | { kind: "desktop"; build: BuildId }
  | { kind: "android" }
  /** A phone or tablet with no direct download, or a system we can't name:
   *  the buttons keep pointing at the downloads table. */
  | { kind: "table" };

export function detectTarget(client: Client): Target {
  const ua = client.userAgent ?? "";
  const platform = client.userAgentData?.platform || client.platform || "";
  if (/Android/i.test(ua) || /Android/i.test(platform)) return { kind: "android" };
  // An iPad reports itself as a Mac with a touch screen.
  const mobile = /iPhone|iPad|iPod/i.test(ua) || client.userAgentData?.mobile === true || (/Mac/i.test(platform) && (client.maxTouchPoints ?? 0) > 1);
  if (mobile) return { kind: "table" };
  if (/Win/i.test(platform)) return { kind: "desktop", build: "windows" };
  if (/Mac/i.test(platform)) return { kind: "desktop", build: "macos" };
  if (/Linux/i.test(platform)) return { kind: "desktop", build: /aarch64|arm64/i.test(`${platform} ${ua}`) ? "linux-arm" : "linux" };
  return { kind: "table" };
}

export function buildUrl(version: string, build: BuildId): string {
  return `${RELEASES_URL}zeron-${version}-${DESKTOP_BUILDS[build].file}`;
}

export function targetUrl(target: Target, version: string): string | null {
  if (target.kind === "desktop") return buildUrl(version, target.build);
  return target.kind === "android" ? ANDROID_APK : null;
}

/** `latest.txt`'s answer as a version to link to, or null when it isn't one
 *  or is older than the fallback (a stale cache must not roll links back). */
export function acceptVersion(text: string, floor: string = FALLBACK_VERSION): string | null {
  const version = text.trim();
  if (!/^\d+\.\d+\.\d+$/.test(version)) return null;
  const a = version.split(".").map(Number);
  const b = floor.split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    if (a[i]! !== b[i]!) return a[i]! > b[i]! ? version : null;
  }
  return version;
}
