import { describe, expect, it } from "vitest";
import {
  DISK_CRITICAL,
  DISK_WARN,
  FAST_STALE_MIN_MS,
  HEARTBEAT_STALE_MS,
  MACHINES,
  REFRESH_MARGIN_MS,
  diskLevel,
  heartbeatStaleMs,
  idleDecision,
  machineName,
  machineSpec,
  refreshDueAt,
  parseIdleMs,
  parseTtlSeconds,
  shouldExtendTtl,
  tokenTimestampCheck,
  workflowInstanceId
} from "./policy";

describe("the machine catalog", () => {
  it("names every size once, smallest first, and only those", () => {
    expect(MACHINES.map((m) => m.id)).toEqual(["small", "default", "large", "xlarge"]);
    for (const machine of MACHINES) expect(machineName(machine.id)).toBe(machine.id);
    expect(machineName("huge")).toBeUndefined();
    expect(machineName(1)).toBeUndefined();
    expect(machineName(undefined)).toBeUndefined();
    const disks = MACHINES.map((m) => m.diskGiB);
    expect([...disks].sort((a, b) => a - b)).toEqual(disks);
  });

  it("carries what Boat's refusal said large holds", () => {
    expect(machineSpec("large")?.diskGiB).toBe(111.8);
  });
});

describe("disk levels", () => {
  it("warns at 80% and is critical at 90% of what the size holds", () => {
    expect(diskLevel(79.9, 100)).toBe("ok");
    expect(diskLevel(100 * DISK_WARN, 100)).toBe("warn");
    expect(diskLevel(89.9, 100)).toBe("warn");
    expect(diskLevel(100 * DISK_CRITICAL, 100)).toBe("critical");
    expect(diskLevel(118.3, 111.8)).toBe("critical");
    expect(diskLevel(0, 51)).toBe("ok");
  });
});

describe("a sleeping machine's refresh", () => {
  it("falls due before the retention ends, with room for another try", () => {
    const retention = 14 * 24 * 60 * 60_000;
    expect(refreshDueAt(1_000, retention)).toBe(1_000 + retention - REFRESH_MARGIN_MS);
    expect(REFRESH_MARGIN_MS).toBeGreaterThanOrEqual(24 * 60 * 60_000);
  });
});

describe("runner token timestamps", () => {
  const now = 1_800_000_000_000;
  it("accepts ±120 s and strictly increasing", () => {
    expect(tokenTimestampCheck(now - 119_000, undefined, now)).toBe("ok");
    expect(tokenTimestampCheck(now + 119_000, now - 1, now)).toBe("ok");
    expect(tokenTimestampCheck(now - 121_000, undefined, now)).toBe("stale");
    expect(tokenTimestampCheck(now + 121_000, undefined, now)).toBe("stale");
    expect(tokenTimestampCheck(now, now, now)).toBe("replay");
    expect(tokenTimestampCheck(now - 5, now, now)).toBe("replay");
    expect(tokenTimestampCheck(1.5, undefined, now)).toBe("stale");
  });
});

describe("idle decision", () => {
  const idleMs = 20 * 60_000;
  const now = 1_800_000_000_000;
  it("sleeps after the idle window with nothing running", () => {
    expect(idleDecision({ now, idleMs, lastActiveAt: now - idleMs, lastHeartbeatAt: now - 30_000, activeRuns: 0, clients: 0 })).toEqual({ sleep: true });
    expect(idleDecision({ now, idleMs, lastActiveAt: now - 60_000, lastHeartbeatAt: now, activeRuns: 0, clients: 0 })).toEqual({
      sleep: false,
      checkAt: now - 60_000 + idleMs
    });
  });
  it("stays awake while a fresh heartbeat reports runs or clients", () => {
    const busy = idleDecision({ now, idleMs, lastActiveAt: now - 2 * idleMs, lastHeartbeatAt: now - 10_000, activeRuns: 1, clients: 0 });
    expect(busy).toEqual({ sleep: false, checkAt: now + idleMs });
    expect(idleDecision({ now, idleMs, lastActiveAt: now - 2 * idleMs, lastHeartbeatAt: now, activeRuns: 0, clients: 2 }).sleep).toBe(false);
  });
  it("ignores a stale heartbeat's busy report (dead engine)", () => {
    expect(
      idleDecision({ now, idleMs, lastActiveAt: now - idleMs, lastHeartbeatAt: now - HEARTBEAT_STALE_MS, activeRuns: 3, clients: 1 })
    ).toEqual({ sleep: true });
  });
});

describe("TTL extension and config parsing", () => {
  it("extends a TTL'd busy sandbox at most every 10 minutes", () => {
    const now = 1_800_000_000_000;
    expect(shouldExtendTtl({ ttlSeconds: 7200, busy: true, lastExtendedAt: undefined, now })).toBe(true);
    expect(shouldExtendTtl({ ttlSeconds: 7200, busy: true, lastExtendedAt: now - 9 * 60_000, now })).toBe(false);
    expect(shouldExtendTtl({ ttlSeconds: 7200, busy: true, lastExtendedAt: now - 10 * 60_000, now })).toBe(true);
    expect(shouldExtendTtl({ ttlSeconds: 7200, busy: false, lastExtendedAt: undefined, now })).toBe(false);
    expect(shouldExtendTtl({ ttlSeconds: null, busy: true, lastExtendedAt: undefined, now })).toBe(false);
  });
  it("parses CLOUD_TTL_SECONDS and CLOUD_IDLE_MINUTES", () => {
    expect(parseTtlSeconds(undefined)).toBeNull();
    expect(parseTtlSeconds("")).toBeNull();
    expect(parseTtlSeconds("null")).toBeNull();
    expect(parseTtlSeconds("7200")).toBe(7200);
    expect(parseTtlSeconds("-1")).toBeNull();
    expect(parseIdleMs(undefined)).toBe(10 * 60_000);
    expect(parseIdleMs("5")).toBe(5 * 60_000);
    expect(parseIdleMs("nope")).toBe(10 * 60_000);
  });
  it("derives deterministic workflow ids per (device, generation)", () => {
    expect(workflowInstanceId("provision", "cloud-abc", 3)).toBe("prov-cloud-abc-3");
    expect(workflowInstanceId("wake", "cloud-abc", 4)).toBe("wake-cloud-abc-4");
    expect(workflowInstanceId("sleep", "cloud-abc", 4)).toBe(workflowInstanceId("sleep", "cloud-abc", 4));
    // Cloudflare caps instance ids at 100 chars; a uuid device id leaves room.
    expect(workflowInstanceId("provision", `cloud-${crypto.randomUUID()}`, 1_000_000).length).toBeLessThanOrEqual(64);
  });
});

describe("heartbeatStaleMs", () => {
  it("is three fast beats, never under 45 s, and 3 min for the idle pace or an engine that doesn't say", () => {
    expect(heartbeatStaleMs(15_000)).toBe(FAST_STALE_MIN_MS);
    expect(heartbeatStaleMs(5_000)).toBe(FAST_STALE_MIN_MS);
    expect(heartbeatStaleMs(20_000)).toBe(60_000);
    expect(heartbeatStaleMs(60_000)).toBe(HEARTBEAT_STALE_MS);
    expect(heartbeatStaleMs(undefined)).toBe(HEARTBEAT_STALE_MS);
    expect(heartbeatStaleMs(0)).toBe(HEARTBEAT_STALE_MS);
  });
});
