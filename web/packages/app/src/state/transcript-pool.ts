import { TranscriptStore, type TranscriptCache, type TranscriptClient } from "./transcript-store";

/** Keep recent chats current so returning to them needs no new reset roundtrip. */
export const WARM_TRANSCRIPT_LIMIT = 5;

export class TranscriptPool {
  readonly #stores = new Map<string, TranscriptStore>();
  #disposed = false;

  constructor(readonly client: TranscriptClient) {}

  get(docId: string, cache?: TranscriptCache): TranscriptStore {
    if (this.#disposed) {
      throw new Error("transcript pool is disposed");
    }
    const existing = this.#stores.get(docId);
    if (existing !== undefined) {
      this.#stores.delete(docId);
      this.#stores.set(docId, existing);
      return existing;
    }
    const store = new TranscriptStore(this.client, docId, { cache });
    this.#stores.set(docId, store);
    if (this.#stores.size > WARM_TRANSCRIPT_LIMIT) {
      const oldest = this.#stores.keys().next().value!;
      this.#stores.get(oldest)!.dispose();
      this.#stores.delete(oldest);
    }
    return store;
  }

  dispose(): void {
    this.#disposed = true;
    for (const store of this.#stores.values()) store.dispose();
    this.#stores.clear();
  }
}
