import type { ExplicitFoldSnapshot } from "../lib/tool-motion";

/**
 * Per-chat fold preference memory (ticket 68, decision option 1): the
 * explicit group/detail pins of the chats this window has visited, kept
 * across chat switches in a bounded in-memory LRU. Presentation state only —
 * nothing here persists to disk, crosses an app/browser restart, or speaks
 * to the engine; the transcript payload and reveal baselines stay owned by
 * the store and the motion store.
 *
 * Keyed by engine identity plus chat/doc id: paired engines can host the
 * same raw doc id, so a cache keyed by the doc alone would collide across
 * them. Detail pins ride the motion store's existing stable identity
 * (`"{rowId}#d{ix}"`), so vanished or reordered rows are harmless key
 * misses. An empty pin set never overwrites an older entry (a visit that
 * pinned nothing must not erase what a previous visit remembered).
 */

/** Chats remembered per engine — the LRU bound (mirrors the desktop's 32). */
export const MAX_SAVED_FOLD_CHATS = 32;

/** One chat's remembered pins, captured with tween clocks stripped. */
export type SavedChatFolds = ExplicitFoldSnapshot;

export class TranscriptFoldCache {
  readonly #engines = new Map<string, Map<string, SavedChatFolds>>();

  /**
   * Remember one chat's explicit pins, refreshing its recency. An empty
   * snapshot is a no-op: "nothing pinned this visit" keeps the older memory
   * (mirrors the viewport cache's empty-rows rule).
   */
  capture(engineKey: string, docId: string, folds: ExplicitFoldSnapshot): void {
    if (folds.groups.size === 0 && folds.details.size === 0) {
      return;
    }
    let chats = this.#engines.get(engineKey);
    if (chats === undefined) {
      chats = new Map();
      this.#engines.set(engineKey, chats);
    }
    // Map insertion order is the LRU order: delete-then-set refreshes.
    chats.delete(docId);
    chats.set(docId, folds);
    while (chats.size > MAX_SAVED_FOLD_CHATS) {
      const oldest = chats.keys().next().value;
      if (oldest === undefined) {
        break;
      }
      chats.delete(oldest);
    }
  }

  /** The chat's saved pins, or null when nothing was remembered. Read-only. */
  restore(engineKey: string, docId: string): SavedChatFolds | null {
    return this.#engines.get(engineKey)?.get(docId) ?? null;
  }

  /** Test seam — drop every remembered chat. */
  clear(): void {
    this.#engines.clear();
  }
}

/**
 * The app's ONE fold preference cache, module scoped like the viewport
 * cache: a transcript surface is created per open chat and destroyed on
 * every switch, so the pins must outlive them.
 */
export const transcriptFoldCache = new TranscriptFoldCache();
