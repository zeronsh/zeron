/**
 * What a runner bearer (the Cloud device's runner JWT) may NOT do
 * (docs/design/cloud-device.md, "Runner identity" → "Trust boundary").
 *
 * The Cloud sandbox runs full-access agents on arbitrary repository code, so
 * a runner token must be treated as possibly stolen. It acts as ITS DEVICE,
 * not as the user: it never manages the account (`/auth/*` org routes,
 * `/cloud/*`), touches the vault beyond a grant for itself, reaches another
 * device's room (except read-only liveness), uses the legacy loro rooms, or
 * sees registry internals. Chat-scoped data (chat2 rooms, blobs) is gated
 * per chat on the registry's host field (runner-access.ts); registry rows are
 * filtered and ownership-checked inside RegistryRoom (registry-runner.ts).
 * Pure, so the whole path table is unit-testable without a Worker.
 */
import type { Verified } from "./auth";
import { CLOUD_DEVICE_PREFIX } from "./cloud/policy";

/** The pre-chat2 loro rooms (s2 sessions, workspace docs): whole-user
 * documents with no per-device scoping — never for a runner. */
const LEGACY_ROOTS = new Set(["session", "tail", "stats", "diff", "snapshot", "append", "workspace"]);

/** A reason string when the request must be refused with 403, else undefined. */
export const runnerRefusal = (auth: Verified, method: string, url: URL): string | undefined => {
  if (auth.kind !== "runner") return undefined;
  const parts = url.pathname.split("/").filter(Boolean);
  const root = parts[0] ?? "";
  if (root === "auth") return "runner tokens cannot manage the account";
  if (root === "cloud") {
    // Delegation: a runner starts, wakes, sleeps and deletes Cloud sessions
    // for the chat it hosts — the CloudAccount checks each is its own child.
    // Everything else under /cloud is the user's.
    const sessions = parts[2] === "sessions";
    if (sessions && parts.length === 3 && method === "POST") return undefined;
    if (sessions && parts.length === 4 && method === "DELETE") return undefined;
    if (sessions && parts.length === 5 && method === "POST" && (parts[4] === "wake" || parts[4] === "sleep")) return undefined;
    return "runner tokens cannot manage the account";
  }
  if (LEGACY_ROOTS.has(root)) return "runner tokens cannot use the legacy rooms";
  switch (root) {
    case "vault":
      // Grants are the one vault call a runner makes (for its own device; the
      // vault route re-checks the body's deviceId against the token).
      if (method === "POST" && parts.length === 3 && parts[2] === "grant") return undefined;
      return "runner tokens may only request vault grants";
    case "device":
      if (parts[1] === auth.deviceId) return undefined;
      // Another device's room is that device's RPC surface (terminals,
      // files, sidecars): read-only liveness, and — for the machine of a
      // delegate it started (checked by runnerDeviceGate) — a client link
      // and the command nudge. Never the host role.
      if (parts[2] === "status" && parts.length === 3 && method === "GET") return undefined;
      // Only ever another Cloud machine: a delegate's chat can be written on
      // a user's computer (its RPC surface: terminals, files, the user's
      // bearer), and a chat's parent alone must never open that.
      if (!(parts[1] ?? "").startsWith(CLOUD_DEVICE_PREFIX)) return "a runner may only use its own device room";
      if (parts[2] === "ws" && parts.length === 3 && (url.searchParams.get("role") ?? "client") === "client") return undefined;
      if (parts[2] === "nudge" && parts.length === 3 && method === "POST") return undefined;
      return "a runner may only use its own device room";
    case "registry":
      // Row sync only; stats (push targets/log), APNs targets (notification
      // titles of every chat) and the operator wipe are the user's.
      if (parts[2] === "ws" || (parts[2] === "rows" && method === "GET") || (parts[2] === "push" && method === "POST")) {
        return undefined;
      }
      return "runner tokens may only sync registry rows";
    case "preview":
      // Its own services only, as a publisher (the route pins the device to
      // the runner's, and the room never shows it anyone else's).
      if (parts.length === 3 && parts[2] === "ws") return undefined;
      return "runner tokens cannot use this route";
    case "chat2":
    case "blob":
      // Gated per chat on the registry's host field (runner-access.ts).
      return undefined;
    case "runner":
      // Its own heartbeat (the account DO checks the device).
      return undefined;
    case "attachments":
      // Retired mirror: acknowledged and discarded.
      return undefined;
    default:
      // Anything not named here is the user's, including routes added later.
      return "runner tokens cannot use this route";
  }
};
