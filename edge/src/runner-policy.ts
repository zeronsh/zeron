/**
 * What a runner bearer (the Cloud device's runner JWT) may NOT do
 * (docs/design/cloud-device.md, "Runner identity" → "Trust boundary").
 *
 * The Cloud sandbox runs full-access agents on arbitrary repository code, so
 * a runner token must be treated as possibly stolen. It acts as ITS DEVICE,
 * not as the user: it never manages the account (`/auth/*` org routes,
 * `/cloud/*`), touches the vault beyond a grant for itself, hosts another
 * device's room, uses the legacy loro rooms, or sees registry internals. Like
 * any of the person's devices it reaches their other devices (a client link:
 * their chats, terminals and files) and every chat's data (chat2 rooms,
 * blobs); its registry writes are ownership-checked inside RegistryRoom
 * (registry-runner.ts).
 * Pure, so the whole path table is unit-testable without a Worker.
 */
import type { Verified } from "./auth";

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
      // Another device's room is that device's RPC surface (chats,
      // terminals, files): read-only liveness, a client link and the command
      // nudge (a message to a chat it hosts), as any of the person's devices
      // has. Never the host role, never its sidecars.
      if (parts[2] === "status" && parts.length === 3 && method === "GET") return undefined;
      if (parts[2] === "ws" && parts.length === 3 && (url.searchParams.get("role") ?? "client") === "client") return undefined;
      if (parts[2] === "nudge" && parts.length === 3 && method === "POST") return undefined;
      return "a runner reaches another device's room only as a client";
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
      // Any of the person's chats, as their devices reach them.
      return undefined;
    case "runner":
      // Its own heartbeat (the account DO checks the device).
      return undefined;
    case "attachments":
      // Retired mirror: acknowledged and discarded.
      return undefined;
    case "plugin-keys":
      // Who made a plugin: a lookup of one key (its owner's username, and
      // whether it's this account's), so a Cloud machine can tell its
      // plugins' authors. Registering, listing and revoking keys bind or
      // unbind them from the account: the user's alone.
      if (method === "GET" && parts.length === 2) return undefined;
      return "runner tokens may only look up plugin keys";
    default:
      // Anything not named here is the user's, including routes added later.
      return "runner tokens cannot use this route";
  }
};
