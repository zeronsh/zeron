import type { WorkspaceFileText, WorkspaceReadOnlyReason } from "@zeron/proto";
import type { WorkspaceFilesClient } from "./files-client";
import { describeFilesError } from "./files-client";
import { fileReadOnlyReason, isMarkdownPath, writableEncoding, writableLineEnding } from "./files";

/**
 * One open workspace file: load, edit, save, and the desktop's write-outcome
 * state machine (crates/ui/src/files/document.rs). Saves carry the read
 * snapshot's checkout identity and content hash, so a file that changed on
 * disk since the read answers `conflict` — the buffer is preserved and the
 * phase banner offers a reload, exactly like the desktop.
 */

export type DocumentPhase =
  | { readonly kind: "loading" }
  | { readonly kind: "ready" }
  | { readonly kind: "saving" }
  | { readonly kind: "saveFailed"; readonly message: string }
  | { readonly kind: "conflict"; readonly diskHash: string | null }
  | { readonly kind: "externallyModified"; readonly diskHash: string | null }
  | { readonly kind: "deletedOnDisk" }
  | { readonly kind: "readOnly"; readonly reason: WorkspaceReadOnlyReason }
  | { readonly kind: "error"; readonly message: string };

export interface FileDocumentSnapshot {
  readonly phase: DocumentPhase;
  /** The editor buffer (== the last read text until edited). */
  readonly text: string;
  readonly file: WorkspaceFileText | null;
  /** The document accepts edits (loaded text, writable shape). */
  readonly editable: boolean;
  /** Edits exist that the engine has not acknowledged. */
  readonly dirty: boolean;
  /** Markdown documents: render the preview (`show_markdown`, document.rs). */
  readonly showMarkdown: boolean;
}

/**
 * `FilesCloseDisposition` (mod.rs:152) — what a close request resolved to.
 * "allow" closes now; "pending" waits for in-flight autosaves to land;
 * "blocked" shows the Retry/Keep Open/Discard banner.
 */
export type CloseDisposition = "allow" | "pending" | "blocked";

interface PendingSave {
  readonly revision: number;
  readonly text: string;
  readonly expectedContentHash: string;
  readonly expectedCheckoutId: string;
}

export class FileDocument {
  readonly #client: WorkspaceFilesClient;
  readonly path: string;
  readonly #listeners = new Set<() => void>();

  #phase: DocumentPhase = { kind: "loading" };
  #file: WorkspaceFileText | null = null;
  #text = "";
  #generation = 0;
  #revision = 0;
  #savedRevision = 0;
  #savedHash: string | null = null;
  #pendingSave: PendingSave | null = null;
  #snapshot: FileDocumentSnapshot;
  #disposed = false;
  #showMarkdown: boolean;
  #autosaveEnabled = false;
  #autosaveDelayMs: number;
  #autosavePaused = false;
  #autosaveTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(client: WorkspaceFilesClient, path: string, options: { autosaveDelayMs?: number } = {}) {
    this.#client = client;
    this.path = path;
    // document.rs `FileDocument::loading` — markdown starts in preview mode.
    this.#showMarkdown = isMarkdownPath(path);
    this.#autosaveDelayMs = options.autosaveDelayMs ?? 900;
    this.#snapshot = this.#takeSnapshot();
  }

  getSnapshot(): FileDocumentSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  dispose(): void {
    this.#disposed = true;
    this.#generation += 1;
    this.#clearAutosaveTimer();
    this.#listeners.clear();
  }

  /** (Re)read the file from the engine; the fresh snapshot replaces the buffer. */
  load(): void {
    const generation = ++this.#generation;
    this.#phase = { kind: "loading" };
    this.#pendingSave = null;
    this.#clearAutosaveTimer();
    this.#commit();
    void this.#client
      .readFile(this.path)
      .then((file) => {
        if (this.#accepts(generation)) {
          this.#setLoaded(file);
        }
      })
      .catch((error: unknown) => {
        if (this.#accepts(generation)) {
          this.#phase = { kind: "error", message: describeFilesError(error) };
          this.#commit();
        }
      });
  }

  /** Desktop `mark_user_edit`: dirty the buffer; a failed save retries cleanly. */
  edit(text: string): void {
    if (!this.isEditable()) {
      return;
    }
    this.#text = text;
    this.#revision += 1;
    if (this.#phase.kind === "saveFailed") {
      this.#phase = { kind: "ready" };
    }
    this.#commit();
    // desktop on_editor_change → schedule_autosave: idle edits earn a save.
    this.#scheduleAutosave();
  }

  /** `show_markdown` — the markdown/code presentation toggle. */
  setShowMarkdown(show: boolean): void {
    if (this.#showMarkdown === show) {
      return;
    }
    this.#showMarkdown = show;
    this.#commit();
  }

  /** Desktop `begin_save` + the write RPC; outcomes land as phases. */
  save(): void {
    if (!this.canSave()) {
      return;
    }
    const pending: PendingSave = {
      revision: this.#revision,
      text: this.#text,
      expectedContentHash: this.#savedHash!,
      expectedCheckoutId: this.#file!.checkoutId,
    };
    const file = this.#file!;
    this.#pendingSave = pending;
    this.#phase = { kind: "saving" };
    this.#commit();
    void this.#client
      .writeFile({
        expectedCheckoutId: pending.expectedCheckoutId,
        path: this.path,
        text: pending.text,
        expectedContentHash: pending.expectedContentHash,
        encoding: writableEncoding(file.encoding)!,
        lineEnding: writableLineEnding(file.lineEnding)!,
      })
      .then((outcome) => {
        if (this.#pendingSave?.revision !== pending.revision) {
          return;
        }
        this.#clearAutosaveTimer();
        if (outcome.status === "written") {
          this.#savedHash = outcome.file.contentHash;
          this.#savedRevision = pending.revision;
          this.#pendingSave = null;
          this.#phase = { kind: "ready" };
        } else {
          this.#pendingSave = null;
          this.#phase = { kind: "conflict", diskHash: outcome.currentContentHash ?? null };
        }
        this.#commit();
      })
      .catch((error: unknown) => {
        if (this.#pendingSave?.revision !== pending.revision) {
          return;
        }
        this.#clearAutosaveTimer();
        this.#pendingSave = null;
        this.#phase = { kind: "saveFailed", message: describeFilesError(error) };
        this.#commit();
      });
  }

  /**
   * Reload from disk, discarding the buffer (the desktop's explicit
   * discard-and-reload affordance behind the conflict/changed banners).
   */
  reloadFromDisk(): void {
    this.load();
  }

  /**
   * `keep_external_edits` (preview.rs:2043): dismiss the changed-on-disk
   * banner by converting `externallyModified` into `conflict` — the buffer
   * stays, and a later save stays blocked until an explicit reload.
   */
  keepEditing(): void {
    if (this.#phase.kind === "externallyModified") {
      this.#clearAutosaveTimer();
      this.#phase = { kind: "conflict", diskHash: this.#phase.diskHash };
      this.#commit();
    }
  }

  /** Desktop `discard_changes` (document.rs:300): resolve dirty state. */
  discardChanges(): void {
    this.#clearAutosaveTimer();
    this.#pendingSave = null;
    this.#savedRevision = this.#revision;
    this.#commit();
  }

  /**
   * `prepare_close` (preview.rs:1629) for one document: clean ⇒ allow;
   * a dirty doc that can autosave saves now and pends; a dirty doc stuck
   * in a phase that cannot autosave blocks (Retry/Keep Open/Discard).
   */
  prepareClose(): CloseDisposition {
    if (!this.isDirty()) {
      return "allow";
    }
    if (this.blocksLifecycleClose()) {
      return "blocked";
    }
    if (this.canAutosave()) {
      this.save();
    }
    return "pending";
  }

  /** `document_blocks_lifecycle` (preview.rs:506). */
  blocksLifecycleClose(): boolean {
    return (
      this.isDirty() &&
      (this.#phase.kind === "saveFailed" ||
        this.#phase.kind === "conflict" ||
        this.#phase.kind === "externallyModified" ||
        this.#phase.kind === "deletedOnDisk")
    );
  }

  /** Desktop `can_autosave` (document.rs:182). */
  canAutosave(): boolean {
    return this.canSave() && this.#phase.kind === "ready";
  }

  /**
   * `set_autosave_enabled` / `set_autosave_delay_ms` (preview.rs:342-366):
   * reconfigure, drop any pending timer, and re-arm a capable document.
   */
  configureAutosave(enabled: boolean, delayMs: number): void {
    this.#autosaveEnabled = enabled;
    this.#autosaveDelayMs = delayMs;
    this.#clearAutosaveTimer();
    this.#scheduleAutosave();
  }

  /**
   * `autosave_paused_for_reload`: a pending destructive-reload confirmation
   * holds autosave back so it cannot race the user's choice; canceling the
   * confirmation re-arms a capable document (preview.rs:2028-2040).
   */
  setAutosavePaused(paused: boolean): void {
    this.#autosavePaused = paused;
    if (paused) {
      this.#clearAutosaveTimer();
    } else {
      this.#scheduleAutosave();
    }
  }

  /** `has_unsaved_changes` for this document. */
  hasUnsavedChanges(): boolean {
    return this.isDirty();
  }

  #scheduleAutosave(): void {
    if (this.#disposed || !this.#autosaveEnabled || this.#autosavePaused || !this.canAutosave()) {
      return;
    }
    this.#clearAutosaveTimer();
    const revision = this.#revision;
    const generation = this.#generation;
    this.#autosaveTimer = setTimeout(() => {
      this.#autosaveTimer = null;
      // desktop: still enabled, unpaused, same generation and revision, and
      // still capable — every edit reschedules, so a stale timer never fires.
      if (
        !this.#disposed &&
        this.#autosaveEnabled &&
        !this.#autosavePaused &&
        this.#generation === generation &&
        this.#revision === revision &&
        this.canAutosave()
      ) {
        this.save();
      }
    }, this.#autosaveDelayMs);
  }

  #clearAutosaveTimer(): void {
    if (this.#autosaveTimer !== null) {
      clearTimeout(this.#autosaveTimer);
      this.#autosaveTimer = null;
    }
  }

  /**
   * Watch reconcile (desktop `reconcile_document`): re-read and compare the
   * on-disk hash — a clean document silently picks up the new disk state; a
   * dirty one keeps its buffer and surfaces "changed on disk".
   */
  reconcile(): void {
    if (this.#phase.kind === "loading" || this.#phase.kind === "saving" || this.#disposed) {
      return;
    }
    const generation = this.#generation;
    void this.#client
      .readFile(this.path)
      .then((file) => {
        if (this.#disposed || this.#generation !== generation) {
          return;
        }
        const diskHash = file.contentHash ?? null;
        if (diskHash !== null && diskHash === this.#savedHash) {
          return;
        }
        if (this.isDirty()) {
          if (this.#phase.kind === "ready" || this.#phase.kind === "saveFailed") {
            // desktop mark_external: the pending autosave is dropped.
            this.#clearAutosaveTimer();
            this.#phase = { kind: "externallyModified", diskHash };
            this.#commit();
          }
          return;
        }
        this.#setLoaded(file);
      })
      .catch(() => {
        // A reconcile read rides the next change frame if it failed.
      });
  }

  /** Watch: the open file was removed on disk (buffer preserved). */
  markDeleted(): void {
    if (this.#disposed) {
      return;
    }
    this.#pendingSave = null;
    // desktop mark_deleted: autosave and any save in flight stop.
    this.#clearAutosaveTimer();
    this.#phase = { kind: "deletedOnDisk" };
    this.#commit();
  }

  /** Watch: a path matching this document reappeared; restore it from disk. */
  restore(): void {
    if (this.#phase.kind === "deletedOnDisk") {
      this.load();
    }
  }

  isDirty(): boolean {
    return this.#revision !== this.#savedRevision;
  }

  isEditable(): boolean {
    return (
      this.#phase.kind !== "loading" &&
      this.#phase.kind !== "readOnly" &&
      this.#phase.kind !== "error" &&
      this.#file !== null &&
      this.#file.text !== null &&
      this.#file.text !== undefined &&
      this.#savedHash !== null &&
      writableEncoding(this.#file.encoding) !== null &&
      writableLineEnding(this.#file.lineEnding) !== null
    );
  }

  /** Desktop `can_save`: editable, dirty, idle phase, no save in flight. */
  canSave(): boolean {
    return (
      this.isEditable() &&
      this.isDirty() &&
      this.#pendingSave === null &&
      (this.#phase.kind === "ready" || this.#phase.kind === "saveFailed")
    );
  }

  #accepts(generation: number): boolean {
    return !this.#disposed && generation === this.#generation;
  }

  /** Desktop `set_loaded`: fresh disk state, clean buffer, read-only derived. */
  #setLoaded(file: WorkspaceFileText): void {
    const readOnly = fileReadOnlyReason(file);
    this.#file = file;
    this.#text = file.text ?? "";
    this.#savedHash = file.contentHash ?? null;
    this.#pendingSave = null;
    this.#phase = readOnly !== null ? { kind: "readOnly", reason: readOnly } : { kind: "ready" };
    // A programmatic reload lands clean: the saved revision meets the current
    // one, so reloaded content is never dirty (desktop apply_external_reload).
    this.#savedRevision = this.#revision;
    this.#commit();
  }

  #takeSnapshot(): FileDocumentSnapshot {
    return {
      phase: this.#phase,
      text: this.#text,
      file: this.#file,
      editable: this.isEditable(),
      dirty: this.isDirty(),
      showMarkdown: this.#showMarkdown,
    };
  }

  #commit(): void {
    this.#snapshot = this.#takeSnapshot();
    for (const listener of this.#listeners) {
      listener();
    }
  }
}
