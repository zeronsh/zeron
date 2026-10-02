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

/** The pre-chat2 loro rooms (s2 sessions, workspace docs): whole-user
 * documents with no per-device scoping — never for a runner. */
const LEGACY_ROOTS = new Set(["session", "tail", "stats", "diff", "snapshot", "append", "workspace"]);

/** A reason string when the request must be refused with 403, else undefined. */
export const runnerRefusal = (auth: Verified, method: string, url: URL): string | undefined => {
  if (auth.kind !== "runner") return undefined;
  const parts = url.pathname.split("/").filter(Boolean);
  const root = parts[0] ?? "";
  if (root === "auth" || root === "cloud") return "runner tokens cannot manage the account";
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
      // files, sidecars): read-only liveness only.
      if (parts[2] === "status" && parts.length === 3 && method === "GET") return undefined;
      return "a runner may only use its own device room";
    case "registry":
      // Row sync only; stats (push targets/log), APNs targets (notification
      // titles of every chat) and the operator wipe are the user's.
      if (parts[2] === "ws" || (parts[2] === "rows" && method === "GET") || (parts[2] === "push" && method === "POST")) {
        return undefined;
      }
      return "runner tokens may only sync registry rows";
    default:
      return undefined;
  }
};
