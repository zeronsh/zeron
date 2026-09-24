/**
 * The new-thread background image's durable home. The desktop copies the
 * chosen file into `{data_dir}/new-thread-backgrounds/` and points the
 * settings field at that path (settings.rs:305-362); a browser has no
 * filesystem, so the managed copy is an IndexedDB blob under one fixed key
 * and the settings field's `path` is that key (`idb:new-thread-composer-
 * background`) — the ticket's "object URL / IndexedDB blob" substitute.
 *
 * `url()` hands out a session-scoped object URL (a stable string the <img>
 * can consume), revoking the previous URL whenever the blob is replaced or
 * removed — the desktop's managed-file retirement.
 */

const DB_NAME = "zeron-settings";
const DB_VERSION = 1;
const STORE = "blobs";
export const NEW_THREAD_BACKGROUND_KEY = "new-thread-composer-background";

export interface BackgroundBlobStore {
  /** Overwrite the managed blob; any previous URL is retired. */
  put(blob: Blob): Promise<void>;
  /** The blob's object URL, or null when nothing (or only a broken entry) is stored. */
  url(): Promise<string | null>;
  /** Retire the blob and its URL. */
  delete(): Promise<void>;
}

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE)) {
        request.result.createObjectStore(STORE);
      }
    };
    request.onsuccess = () => {
      resolve(request.result);
    };
    request.onerror = () => {
      reject(request.error ?? new Error("could not open the settings database"));
    };
  });
}

function requestToPromise<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => {
      resolve(request.result);
    };
    request.onerror = () => {
      reject(request.error ?? new Error("the settings database request failed"));
    };
  });
}

/**
 * The ONE process-wide store (ticket 35, gap G19): every background
 * resolution shares a single `cachedUrl`, so the object URL minted for a
 * blob revision — hence the artwork's identity — is stable across every
 * mount, and a put/delete retires it exactly once. Before ticket 35 each
 * call returned a fresh closure with its own cache, so every resolve minted
 * a NEW blob: URL nobody ever revoked: the artwork's id changed per mount,
 * the keyed readiness wrapper remounted, and the 120 ms fade replayed.
 */
export function idbBackgroundBlobStore(): BackgroundBlobStore {
  if (singletonBlobStore === null) {
    singletonBlobStore =
      typeof indexedDB === "undefined" ? memoryBackgroundBlobStore() : createIdbBackgroundBlobStore();
  }
  return singletonBlobStore;
}

let singletonBlobStore: BackgroundBlobStore | null = null;

/** The real store; a memory stand-in when IndexedDB is unavailable (tests). */
function createIdbBackgroundBlobStore(): BackgroundBlobStore {
  let cachedUrl: string | null = null;
  const revoke = (): void => {
    if (cachedUrl !== null) {
      URL.revokeObjectURL(cachedUrl);
      cachedUrl = null;
    }
  };
  return {
    async put(blob: Blob): Promise<void> {
      const database = await openDatabase();
      try {
        await requestToPromise(database.transaction(STORE, "readwrite").objectStore(STORE).put(blob, NEW_THREAD_BACKGROUND_KEY));
      } finally {
        database.close();
      }
      revoke();
    },
    async url(): Promise<string | null> {
      const database = await openDatabase();
      let blob: unknown;
      try {
        blob = await requestToPromise(database.transaction(STORE, "readonly").objectStore(STORE).get(NEW_THREAD_BACKGROUND_KEY));
      } finally {
        database.close();
      }
      if (!(blob instanceof Blob)) {
        revoke();
        return null;
      }
      if (cachedUrl === null) {
        cachedUrl = URL.createObjectURL(blob);
      }
      return cachedUrl;
    },
    async delete(): Promise<void> {
      const database = await openDatabase();
      try {
        await requestToPromise(database.transaction(STORE, "readwrite").objectStore(STORE).delete(NEW_THREAD_BACKGROUND_KEY));
      } finally {
        database.close();
      }
      revoke();
    },
  };
}

/** In-memory substitute: same contract, for tests and storage-less runtimes. */
export function memoryBackgroundBlobStore(): BackgroundBlobStore & { snapshot(): Blob | null } {
  let blob: Blob | null = null;
  let cachedUrl: string | null = null;
  return {
    async put(next: Blob): Promise<void> {
      if (cachedUrl !== null) {
        URL.revokeObjectURL(cachedUrl);
        cachedUrl = null;
      }
      blob = next;
    },
    async url(): Promise<string | null> {
      if (blob === null) {
        return null;
      }
      if (cachedUrl === null) {
        cachedUrl = `blob:memory-${blob.size}`;
      }
      return cachedUrl;
    },
    async delete(): Promise<void> {
      if (cachedUrl !== null) {
        URL.revokeObjectURL(cachedUrl);
        cachedUrl = null;
      }
      blob = null;
    },
    snapshot(): Blob | null {
      return blob;
    },
  };
}
