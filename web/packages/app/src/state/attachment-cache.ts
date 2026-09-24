import { useSyncExternalStore } from "react";
import { readAttachmentImage } from "../lib/attachments";

/**
 * The transcript's attachment image cache — the web peer of
 * `crates/ui/src/attachments.rs`'s module-level `CACHE` (`ImageCache`).
 * Decoded images, keyed by `(deviceId, path)`, are seeded once and replayed
 * across renders. Loads are kicked via `beginLoad` (so a row mounts without
 * a race); results arrive through a per-key `Promise` and notify subscribers
 * on commit.
 *
 * Budget discipline (`attachments.rs:582-654`): every loaded entry carries its
 * encoded byte size and a `lastUsed` LRU tick (bumped on every snapshot
 * read). Once the retained bytes exceed `IMAGE_CACHE_BUDGET_BYTES`, the
 * globally-oldest entry that is neither the one just inserted nor in the
 * protected set is evicted, repeatedly, until the cache is under budget. The
 * protected set (`protect_attachments`) is REPLACED wholesale with exactly
 * the attachments on screen, so a visible thumbnail can never be evicted out
 * from under the user — only off-screen images in other chats remain
 * evictable.
 *
 * Errors back off with the desktop's 2s→15s ladder — the row paints a
 * skeleton and schedules its own re-attempt when the countdown lands.
 *
 * The same module hosts the send-wide upload-progress store
 * (`AppState.begin_upload_progress` / `upload_progress_percent`,
 * state.rs:1160-1186): the composer begins it with the send's total bytes,
 * the upload loop publishes committed bytes, and the transcript's
 * sending-overlay ring reads the percent.
 */

/** `IMAGE_CACHE_BUDGET_BYTES` (attachments.rs:585) — 64 MiB. */
export const IMAGE_CACHE_BUDGET_BYTES = 64 * 1024 * 1024;

interface CacheEntry {
  state: "loading" | "loaded" | "error";
  attempts: number;
  image?: { name: string; mime: string; bytes: Uint8Array };
  retryAt?: number;
  /** Encoded byte size while loaded — the eviction budget's unit. */
  bytes?: number;
  /** LRU tick of the last snapshot read. */
  lastUsed?: number;
  /**
   * Cached error snapshot (`useSyncExternalStore` requires a stable identity
   * between reads): rebuilt only when the retry countdown's second bucket
   * moves, so a mounted row re-renders at most once a second while waiting.
   */
  errorSnapshot?: { bucket: number; snapshot: AttachmentImageSnapshot };
  /** Cached loaded snapshot — identity-stable for the same image. */
  loadedSnapshot?: AttachmentImageSnapshot;
}

export interface AttachmentImageSnapshot {
  state: "loading" | "loaded" | "error";
  image: { name: string; mime: string; bytes: Uint8Array } | null;
  /** Milliseconds until another load is attempted (0 when no backoff). */
  retryIn: number;
}

/** `retry_delay` (attachments.rs:578-580): `2s << min(attempts-1, 3)`, capped 15s. */
export function retryDelayMs(attempts: number): number {
  return Math.min(2000 << Math.min(attempts - 1, 3), 15000);
}

/**
 * The shared loading/missing snapshot — ONE object, because
 * `useSyncExternalStore` re-renders endlessly when getSnapshot returns a new
 * identity per call (React error #185).
 */
const EMPTY_SNAPSHOT: AttachmentImageSnapshot = { state: "loading", image: null, retryIn: 0 };

function emptySnapshot(): AttachmentImageSnapshot {
  return EMPTY_SNAPSHOT;
}

/** Module-level so a freshly-mounted row picks up the seed from a prior
 *  send (the desktop's `seed_attachment` call). */
const cache = new Map<string, CacheEntry>();
const listeners = new Map<string, Set<() => void>>();
/** Monotonic access clock for LRU ordering (`ImageCache.tick`). */
let tick = 0;
/** Sum of every loaded entry's `bytes` (`ImageCache.loaded_bytes`). */
let loadedBytes = 0;
/** Keys shielded from eviction — the open transcript's attachments
 *  (`attachments.rs::protected`). Replaced wholesale. */
let protectedKeys = new Set<string>();

/** The cache's composite key, spelled once: `${deviceId}\0${path}`. */
export function attachmentCacheKey(deviceId: string, path: string): string {
  return `${deviceId}\0${path}`;
}

function notify(key: string): void {
  const set = listeners.get(key);
  if (set === undefined) {
    return;
  }
  for (const listener of set) {
    listener();
  }
}

/**
 * Replace the eviction shield with exactly the given keys
 * (`protect_attachments`, attachments.rs:651-654) — the transcript calls this
 * with what is on screen so a visible thumbnail is never evicted under
 * budget pressure.
 */
export function protectAttachments(keys: ReadonlySet<string>): void {
  protectedKeys = new Set(keys);
}

/**
 * `ImageCache::insert_loaded` (attachments.rs:599-631), minus the deferred
 * frees (the JS GC owns those): set/replace the entry, then evict the
 * globally-oldest non-protected entry until `loadedBytes` is under budget.
 * The just-inserted key is never its own eviction candidate.
 */
function insertLoaded(key: string, image: { name: string; mime: string; bytes: Uint8Array }): void {
  tick += 1;
  const previous = cache.get(key);
  if (previous !== undefined && previous.state === "loaded" && previous.bytes !== undefined) {
    loadedBytes -= previous.bytes;
  }
  cache.set(key, {
    state: "loaded",
    attempts: 0,
    image,
    bytes: image.bytes.byteLength,
    lastUsed: tick,
  });
  loadedBytes += image.bytes.byteLength;
  while (loadedBytes > IMAGE_CACHE_BUDGET_BYTES) {
    let oldestKey: string | null = null;
    let oldestTick = Number.POSITIVE_INFINITY;
    for (const [entryKey, entry] of cache) {
      if (entryKey === key || protectedKeys.has(entryKey) || entry.state !== "loaded") {
        continue;
      }
      const used = entry.lastUsed ?? 0;
      if (used < oldestTick) {
        oldestTick = used;
        oldestKey = entryKey;
      }
    }
    if (oldestKey === null) {
      break;
    }
    const evicted = cache.get(oldestKey);
    cache.delete(oldestKey);
    if (evicted !== undefined && evicted.bytes !== undefined) {
      loadedBytes -= evicted.bytes;
    }
  }
}

/** Seed the cache after a successful upload so the just-sent bubble's
 *  thumbnail renders from local bytes instead of round-tripping the host.
 *  Mirrors `seed_attachment` (state.rs / attachments.rs). */
export function seedAttachment(
  deviceId: string,
  path: string,
  image: { name: string; mime: string; bytes: Uint8Array },
): void {
  insertLoaded(attachmentCacheKey(deviceId, path), image);
  notify(attachmentCacheKey(deviceId, path));
}

/** Read the current snapshot for a (deviceId, path) tuple. Returns a
 *  snapshot-stable object: identity tracks the entry's state, so React's
 *  re-render only fires when something actually changed. Every read bumps
 *  the entry's LRU tick (`attachment_snapshot`). */
export function getAttachmentSnapshot(
  deviceId: string,
  path: string,
): AttachmentImageSnapshot {
  const key = attachmentCacheKey(deviceId, path);
  const entry = cache.get(key);
  if (entry === undefined) {
    return emptySnapshot();
  }
  tick += 1;
  entry.lastUsed = tick;
  if (entry.state === "loading") {
    return emptySnapshot();
  }
  if (entry.state === "loaded" && entry.image !== undefined) {
    if (entry.loadedSnapshot !== undefined && entry.loadedSnapshot.image === entry.image) {
      return entry.loadedSnapshot;
    }
    const snapshot: AttachmentImageSnapshot = { state: "loaded", image: entry.image, retryIn: 0 };
    entry.loadedSnapshot = snapshot;
    return snapshot;
  }
  const now = Date.now();
  const retryIn = entry.retryAt !== undefined ? Math.max(0, entry.retryAt - now) : 0;
  const bucket = Math.ceil(retryIn / 1000);
  if (entry.errorSnapshot !== undefined && entry.errorSnapshot.bucket === bucket) {
    return entry.errorSnapshot.snapshot;
  }
  const snapshot: AttachmentImageSnapshot = { state: "error", image: null, retryIn };
  entry.errorSnapshot = { bucket, snapshot };
  return snapshot;
}

/** Subscribe to changes for a (deviceId, path) tuple. Returns the
 *  unsubscribe; pass it to `useSyncExternalStore`. */
export function subscribeAttachment(
  deviceId: string,
  path: string,
  listener: () => void,
): () => void {
  const key = attachmentCacheKey(deviceId, path);
  let set = listeners.get(key);
  if (set === undefined) {
    set = new Set();
    listeners.set(key, set);
  }
  set.add(listener);
  return () => {
    const current = listeners.get(key);
    if (current === undefined) {
      return;
    }
    current.delete(listener);
    if (current.size === 0) {
      listeners.delete(key);
    }
  };
}

/** React hook: snapshot + subscription. Pass `null` for `path` when the
 *  row has no attachment; the hook returns a stable empty snapshot. */
export function useAttachmentImage(
  deviceId: string | null,
  path: string | null,
): AttachmentImageSnapshot {
  const subscribe = (listener: () => void): (() => void) => {
    if (deviceId === null || path === null) {
      return () => {};
    }
    return subscribeAttachment(deviceId, path, listener);
  };
  const getSnapshot = (): AttachmentImageSnapshot => {
    if (deviceId === null || path === null) {
      return emptySnapshot();
    }
    return getAttachmentSnapshot(deviceId, path);
  };
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** Claim the load for a (deviceId, path) tuple. Returns `true` iff the
 *  caller should start fetching now (so concurrent renders don't
 *  double-fetch). Errors back off with a 2s→15s ladder. */
export function beginAttachmentLoad(deviceId: string, path: string): boolean {
  const key = attachmentCacheKey(deviceId, path);
  const entry = cache.get(key);
  if (entry === undefined) {
    cache.set(key, { state: "loading", attempts: 0 });
    notify(key);
    return true;
  }
  if (entry.state === "loaded") {
    return false;
  }
  if (entry.state === "error") {
    const now = Date.now();
    if (entry.retryAt !== undefined && now < entry.retryAt) {
      return false;
    }
    cache.set(key, { state: "loading", attempts: entry.attempts });
    notify(key);
    return true;
  }
  return false;
}

/** Mark an entry as errored with the next retry time. */
export function storeAttachmentError(deviceId: string, path: string): void {
  const key = attachmentCacheKey(deviceId, path);
  const entry = cache.get(key);
  const attempts = (entry?.attempts ?? 0) + 1;
  const delay = retryDelayMs(attempts);
  cache.set(key, { state: "error", attempts, retryAt: Date.now() + delay });
  notify(key);
}

/** Convenience: kick a load for a single source. Returns a Promise that
 *  resolves when the entry settles (loaded or errored). */
export async function loadAttachment(
  client: { call(method: string, params: unknown): Promise<unknown> },
  deviceId: string,
  path: string,
): Promise<void> {
  if (!beginAttachmentLoad(deviceId, path)) {
    return;
  }
  const image = await readAttachmentImage(client, path);
  if (image === null) {
    storeAttachmentError(deviceId, path);
    return;
  }
  insertLoaded(attachmentCacheKey(deviceId, path), image);
  notify(attachmentCacheKey(deviceId, path));
}

// ---------------------------------------------------------------------------
// Send-wide upload progress (`AppState.begin_upload_progress`,
// state.rs:1160-1186) — one live upload at a time (the composer's `busy`
// gate serializes sends), read by the transcript's sending overlay.
// ---------------------------------------------------------------------------

let uploadProgress: { total: number; done: number } | null = null;
const uploadListeners = new Set<() => void>();

function notifyUpload(): void {
  for (const listener of uploadListeners) {
    listener();
  }
}

/** Begin a send's upload accounting (`begin_upload_progress`). */
export function beginUploadProgress(totalBytes: number): void {
  uploadProgress = { total: Math.max(totalBytes, 1), done: 0 };
  notifyUpload();
}

/** Publish committed bytes (`upload_progress.fetch_add`). Notifies only when
 *  the integer percent moves — the ring labels whole percents, so a percent
 *  of chunk-level notifies would only burn renders. */
export function setUploadProgress(uploadedBytes: number): void {
  if (uploadProgress === null) {
    return;
  }
  const before = percentOf(uploadProgress.done, uploadProgress.total);
  uploadProgress = { total: uploadProgress.total, done: Math.min(uploadedBytes, uploadProgress.total) };
  if (percentOf(uploadProgress.done, uploadProgress.total) !== before) {
    notifyUpload();
  }
}

/** Clear the accounting when the send settles (`end_upload_progress`). */
export function endUploadProgress(): void {
  if (uploadProgress === null) {
    return;
  }
  uploadProgress = null;
  notifyUpload();
}

/**
 * `upload_progress_percent` (state.rs:1177-1186): the whole-send percent as a
 *  0..100 integer, or null when nothing is uploading.
 */
export function uploadProgressPercent(): number | null {
  if (uploadProgress === null) {
    return null;
  }
  return percentOf(uploadProgress.done, uploadProgress.total);
}

function percentOf(done: number, total: number): number {
  return Math.min(Math.max(Math.floor((done / total) * 100), 0), 100);
}

/** React hook over [`uploadProgressPercent`]. */
export function useUploadProgressPercent(): number | null {
  return useSyncExternalStore(
    (listener: () => void) => {
      uploadListeners.add(listener);
      return () => {
        uploadListeners.delete(listener);
      };
    },
    uploadProgressPercent,
    uploadProgressPercent,
  );
}

/** Test-only escape hatch. Clears the module-level cache so each test
 *  starts from a clean slate. */
export function __resetAttachmentCacheForTests(): void {
  cache.clear();
  listeners.clear();
  tick = 0;
  loadedBytes = 0;
  protectedKeys = new Set();
  uploadProgress = null;
  uploadListeners.clear();
}

/** Test-only: the retained byte total (eviction assertions). */
export function __loadedBytesForTests(): number {
  return loadedBytes;
}
