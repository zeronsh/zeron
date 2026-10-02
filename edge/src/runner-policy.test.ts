import { describe, expect, it } from "vitest";
import type { Verified } from "./auth";
import { runnerRefusal } from "./runner-policy";

const runner: Verified = { userId: "u", orgId: "o", kind: "runner", deviceId: "cloud-1" };
const user: Verified = { userId: "u", orgId: "o", kind: "user" };
const refused = (auth: Verified, method: string, path: string) =>
  runnerRefusal(auth, method, new URL(`https://edge.test${path}`)) !== undefined;

const REFUSED: [string, string][] = [
  ["GET", "/auth/orgs"],
  ["GET", "/cloud/o"],
  ["POST", "/cloud/o/enable"],
  ["DELETE", "/cloud/o"],
  ["GET", "/cloud/o/usage"],
  ["GET", "/vault/o"],
  ["PUT", "/vault/o/credentials/codex"],
  ["POST", "/vault/o/devices/cloud-1/revoke"],
  ["POST", "/vault/o/disable"],
  ["POST", "/vault/o/github/device"],
  ["GET", "/vault/o/grant"],
  // Another device's room: its RPC surface.
  ["GET", "/device/laptop-1/ws?role=client"],
  ["GET", "/device/laptop-1/ws?role=host"],
  ["GET", "/device/laptop-1/ws?role=HOST"],
  ["POST", "/device/laptop-1/nudge"],
  ["GET", "/device/laptop-1/sidecar/repos"],
  ["POST", "/device/laptop-1/sidecar/repos"],
  ["POST", "/device/laptop-1/status"],
  // Legacy whole-user loro rooms.
  ["GET", "/session/c1/ws"],
  ["GET", "/tail/c1"],
  ["GET", "/stats/c1"],
  ["GET", "/diff/c1"],
  ["POST", "/diff/c1"],
  ["GET", "/snapshot/c1"],
  ["POST", "/append/c1"],
  ["GET", "/workspace/o/ws"],
  ["GET", "/workspace/o/tail"],
  // Registry internals.
  ["GET", "/registry/o/stats"],
  ["POST", "/registry/o/reset"],
  ["POST", "/registry/o/push-target"],
  ["DELETE", "/registry/o/push-target"]
];

describe("runner policy", () => {
  it("refuses account, vault admin, other devices, legacy rooms and registry internals — for runners only", () => {
    for (const [method, path] of REFUSED) {
      expect(refused(runner, method, path), `${method} ${path}`).toBe(true);
      expect(refused(user, method, path), `${method} ${path} (user)`).toBe(false);
    }
  });

  it("allows grants, its own device room, other devices' liveness, and row sync", () => {
    for (const [method, path] of [
      ["POST", "/vault/o/grant"],
      ["GET", "/device/cloud-1/ws?role=host"],
      ["GET", "/device/cloud-1/ws?role=client"],
      ["POST", "/device/cloud-1/nudge"],
      ["POST", "/device/cloud-1/sidecar/repos"],
      ["GET", "/device/laptop-1/status"],
      ["GET", "/registry/o/ws"],
      ["GET", "/registry/o/rows"],
      ["POST", "/registry/o/push"],
      // Gated per chat by runner-access.ts, not by path.
      ["GET", "/chat2/c1/ws"],
      ["PUT", "/blob/c1/p1"],
      ["POST", "/runner/heartbeat"]
    ]) {
      expect(refused(runner, method!, path!), `${method} ${path}`).toBe(false);
    }
  });
});
