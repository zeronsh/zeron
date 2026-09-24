import type { Chat, Device, Session, Space } from "@zeron/proto";
import type { SessionMessageEntry } from "@zeron/proto";

/**
 * The disposable client-side engine cache — the port of `engine_cache.rs`
 * (`crates/ui/src/engine_cache.rs`). Last-known row sets per paired engine
 * plus per-chat transcripts for chats the user has actually opened, seeded
 * at registry startup so the sidebar renders while connections dial, and
 * re-read whenever an engine is reconnecting or off. A partial or obsolete
 * entry is a miss — engine data stays authoritative.
 *
 * Storage is IndexedDB, not `localStorage`: row sets and transcripts are
 * large and written often, which the synchronous `localStorage` API and its
 * small quota handle poorly. Granularity mirrors the desktop's
 * `data_dir/engine-cache-v1/<sha256(engineKey)>/{rows.json,
 * chat-<sha256(chatId)>.json}`: one row-set entry per engine, one
 * transcript entry per chat. Hashing the key is unnecessary here — IndexedDB
 * keys are not filesystem paths — but the per-engine/per-chat split is kept.
 * Credentials never enter this store.
 */

/** The four row collections persisted per engine (`engine_cache.rs::CachedRows`). */
export interface CachedRows {
  readonly chats: readonly Chat[];
  readonly spaces: readonly Space[];
  readonly devices: readonly Device[];
  readonly sessions: readonly Session[];
}

/** The cache surface the registry drives. */
export interface EngineCacheStore {
  /** Last-known rows for one engine, or null when nothing usable is stored. */
  loadRows(engineKey: string): Promise<CachedRows | null>;
  /** Overwrite the engine's row set atomically (one transaction). */
  saveRows(engineKey: string, rows: CachedRows): Promise<void>;
  /** One chat's cached transcript, or null when never opened. */
  loadTranscript(engineKey: string, chatId: string): Promise<readonly SessionMessageEntry[] | null>;
  /** Overwrite one chat's transcript atomically. */
  saveTranscript(engineKey: string, chatId: string, entries: readonly SessionMessageEntry[]): Promise<void>;
  /** Delete every stored entry for the engine; absent entries are not an error. */
  forgetEngine(engineKey: string): Promise<void>;
}

const DB_NAME = "zeron-engine-cache";
const DB_VERSION = 1;
const ROWS_STORE = "rows";
const TRANSCRIPTS_STORE = "transcripts";

/**
 * IndexedDB-backed cache. The database opens lazily on first use so
 * constructing the singleton has no side effects; in a non-browser runtime
 * (tests, SSR) every operation no-ops — the cache is disposable by design.
 */
export class IndexedDbEngineCache implements EngineCacheStore {
  readonly #database: Promise<IDBDatabase | null>;

  constructor() {
    this.#database = openEngineCache();
  }

  async loadRows(engineKey: string): Promise<CachedRows | null> {
    const db = await this.#database;
    if (db === null) {
      return null;
    }
    return (await request(db, "readonly", ROWS_STORE, (store) => store.get(engineKey))) as CachedRows | null;
  }

  async saveRows(engineKey: string, rows: CachedRows): Promise<void> {
    const db = await this.#database;
    if (db === null) {
      return;
    }
    await request(db, "readwrite", ROWS_STORE, (store) => store.put(rows, engineKey));
  }

  async loadTranscript(engineKey: string, chatId: string): Promise<readonly SessionMessageEntry[] | null> {
    const db = await this.#database;
    if (db === null) {
      return null;
    }
    return (await request(db, "readonly", TRANSCRIPTS_STORE, (store) =>
      store.get([engineKey, chatId]),
    )) as readonly SessionMessageEntry[] | null;
  }

  async saveTranscript(engineKey: string, chatId: string, entries: readonly SessionMessageEntry[]): Promise<void> {
    const db = await this.#database;
    if (db === null) {
      return;
    }
    await request(db, "readwrite", TRANSCRIPTS_STORE, (store) => store.put([...entries], [engineKey, chatId]));
  }

  async forgetEngine(engineKey: string): Promise<void> {
    const db = await this.#database;
    if (db === null) {
      return;
    }
    // Tolerate already-absent entries: delete() on a missing key succeeds.
    await request(db, "readwrite", ROWS_STORE, (store) => store.delete(engineKey));
    // One chat at a time: IndexedDB has no prefix delete over tuple keys.
    const keys = await request(db, "readonly", TRANSCRIPTS_STORE, (store) => {
      const range = IDBKeyRange.bound([engineKey], [engineKey, "\uffff"]);
      return store.getAllKeys(range) as unknown;
    });
    if (Array.isArray(keys) && keys.length > 0) {
      await request(db, "readwrite", TRANSCRIPTS_STORE, (store) => {
        for (const key of keys) {
          store.delete(key as IDBValidKey);
        }
        return undefined;
      });
    }
  }
}

async function openEngineCache(): Promise<IDBDatabase | null> {
  const indexedDB = (globalThis as { indexedDB?: typeof globalThis.indexedDB }).indexedDB;
  if (indexedDB === undefined) {
    return null;
  }
  return new Promise<IDBDatabase | null>((resolve, reject) => {
    const open = indexedDB.open(DB_NAME, DB_VERSION);
    open.onupgradeneeded = () => {
      const db = open.result;
      if (!db.objectStoreNames.contains(ROWS_STORE)) {
        db.createObjectStore(ROWS_STORE);
      }
      if (!db.objectStoreNames.contains(TRANSCRIPTS_STORE)) {
        db.createObjectStore(TRANSCRIPTS_STORE);
      }
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  }).catch(() => null);
}

function request(
  db: IDBDatabase,
  mode: IDBTransactionMode,
  storeName: string,
  operate: (store: IDBObjectStore) => unknown,
): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const transaction = db.transaction(storeName, mode);
    const request_ = operate(transaction.objectStore(storeName));
    transaction.onabort = () => reject(transaction.error);
    transaction.onerror = () => reject(transaction.error);
    transaction.oncomplete = () => {
      resolve(request_ instanceof IDBRequest ? request_.result : undefined);
    };
  });
}

/** In-memory cache for tests and non-browser runtimes. */
export class MemoryEngineCache implements EngineCacheStore {
  readonly #rows = new Map<string, CachedRows>();
  readonly #transcripts = new Map<string, readonly SessionMessageEntry[]>();
  readonly #forgotten: string[] = [];

  async loadRows(engineKey: string): Promise<CachedRows | null> {
    return this.#rows.get(engineKey) ?? null;
  }

  async saveRows(engineKey: string, rows: CachedRows): Promise<void> {
    this.#rows.set(engineKey, rows);
  }

  async loadTranscript(engineKey: string, chatId: string): Promise<readonly SessionMessageEntry[] | null> {
    return this.#transcripts.get(transcriptKey(engineKey, chatId)) ?? null;
  }

  async saveTranscript(engineKey: string, chatId: string, entries: readonly SessionMessageEntry[]): Promise<void> {
    this.#transcripts.set(transcriptKey(engineKey, chatId), [...entries]);
  }

  async forgetEngine(engineKey: string): Promise<void> {
    this.#forgotten.push(engineKey);
    this.#rows.delete(engineKey);
    for (const key of [...this.#transcripts.keys()]) {
      if (key.startsWith(`${engineKey}\u0000`)) {
        this.#transcripts.delete(key);
      }
    }
  }

  /** Test assertion helper — the engines whose cache was forgotten. */
  forgotten(): readonly string[] {
    return this.#forgotten;
  }

  /** Test assertion helper — whether any rows are stored for an engine. */
  hasRows(engineKey: string): boolean {
    return this.#rows.has(engineKey);
  }

  /** Test assertion helper — whether a transcript is stored. */
  hasTranscript(engineKey: string, chatId: string): boolean {
    return this.#transcripts.has(transcriptKey(engineKey, chatId));
  }
}

function transcriptKey(engineKey: string, chatId: string): string {
  return `${engineKey}\u0000${chatId}`;
}
