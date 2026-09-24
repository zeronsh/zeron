import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeScopedId, methods, RpcError } from "@zeron/engine-client";
import type { Space, WorkspaceFileText, WorkspaceImageChunk } from "@zeron/proto";
import type { FilesCaller, FilesTarget } from "../src/lib/files-client";
import {
  ICON_PATHS,
  PROJECT_ICON_RETRY_MS,
  PROJECT_ICON_TTL_MS,
  ProjectIconsStore,
  iconDataUrl,
  isRetryableFilesError,
  probeWorkspaceIcon,
} from "../src/lib/project-icons";

/**
 * Ticket 05 — the web port of the desktop's client-side icon discovery
 * (project_icon.rs): the ICON_PATHS probe over a scripted fake files
 * client (the repo's vitest pattern — nothing renders components in
 * tests; the component wiring is `tsc --noEmit`'s job), plus the
 * per-space cache's fill/invalidate lifecycle.
 */

const ENGINE = "http://engine.local";
const SPACE_ID = encodeScopedId(ENGINE, "space-1");
const DEVICE_ID = encodeScopedId(ENGINE, "device-1");
const TARGET: FilesTarget = { spaceId: SPACE_ID, targetDeviceId: DEVICE_ID };

function encode(bytes: readonly number[]): string {
  return btoa(String.fromCharCode(...bytes));
}

interface HarnessOptions {
  /** Present files by workspace path → checkout identity. */
  readonly files?: Readonly<Record<string, string>>;
  /** Present images by path. */
  readonly images?: Readonly<Record<string, { readonly mimeType: string; readonly bytes: readonly number[] }>>;
  /** Per-call read error factory (null = default missing-file error). */
  readonly readError?: (path: string, read: number) => Error | null;
  /** Reads never resolve — the probe-budget race. */
  readonly hangReads?: boolean;
}

/**
 * The scripted fake files caller: `ReadWorkspaceFile` answers with the
 * path's checkout identity (a failed/missing read is an engine ANSWERED
 * error — non-retryable, the desktop continues to the next candidate),
 * `ReadWorkspaceImage` answers with a single done chunk.
 */
function filesHarness(options: HarnessOptions = {}) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  let reads = 0;
  const caller: FilesCaller = {
    async call<T>(method: string, params?: unknown): Promise<T> {
      const request = (params ?? {}) as Record<string, unknown>;
      calls.push({ method, params: request });
      const path = request.path as string;
      if (method === methods.READ_WORKSPACE_FILE) {
        reads += 1;
        if (options.hangReads === true) {
          await new Promise<never>(() => undefined);
        }
        const error = options.readError?.(path, reads) ?? null;
        if (error !== null) {
          throw error;
        }
        const checkoutId = options.files?.[path];
        if (checkoutId === undefined) {
          throw new RpcError("failed", "file not found");
        }
        const file: WorkspaceFileText = {
          checkoutId,
          path,
          size: 0,
          encoding: "binary",
          truncated: false,
        };
        return file as T;
      }
      if (method === methods.READ_WORKSPACE_IMAGE) {
        const image = options.images?.[path];
        if (image === undefined) {
          throw new RpcError("failed", "image not found");
        }
        const chunk: WorkspaceImageChunk = {
          checkoutId: request.expectedCheckoutId as string,
          contentHash: "hash-1",
          mimeType: image.mimeType,
          data: encode(image.bytes),
          nextOffset: image.bytes.length,
          size: image.bytes.length,
          done: true,
        };
        return chunk as T;
      }
      throw new Error(`unexpected method: ${method}`);
    },
  };
  return { calls, caller, reads: () => reads };
}

function space(fields: Partial<Space> & { readonly id: string; readonly deviceId: string }): Space {
  return {
    path: "/srv/project",
    name: null,
    gitDetected: true,
    checkoutId: "co-1",
    createdAt: "2026-01-01T00:00:00Z",
    ...fields,
  };
}

const projectSpace = () => space({ id: SPACE_ID, deviceId: DEVICE_ID });
const ICON_PNG = { mimeType: "image/png", bytes: [1, 2, 3] };

describe("probeWorkspaceIcon (project_icon.rs's ICON_PATHS probe)", () => {
  it("walks the candidates in priority order and returns the first artwork found", async () => {
    const harness = filesHarness({
      files: { "public/favicon.png": "co-1" },
      images: { "public/favicon.png": ICON_PNG },
    });
    const outcome = await probeWorkspaceIcon(harness.caller, TARGET);
    expect(outcome).toEqual({ kind: "icon", src: iconDataUrl("image/png", new Uint8Array([1, 2, 3])) });
    // Misses fall through in ICON_PATHS order until the first hit.
    const readPaths = harness.calls
      .filter((call) => call.method === methods.READ_WORKSPACE_FILE)
      .map((call) => call.params.path);
    expect(readPaths).toEqual(ICON_PATHS.slice(0, ICON_PATHS.indexOf("public/favicon.png") + 1));
    // Every read request carries the space target and the routing hint.
    for (const call of harness.calls) {
      expect(call.params.spaceId).toBe(SPACE_ID);
      expect(call.params.targetDeviceId).toBe(DEVICE_ID);
    }
  });

  it("an engine-answered miss (no candidate exists) is a miss, reading every candidate", async () => {
    const harness = filesHarness({});
    const outcome = await probeWorkspaceIcon(harness.caller, TARGET);
    expect(outcome).toEqual({ kind: "miss" });
    expect(harness.calls).toHaveLength(ICON_PATHS.length);
    expect(harness.calls.map((call) => call.params.path)).toEqual(ICON_PATHS);
  });

  it("a transport-class read error aborts the whole probe (no lower-priority art)", async () => {
    const harness = filesHarness({
      readError: () => new RpcError("transport", "Engine is offline; reconnecting"),
    });
    const outcome = await probeWorkspaceIcon(harness.caller, TARGET);
    expect(outcome).toEqual({ kind: "offline" });
    expect(harness.calls).toHaveLength(1);
  });

  it("a readImage failure after a resolved checkout is a miss, not a fall-through", async () => {
    const harness = filesHarness({ files: { "public/apple-touch-icon.png": "co-1" } });
    const outcome = await probeWorkspaceIcon(harness.caller, TARGET);
    expect(outcome).toEqual({ kind: "miss" });
    // One identity read, one image read — never a second candidate.
    expect(harness.calls.map((call) => call.method)).toEqual([
      methods.READ_WORKSPACE_FILE,
      methods.READ_WORKSPACE_IMAGE,
    ]);
  });

  it("the probe budget races a hung engine into a miss", async () => {
    const harness = filesHarness({ hangReads: true });
    const outcome = await probeWorkspaceIcon(harness.caller, TARGET, 5);
    expect(outcome).toEqual({ kind: "miss" });
    expect(harness.calls).toHaveLength(1);
  });

  it("isRetryableFilesError splits transport-class from engine-answered errors", () => {
    for (const kind of ["transport", "closed", "parked", "timeout"] as const) {
      expect(isRetryableFilesError(new RpcError(kind, "offline"))).toBe(true);
    }
    for (const kind of ["failed", "unknown-method", "bad-reply"] as const) {
      expect(isRetryableFilesError(new RpcError(kind, "answered"))).toBe(false);
    }
    expect(isRetryableFilesError(new Error("not an rpc error"))).toBe(false);
    expect(isRetryableFilesError(null)).toBe(false);
  });

  it("iconDataUrl encodes the bytes as a data URL under the engine's mime", () => {
    expect(iconDataUrl("image/svg+xml", new Uint8Array([60, 115, 118, 103]))).toBe(
      "data:image/svg+xml;base64,PHN2Zw==",
    );
    expect(iconDataUrl("image/png", new Uint8Array([1, 2]))).toBe("data:image/png;base64,AQI=");
  });
});

describe("ProjectIconsStore (per-space cache + space-list invalidation)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** Flush the probe's promise chain: the 17-candidate walk is sequential
   * (one readFile round trip per candidate), so it needs many microtask
   * ticks — fake timers hold macrotasks back. */
  async function settle(): Promise<void> {
    for (let tick = 0; tick < 200; tick += 1) {
      await Promise.resolve();
    }
  }

  function sessionsOf(harness: { readonly caller: FilesCaller }): Map<string, { client: FilesCaller }> {
    return new Map([[ENGINE, { client: harness.caller }]]);
  }

  it("probes a space through its owning engine and caches the artwork", async () => {
    const harness = filesHarness({
      files: { "public/apple-touch-icon.png": "co-1" },
      images: { "public/apple-touch-icon.png": ICON_PNG },
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], sessionsOf(harness), null);
    expect(store.iconOf(SPACE_ID)).toBeNull(); // loading: monogram
    await settle();
    expect(store.iconOf(SPACE_ID)).toBe(iconDataUrl("image/png", new Uint8Array([1, 2, 3])));
    // Routing: the owning engine, the space target, the device hint.
    expect(harness.calls[0]?.params.spaceId).toBe(SPACE_ID);
    expect(harness.calls[0]?.params.targetDeviceId).toBe(DEVICE_ID);
  });

  it("the client's own device drops the routing hint (the desktop's local arm)", async () => {
    const harness = filesHarness({
      files: { "public/apple-touch-icon.png": "co-1" },
      images: { "public/apple-touch-icon.png": ICON_PNG },
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], sessionsOf(harness), DEVICE_ID);
    await settle();
    expect(harness.calls[0]?.params.targetDeviceId).toBeNull();
  });

  it("a repeat ensure re-uses the cache; nothing re-probes", async () => {
    const harness = filesHarness({
      files: { "public/apple-touch-icon.png": "co-1" },
      images: { "public/apple-touch-icon.png": ICON_PNG },
    });
    const store = new ProjectIconsStore();
    const spaces = [projectSpace()];
    store.ensure(spaces, sessionsOf(harness), null);
    await settle();
    const reads = harness.reads();
    // A fresh array identity (every registry publication) must not churn.
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(harness.reads()).toBe(reads);
    expect(store.iconOf(SPACE_ID)).not.toBeNull();
  });

  it("an engine without a session is never probed and never cached", async () => {
    const harness = filesHarness({
      files: { "public/apple-touch-icon.png": "co-1" },
      images: { "public/apple-touch-icon.png": ICON_PNG },
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], new Map(), null);
    await settle();
    expect(store.iconOf(SPACE_ID)).toBeNull();
    expect(harness.calls).toHaveLength(0);
    // The connection arriving later fills the cache.
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(store.iconOf(SPACE_ID)).not.toBeNull();
  });

  it("a space leaving the list drops its entry (invalidate with the space list)", async () => {
    const other = space({ id: encodeScopedId(ENGINE, "space-2"), deviceId: DEVICE_ID, path: "/srv/other" });
    const harness = filesHarness({
      files: {
        "public/apple-touch-icon.png": "co-1",
        "favicon.svg": "co-2",
      },
      images: {
        "public/apple-touch-icon.png": ICON_PNG,
        "favicon.svg": { mimeType: "image/svg+xml", bytes: [60] },
      },
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace(), other], sessionsOf(harness), null);
    await settle();
    expect(store.iconOf(other.id)).not.toBeNull();
    store.ensure([projectSpace()], sessionsOf(harness), null);
    expect(store.iconOf(other.id)).toBeNull();
    expect(store.iconOf(SPACE_ID)).not.toBeNull();
  });

  it("a checkout identity change re-probes the space", async () => {
    const harness = filesHarness({
      files: { "public/apple-touch-icon.png": "co-1" },
      images: { "public/apple-touch-icon.png": ICON_PNG },
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    const reads = harness.reads();
    store.ensure([space({ id: SPACE_ID, deviceId: DEVICE_ID, checkoutId: "co-2" })], sessionsOf(harness), null);
    await settle();
    expect(harness.reads()).toBe(reads + 1);
    expect(store.iconOf(SPACE_ID)).not.toBeNull();
  });

  it("the 300 s TTL evicts a settled entry and re-probes it", async () => {
    const harness = filesHarness({
      files: { "public/apple-touch-icon.png": "co-1" },
      images: { "public/apple-touch-icon.png": ICON_PNG },
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    const reads = harness.reads();
    vi.setSystemTime(Date.now() + PROJECT_ICON_TTL_MS + 1);
    store.ensure([projectSpace()], sessionsOf(harness), null);
    expect(store.iconOf(SPACE_ID)).toBeNull(); // loading again
    await settle();
    expect(harness.reads()).toBe(reads + 1);
    expect(store.iconOf(SPACE_ID)).not.toBeNull();
  });

  it("a transport failure stays a monogram (un-cached) and retries after the budget", async () => {
    const harness = filesHarness({
      readError: (path, read) => (read === 1 ? new RpcError("transport", "Engine is offline; reconnecting") : null),
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(store.iconOf(SPACE_ID)).toBeNull();
    expect(harness.reads()).toBe(1);
    // Within the retry floor, ensure never re-probes.
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(harness.reads()).toBe(1);
    // Once the floor passes, the next ensure retries the whole probe.
    vi.setSystemTime(Date.now() + PROJECT_ICON_RETRY_MS + 1);
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(harness.reads()).toBe(1 + ICON_PATHS.length);
    expect(store.iconOf(SPACE_ID)).toBeNull(); // still a miss: monogram
  });

  it("a settled miss caches for the TTL (the desktop's 300 s miss)", async () => {
    const harness = filesHarness({});
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(harness.reads()).toBe(ICON_PATHS.length);
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(harness.reads()).toBe(ICON_PATHS.length);
  });

  it("dropAll clears the cache (the view toggle's off arm)", async () => {
    const harness = filesHarness({
      files: { "public/apple-touch-icon.png": "co-1" },
      images: { "public/apple-touch-icon.png": ICON_PNG },
    });
    const store = new ProjectIconsStore();
    store.ensure([projectSpace()], sessionsOf(harness), null);
    await settle();
    expect(store.iconOf(SPACE_ID)).not.toBeNull();
    store.dropAll();
    expect(store.iconOf(SPACE_ID)).toBeNull();
  });

  it("unscoped space ids never probe", async () => {
    const harness = filesHarness({});
    const store = new ProjectIconsStore();
    store.ensure([space({ id: "space-1", deviceId: "device-1" })], sessionsOf(harness), null);
    await settle();
    expect(harness.calls).toHaveLength(0);
    expect(store.iconOf("space-1")).toBeNull();
  });
});
