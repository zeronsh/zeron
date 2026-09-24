import type { FileTreeModel } from "../lib/file-tree";
import type { CloseDisposition, FileDocument } from "../lib/file-document";

/**
 * The live `FileDocument` registry — the web peer of the desktop shell's
 * `file_surfaces` map slice (shell.rs:2788-3004): documents outlive their
 * tab's component tree (the pane host mounts one surface at a time, so the
 * registry — not the component — owns the lifetime; ticket 22's
 * per-surface Changes store is the precedent), the pane host asks
 * `prepareClose` before dropping a dirty tab, the tab strip reads the
 * dirty dot, and the page-close guard (the browser's `beforeunload`, the
 * web's `prepare_exit` moment) fires while any document has unflushed
 * edits.
 *
 * Surface ids are minted by the pane store's global monotonic sequence, so
 * one id names one surface for the page's whole life — the registry keys
 * by it alone. The pane store subscribes to dirty changes so its tab rows
 * re-render without either module importing the other's internals.
 */

/** One open file tab's live state: the document plus its tree sidebar. */
export interface FileSurfaceEntry {
  readonly document: FileDocument;
  readonly model: FileTreeModel;
  readonly path: string;
}

export class FileDocumentRegistry {
  readonly #bySurface = new Map<string, FileSurfaceEntry>();
  #dirtyListener: (() => void) | null = null;

  /** The live entry for a surface, or null before its first mount. */
  entryFor(surfaceId: string): FileSurfaceEntry | null {
    return this.#bySurface.get(surfaceId) ?? null;
  }

  documentFor(surfaceId: string): FileDocument | null {
    return this.#bySurface.get(surfaceId)?.document ?? null;
  }

  /**
   * Attach a surface's entry (the FileSurface creates the document and
   * tree on FIRST mount and reuses this entry on every later mount);
   * returns the detach — a UNMOUNT detaches the listener but never
   * disposes, so a tab switch keeps the buffer alive. Disposal belongs to
   * the close (`disposeSurface`).
   */
  attach(surfaceId: string, entry: FileSurfaceEntry): () => void {
    this.#bySurface.set(surfaceId, entry);
    const unsubscribe = entry.document.subscribe(() => this.#dirtyListener?.());
    this.#dirtyListener?.();
    return () => {
      unsubscribe();
    };
  }

  /** The close path's teardown: the surface is really going away. */
  disposeSurface(surfaceId: string): void {
    const entry = this.#bySurface.get(surfaceId);
    if (entry === undefined) {
      return;
    }
    this.#bySurface.delete(surfaceId);
    entry.model.dispose();
    entry.document.dispose();
    this.#dirtyListener?.();
  }

  /** The tab strip's dirty dot: this surface's document has unflushed edits. */
  isDirtyFor(surfaceId: string): boolean {
    return this.#bySurface.get(surfaceId)?.document.hasUnsavedChanges() ?? false;
  }

  /** `all_file_edits_flushed`'s negation — any live document still dirty. */
  hasUnsavedChanges(): boolean {
    for (const entry of this.#bySurface.values()) {
      if (entry.document.hasUnsavedChanges()) {
        return true;
      }
    }
    return false;
  }

  /**
   * `prepare_close` for one surface: null when no live document is
   * registered (the pane closes it without asking — images, unmounted
   * tabs), else the document's own disposition.
   */
  prepareClose(surfaceId: string): CloseDisposition | null {
    const document = this.documentFor(surfaceId);
    if (document === null) {
      return null;
    }
    return document.prepareClose();
  }

  setDirtyListener(listener: (() => void) | null): void {
    this.#dirtyListener = listener;
  }
}

export const fileDocuments = new FileDocumentRegistry();

/**
 * `prepare_exit`'s browser moment: while any open document holds unflushed
 * edits, the page-close gets the browser's own confirm. (The desktop's
 * `reveal_unsaved_file` — routing the user to the dirtiest tab — has no
 * browser equivalent; the dirty dots on the tab strip carry that signal.)
 */
(globalThis as { addEventListener?: (type: string, listener: (event: Event) => void) => void }).addEventListener?.(
  "beforeunload",
  (event) => {
    if (fileDocuments.hasUnsavedChanges()) {
      const beforeUnload = event as BeforeUnloadEvent;
      beforeUnload.preventDefault();
      beforeUnload.returnValue = "";
    }
  },
);
