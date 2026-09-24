/**
 * The one id mint (`crates/ui` mints `Uuid::new_v4()` in Rust, which has no
 * context gate). `crypto.randomUUID` is only exposed in secure contexts —
 * HTTPS or localhost — and the engine serves the web client itself over
 * plain HTTP on the LAN (`http://<lan-host>:<port>`), where it is
 * `undefined` and every bare call site breaks with "crypto.randomUUID is
 * not a function". `crypto.getRandomValues` IS available there, so a
 * standards-shaped v4 is possible everywhere; the final arm is a
 * collision-unlikely fallback for environments with neither.
 */

let warnedNoUuid = false;

export function mintId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6]! & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8]! & 0x3f) | 0x80; // variant 10
    const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
    warnOnce();
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  }
  warnOnce();
  return `id-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;
}

/** Dev-only, once per page load: surface the non-secure context, not silence. */
function warnOnce(): void {
  if (warnedNoUuid) {
    return;
  }
  warnedNoUuid = true;
  if (typeof import.meta.env !== "undefined" && import.meta.env.DEV === true) {
    console.warn(
      "crypto.randomUUID is unavailable (non-secure context); mintId is falling back to getRandomValues.",
    );
  }
}
