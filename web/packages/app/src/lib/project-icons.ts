import type { Space, WorkspaceFileText } from "@zeron/proto";
import { parseScopedId, RpcError } from "@zeron/engine-client";
import { WorkspaceFilesClient, type FilesCaller, type FilesTarget } from "./files-client";

/**
 * Project artwork discovery — the web peer of
 * `crates/ui/src/shell/project_icon.rs` (upstream ffaa3102 "monorepo icons").
 *
 * The `Space` proto payload carries no icon field; the desktop discovers
 * artwork CLIENT-SIDE by probing a fixed candidate list in the space's repo
 * working tree. The desktop reads local spaces straight off disk; the web
 * has no local disk, so EVERY probe rides the existing workspace-files RPC
 * surface (`ReadWorkspaceFile` resolves the checkout identity, then the
 * chunked `ReadWorkspaceImage` carries the bytes) through the space's
 * OWNING engine — the same routing the desktop uses for remote spaces.
 * Miss/failure degrade to the monogram (`lib/monogram.ts`), exactly like
 * the desktop.
 */

/**
 * The fixed candidate list (project_icon.rs:6-24), in priority order. NOT
 * the git remote host, NOT workspace metadata, NOT a dedicated icon RPC.
 */
export const ICON_PATHS: readonly string[] = [
  "public/apple-touch-icon.png",
  "apple-touch-icon.png",
  "public/favicon.svg",
  "favicon.svg",
  "public/favicon.png",
  "public/icon.png",
  "public/logo.png",
  "favicon.png",
  "app/icon.png",
  "src/app/icon.png",
  "public/favicon.ico",
  "favicon.ico",
  "app/favicon.ico",
  "static/favicon.ico",
  "src-tauri/icons/icon.png",
  "assets/icon.png",
  "src/assets/icon.png",
];

/** Cache TTL (project_icon.rs's 300 s `refreshed` eviction). */
export const PROJECT_ICON_TTL_MS = 300_000;

/** Probe budget (project_icon.rs's 30 s load race). */
export const PROJECT_ICON_TIMEOUT_MS = 30_000;

/**
 * Retry floor for an engine that was unreachable when probed: the desktop
 * never caches a remote miss before a connection exists; the web's closest
 * equivalent is a transport-class failure, which stays un-cached (the mark
 * shows the monogram) and re-probes once the probe budget has elapsed.
 */
export const PROJECT_ICON_RETRY_MS = PROJECT_ICON_TIMEOUT_MS;

/** The engine session slice the probe needs (structural, testable). */
export interface ProjectIconSession {
  readonly client: FilesCaller;
}

/** A probe's settled outcome. */
export type ProjectIconOutcome =
  | { readonly kind: "icon"; readonly src: string }
  | { readonly kind: "miss" }
  | { readonly kind: "offline" };

/** One cached space's icon state. */
export interface ProjectIconEntry {
  readonly kind: "loading" | "ready" | "offline";
  /** The artwork's data URL, or null for a miss (monogram). */
  readonly src: string | null;
  /** The checkout identity the probe ran under; a change re-probes. */
  readonly checkoutId: string | null;
  /** When the entry settled (or the attempt started, while offline). */
  readonly at: number;
}

/**
 * Desktop `FilesClientError::retryable` — Transport-class failures abort
 * the whole probe (never fall through to lower-priority art), while an
 * engine's answered errors (not found, unsupported) continue to the next
 * candidate. The web kinds that mean "no answer": offline/dial failures,
 * closed or parked clients, and the call timeout.
 */
export function isRetryableFilesError(error: unknown): boolean {
  return (
    error instanceof RpcError &&
    (error.kind === "transport" ||
      error.kind === "closed" ||
      error.kind === "parked" ||
      error.kind === "timeout")
  );
}

/** The artwork as a data URL — icons are a few KB, so no object-URL lifetime. */
export function iconDataUrl(mimeType: string, bytes: Uint8Array): string {
  return `data:${mimeType};base64,${bytesToBase64(bytes)}`;
}

/**
 * The probe: walk `ICON_PATHS` in order through the space's owning engine.
 * A transport-class failure aborts the whole probe (`offline`); a
 * candidate the engine answered negatively (missing / unsupported) falls
 * through to the next; a successful `readFile` resolves the checkout
 * identity, whose `readImage` failure is a miss. The desktop's 30 s budget
 * races the walk and a timeout reads as a miss. Never rejects.
 */
export async function probeWorkspaceIcon(
  caller: FilesCaller,
  target: FilesTarget,
  timeoutMs: number = PROJECT_ICON_TIMEOUT_MS,
): Promise<ProjectIconOutcome> {
  const client = new WorkspaceFilesClient(caller, target);
  const load = probeIconCandidates(client);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cap = new Promise<"timeout">((resolve) => {
    timer = setTimeout(() => resolve("timeout"), timeoutMs);
  });
  try {
    const outcome = await Promise.race([load, cap]);
    return outcome === "timeout" ? { kind: "miss" } : outcome;
  } finally {
    clearTimeout(timer);
  }
}

async function probeIconCandidates(client: WorkspaceFilesClient): Promise<ProjectIconOutcome> {
  for (const path of ICON_PATHS) {
    let file: WorkspaceFileText;
    try {
      // This also resolves the current checkout identity on the owning host.
      file = await client.readFile(path);
    } catch (error) {
      if (isRetryableFilesError(error)) {
        return { kind: "offline" }; // Abort the whole probe.
      }
      continue; // Missing / unsupported: try the next candidate.
    }
    try {
      const image = await client.readImage(path, file.checkoutId);
      return { kind: "icon", src: iconDataUrl(image.mimeType, image.bytes) };
    } catch {
      return { kind: "miss" }; // readImage failure (desktop's `.ok()?`).
    }
  }
  return { kind: "miss" };
}

/**
 * The app-global icon cache — the web peer of `Shell.project_icons`'s
 * entity map: one entry per space (the scoped space id embeds its owning
 * engine), settled by one probe, evicted by the 300 s TTL, and invalidated
 * wholesale by the space list — `ensure` drops entries whose space left
 * the fleet snapshot and re-probes entries whose checkout identity moved.
 * An engine without a session yet is never probed and never cached (the
 * desktop's "don't cache a remote miss before a connection exists").
 */
export class ProjectIconsStore {
  readonly #entries = new Map<string, ProjectIconEntry>();
  readonly #listeners = new Set<() => void>();

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** The artwork's data URL for a space, or null (loading, offline, miss). */
  iconOf(spaceId: string): string | null {
    const entry = this.#entries.get(spaceId);
    return entry !== undefined && entry.kind === "ready" ? entry.src : null;
  }

  /**
   * Reconcile the cache with the live space list: probe new spaces through
   * their owning engine's client, keep settled entries fresh (TTL +
   * checkout identity), retry offline ones after the retry floor, and drop
   * everything the list no longer carries.
   */
  ensure(
    spaces: readonly Space[],
    sessions: ReadonlyMap<string, ProjectIconSession>,
    localDeviceId: string | null,
  ): void {
    const now = Date.now();
    const seen = new Set<string>();
    let changed = false;
    for (const space of spaces) {
      const engineKey = engineKeyOf(space.id);
      if (engineKey === null) {
        continue;
      }
      seen.add(space.id);
      const entry = this.#entries.get(space.id);
      if (entry !== undefined && this.#retains(entry, space.checkoutId ?? null, now)) {
        continue;
      }
      if (entry !== undefined) {
        this.#entries.delete(space.id);
        changed = true;
      }
      const session = sessions.get(engineKey);
      if (session === undefined) {
        // No connection yet: render the monogram, cache nothing.
        continue;
      }
      this.#begin(space, session.client, localDeviceId);
      changed = true;
    }
    for (const key of this.#entries.keys()) {
      if (!seen.has(key)) {
        this.#entries.delete(key);
        changed = true;
      }
    }
    if (changed) {
      this.#notify();
    }
  }

  /** The sidebar toggle's off arm: hidden icons cache nothing. */
  dropAll(): void {
    if (this.#entries.size === 0) {
      return;
    }
    this.#entries.clear();
    this.#notify();
  }

  /** A settled entry survives until the TTL; an offline one until the retry floor. */
  #retains(entry: ProjectIconEntry, checkoutId: string | null, now: number): boolean {
    if (entry.checkoutId !== checkoutId) {
      return false;
    }
    const age = now - entry.at;
    switch (entry.kind) {
      case "loading":
        return true;
      case "ready":
        return age < PROJECT_ICON_TTL_MS;
      case "offline":
        return age < PROJECT_ICON_RETRY_MS;
    }
  }

  #begin(space: Space, client: FilesCaller, localDeviceId: string | null): void {
    const key = space.id;
    const entry: ProjectIconEntry = {
      kind: "loading",
      src: null,
      checkoutId: space.checkoutId ?? null,
      at: Date.now(),
    };
    this.#entries.set(key, entry);
    const target: FilesTarget = {
      spaceId: key,
      // The desktop's FilesRequestContext: route at the space-owning device
      // unless it is this client's own (projectActionContext's hint — set
      // when the local device is unknown, exactly like the desktop).
      targetDeviceId: localDeviceId === space.deviceId ? null : space.deviceId,
    };
    probeWorkspaceIcon(client, target).then((outcome) => {
      // A superseded entry (space vanished, TTL re-probe, checkout change)
      // never lands; neither does a probe that lost the 30 s race.
      if (this.#entries.get(key) !== entry) {
        return;
      }
      this.#entries.set(key, {
        kind: outcome.kind === "offline" ? "offline" : "ready",
        src: outcome.kind === "icon" ? outcome.src : null,
        checkoutId: entry.checkoutId,
        at: Date.now(),
      });
      this.#notify();
    });
  }

  #notify(): void {
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

/** The engine key a scoped space id names; null when unscoped or malformed. */
function engineKeyOf(spaceId: string): string | null {
  try {
    return parseScopedId(spaceId).engine;
  } catch {
    return null;
  }
}

/** Minimal base64 encoder (no dependency; `btoa` is unavailable in jsdom tests). */
function bytesToBase64(bytes: Uint8Array): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  let out = "";
  let i = 0;
  for (; i + 2 < binary.length; i += 3) {
    const a = binary.charCodeAt(i);
    const b = binary.charCodeAt(i + 1);
    const c = binary.charCodeAt(i + 2);
    out += chars[a >> 2];
    out += chars[((a & 3) << 4) | (b >> 4)];
    out += chars[((b & 15) << 2) | (c >> 6)];
    out += chars[c & 63];
  }
  if (i < binary.length) {
    const a = binary.charCodeAt(i);
    const b = i + 1 < binary.length ? binary.charCodeAt(i + 1) : 0;
    out += chars[a >> 2];
    out += chars[((a & 3) << 4) | (b >> 4)];
    if (i + 1 < binary.length) {
      out += chars[(b & 15) << 2];
      out += "=";
    } else {
      out += "==";
    }
  }
  return out;
}

/** The app-global store — the `Shell.project_icons` field's peer. */
export const projectIconsStore = new ProjectIconsStore();
