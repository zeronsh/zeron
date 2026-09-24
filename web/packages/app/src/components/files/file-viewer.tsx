import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Icon } from "@zeron/icons";
import { FileDocument, type FileDocumentSnapshot } from "../../lib/file-document";
import { WorkspaceFilesClient } from "../../lib/files-client";
import { fileName, isImagePath, isMarkdownPath, readOnlyMessage, truncatedMessage } from "../../lib/files";
import { FileTreeModel } from "../../lib/file-tree";
import { clipMarkdownBytes, parseMarkdown, type TaskMarker } from "../../lib/markdown-doc";
import { useResolvedAppearance } from "../../state/appearance";
import { fileDocuments, type FileSurfaceEntry } from "../../state/file-documents";
import { reviewCommentStore, useReviewComments } from "../../state/review-comments";
import { rightPaneStore } from "../../state/right-pane";
import { onShortcut } from "../../state/shortcuts";
import { uiSettings, useUiSettings } from "../../state/ui-settings";
import { useEngineSession } from "../../state/session-provider";
import { Tooltip, TOOLTIP_VIEW_OPTIONS_MS } from "../ui/Tooltip";
import { CodeView, type CodeReviewWiring } from "./code-view";
import { FileIcon } from "./file-icon";
import { EditorContextMenu } from "./editor-context-menu";
import { ImageView, loadWorkspaceImage, type WorkspaceImageLoad } from "./image-view";
import { MarkdownView } from "./markdown-view";

/**
 * The right pane's File surface body — the web peer of the desktop's
 * promoted editor presentation (crates/ui/src/files/preview.rs): the
 * breadcrumb toolbar, the write-outcome banners, and the document body
 * (code view, markdown preview, or image). The tree sidebar split is gone
 * (07aaa418): the explorer is a docked portion of the pane and reveals go
 * through it. One FileDocument per tab, driven by its own chat-scoped
 * watch; Mod-S saves through the shortcut bus; a close of a dirty tab goes
 * through `prepareClose` (allow / pending / blocked) with the
 * Retry/Keep Open/Discard banner instead of discarding edits.
 */

export function FileSurface({ chatId, surfaceId }: { chatId: string; surfaceId: string }) {
  const session = useEngineSession();
  // The surface's path moves with the tab (renames); read it per store
  // version so the body follows.
  useSyncExternalStore(rightPaneStore.subscribe, rightPaneStore.getVersion);
  const path = rightPaneStore.filePathOf(surfaceId);

  const client = useMemo(
    () => (session !== null && path !== null ? new WorkspaceFilesClient(session.client, { chatId }) : null),
    [session, chatId, path],
  );

  // The surface's live entry — document plus tree sidebar — lives in the
  // registry, OUTLIVING this component (the pane host mounts one surface at
  // a time; a tab switch must never discard the buffer — the desktop's
  // `file_surfaces` entities stay alive the same way). First mount creates
  // and loads; later mounts reuse.
  const [entry, setEntry] = useState<FileSurfaceEntry | null>(null);
  useEffect(() => {
    if (client === null || session === null || path === null || isImagePath(path)) {
      setEntry(null);
      return;
    }
    const existing = fileDocuments.entryFor(surfaceId);
    if (existing !== null && existing.path === path) {
      setEntry(existing);
      return;
    }
    const document = new FileDocument(client, path, {
      autosaveDelayMs: uiSettings.getSnapshot().filesAutosaveDelayMs,
    });
    document.load();
    const model = new FileTreeModel({
      client,
      watch: (handlers) => client.watchFiles(session.client, handlers),
      includeIgnored: uiSettings.getSnapshot().filesShowAll,
      onFileEvent: (event) => {
        if (document.path !== path) {
          return;
        }
        if (event.kind === "modified") {
          document.reconcile();
        } else if (event.kind === "removed") {
          document.markDeleted();
        } else if (event.kind === "created") {
          document.restore();
        }
        // `renamed` handling is not wired on web yet — the tab title keeps
        // the old path until the surface reopens (see ticket Comments).
      },
    });
    model.start();
    const created: FileSurfaceEntry = { document, model, path };
    setEntry(created);
    const detach = fileDocuments.attach(surfaceId, created);
    // Unmount only detaches the listener — the entry (and the buffer)
    // survives until the tab actually closes (`disposeSurface`).
    return () => {
      detach();
    };
  }, [client, session, path, surfaceId]);

  if (path === null) {
    return (
      <div className="files-viewer">
        <p className="files-note">Loading file…</p>
      </div>
    );
  }

  // Images fetch through their own RPC path and never mint a document.
  const isImage = isImagePath(path);

  return (
    <div className="files-viewer-pane">
      {isImage ? (
        <ImageViewer
          chatId={chatId}
          client={client}
          path={path}
        />
      ) : (
        <TextViewer
          doc={entry?.document ?? null}
          path={path}
          chatId={chatId}
          surfaceId={surfaceId}
          client={client}
        />
      )}
    </div>
  );
}

// ── The breadcrumb toolbar (render_breadcrumb, preview.rs:2281-2520) ──────

function ViewerToolbar({
  path,
  markdown,
  showingMarkdown,
  snapshot,
  onToggleMarkdown,
  onReveal,
  onToggleWordWrap,
  wordWrap,
  onRetrySave,
}: {
  readonly path: string;
  readonly markdown: boolean;
  readonly showingMarkdown: boolean;
  readonly snapshot: FileDocumentSnapshot | null;
  readonly onToggleMarkdown: (() => void) | null;
  readonly onReveal: () => void;
  readonly onToggleWordWrap: () => void;
  readonly wordWrap: boolean;
  readonly onRetrySave: (() => void) | null;
}) {
  const appearance = useResolvedAppearance();
  const parts = path.split("/");
  return (
    <div className="files-breadcrumb-bar">
      <FileIcon kind="file" name={path} appearance={appearance} size={14} className="files-breadcrumb-icon" />
      <Tooltip
        label={path}
        delay={TOOLTIP_VIEW_OPTIONS_MS}
        trigger={
          <div className="files-breadcrumb">
            {parts.map((part, index) => (
              <span key={index} className="files-crumb">
                {index > 0 && <span className="files-crumb-sep">›</span>}
                <span className={index + 1 === parts.length ? "files-crumb-final" : "files-crumb-part"}>{part}</span>
              </span>
            ))}
          </div>
        }
      />
      {markdown && onToggleMarkdown !== null && (
        <Tooltip
          label={showingMarkdown ? "Show Markdown code" : "Preview Markdown"}
          delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <button
              type="button"
              className={`files-toolbar-button${showingMarkdown ? " files-toolbar-button-on" : ""}`}
              aria-pressed={showingMarkdown}
              aria-label={showingMarkdown ? "Show Markdown code" : "Preview Markdown"}
              onClick={onToggleMarkdown}
            >
              <Icon name={showingMarkdown ? "fileCode" : "eye"} size={14} />
            </button>
          }
        />
      )}
      {snapshot !== null && <SaveStatusPill snapshot={snapshot} onRetry={onRetrySave} />}
      <Tooltip
        label="Reveal file in tree"
        delay={TOOLTIP_VIEW_OPTIONS_MS}
        trigger={
          <button
            type="button"
            className="files-toolbar-button"
            aria-label="Reveal file in tree"
            onClick={onReveal}
          >
            <Icon name="folder" size={14} />
          </button>
        }
      />
      <Tooltip
        label={wordWrap ? "Disable word wrap" : "Enable word wrap"}
        delay={TOOLTIP_VIEW_OPTIONS_MS}
        trigger={
          <button
            type="button"
            className={`files-toolbar-button${wordWrap ? " files-toolbar-button-on" : ""}`}
            aria-pressed={wordWrap}
            aria-label={wordWrap ? "Disable word wrap" : "Enable word wrap"}
            onClick={onToggleWordWrap}
          >
            <Icon name="list" size={14} />
          </button>
        }
      />
    </div>
  );
}

/**
 * The save-status pill (`files-save-status`, preview.rs:2380-2428):
 * phase-colored, `saveFailed` clickable as the retry, tooltips after 350ms.
 */
function SaveStatusPill({
  snapshot,
  onRetry,
}: {
  readonly snapshot: FileDocumentSnapshot;
  readonly onRetry: (() => void) | null;
}) {
  switch (snapshot.phase.kind) {
    case "saveFailed":
      return (
        <Tooltip label={snapshot.phase.message} delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <button
              type="button"
              className="files-save-pill files-save-pill-danger"
              aria-label="Save file"
              onClick={() => onRetry?.()}
            >
              Save failed
            </button>
          }
        />
      );
    case "conflict":
      return (
        <Tooltip label="The file changed on disk. Your editor buffer was preserved." delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <span className="files-save-pill files-save-pill-warning" role="status">Save conflict</span>
          }
        />
      );
    case "deletedOnDisk":
      return (
        <Tooltip label="The file was removed on disk. Your editor buffer was preserved." delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <span className="files-save-pill files-save-pill-warning" role="status">Deleted on disk</span>
          }
        />
      );
    case "externallyModified":
      return (
        <Tooltip label="The file changed on disk. Review it before saving." delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <span className="files-save-pill files-save-pill-warning" role="status">Changed on disk</span>
          }
        />
      );
    default:
      return null;
  }
}

// ── Text and markdown ───────────────────────────────────────────────────

function TextViewer({
  doc,
  path,
  chatId,
  surfaceId,
  client,
}: {
  readonly doc: FileDocument | null;
  readonly path: string;
  readonly chatId: string;
  readonly surfaceId: string;
  readonly client: WorkspaceFilesClient | null;
}) {
  const settings = useUiSettings();
  const subscribe = useCallback(
    (listener: () => void) => (doc === null ? () => {} : doc.subscribe(listener)),
    [doc],
  );
  const getSnapshot = useCallback(
    () => doc?.getSnapshot() ?? { phase: { kind: "loading" as const }, text: "", file: null, editable: false, dirty: false, showMarkdown: isMarkdownPath(path) },
    [doc, path],
  );
  const snapshot = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const markdown = isMarkdownPath(path);
  const [confirmingReload, setConfirmingReload] = useState(false);
  const [markdownFocus, setMarkdownFocus] = useState(false);

  // Autosave configuration follows the settings store (desktop
  // set_autosave_enabled / set_autosave_delay_ms fan-out).
  useEffect(() => {
    doc?.configureAutosave(settings.filesAutosaveEnabled, settings.filesAutosaveDelayMs);
  }, [doc, settings.filesAutosaveEnabled, settings.filesAutosaveDelayMs]);

  // A pending destructive-reload confirmation pauses autosave so it cannot
  // race the user's choice (preview.rs request_reload).
  useEffect(() => {
    doc?.setAutosavePaused(confirmingReload);
  }, [doc, confirmingReload]);

  // Mod-S saves through the shortcut bus (`state/shortcuts.ts` — the
  // app-shell guards it to a chat route with the pane on a Files/File
  // surface), never a component-local keydown listener.
  useEffect(
    () =>
      onShortcut("save-file", () => {
        doc?.save();
      }),
    [doc],
  );

  // The pane host's close request: complete once the saves land.
  const surface = useMemo<{ kind: "file"; id: string }>(() => ({ kind: "file", id: surfaceId }), [surfaceId]);
  const closeRequested = rightPaneStore.isCloseRequested(surface);
  useEffect(() => {
    if (closeRequested && !snapshot.dirty) {
      rightPaneStore.completeFileClose(chatId, surface);
    }
  }, [closeRequested, snapshot.dirty, chatId, surface]);

  const markdownClip = useMemo(
    () => clipMarkdownBytes(snapshot.text),
    [snapshot.text],
  );
  const markdownBlocks = useMemo(
    () => (markdown && snapshot.showMarkdown ? parseMarkdown(markdownClip.text) : null),
    [markdown, snapshot.showMarkdown, markdownClip.text],
  );

  const checkoutId = snapshot.file?.checkoutId ?? null;
  const loadImage = useCallback(
    (imagePath: string): Promise<WorkspaceImageLoad> => {
      if (client === null || checkoutId === null) {
        return Promise.reject(new Error("Workspace image connection unavailable"));
      }
      return loadWorkspaceImage(client, imagePath);
    },
    [client, checkoutId],
  );

  const onToggleTask = useCallback(
    (marker: TaskMarker, next: boolean): void => {
      if (doc === null || !snapshot.editable) {
        return;
      }
      // Only when the parsed source still matches the buffer (the offsets
      // must stay honest), and only through a single edit.
      const source = doc.getSnapshot().text;
      if (source !== markdownClip.text) {
        return;
      }
      doc.edit(
        source.slice(0, marker.offset) + (next ? "[x]" : "[ ]") + source.slice(marker.offset + 3),
      );
    },
    [doc, snapshot.editable, markdownClip.text],
  );

  const showEditor = snapshot.editable && !snapshot.showMarkdown;
  const editorInputRef = useRef<HTMLTextAreaElement | null>(null);
  // ── Ticket 23: the editor-side comments (staged per the chat's composer
  // key; only File-sourced comments on THIS path reach the gutter —
  // `staged_file_comments`, preview.rs:751-762). The overlay mounts only
  // over a live editor (read-only documents get none, matching the
  // desktop's `render_editor_comment_overlays` call site).
  const stagedReview = useReviewComments(chatId);
  const editorReview: CodeReviewWiring | null = showEditor
    ? {
      comments: stagedReview.comments.filter(
        (comment) => comment.source.kind === "file" && comment.path === path,
      ),
      activeId: stagedReview.activeEditorComment,
      draft: stagedReview.editorDraft !== null && stagedReview.editorDraft.path === path ? stagedReview.editorDraft : null,
      onOpenDraft: (line) => reviewCommentStore.openEditorDraft(chatId, path, line),
      onToggleActive: (id) => reviewCommentStore.toggleEditorComment(chatId, id),
      onCardEdit: (id) => reviewCommentStore.editEditorComment(chatId, id),
      onCardRemove: (id) => reviewCommentStore.removeComment(chatId, id),
      onDraftBody: (body) => reviewCommentStore.setEditorDraftBody(chatId, body),
      onDraftCancel: () => reviewCommentStore.cancelEditorDraft(chatId),
      onDraftCommit: () => reviewCommentStore.commitEditorDraft(chatId),
    }
    : null;
  const toolbar = (
    <ViewerToolbar
      path={path}
      markdown={markdown}
      showingMarkdown={snapshot.showMarkdown}
      snapshot={doc === null ? null : snapshot}
      onToggleMarkdown={
        doc !== null
          ? () => {
              doc.setShowMarkdown(!snapshot.showMarkdown);
              if (snapshot.showMarkdown) {
                // Turning the preview off focuses the code view.
                setMarkdownFocus(true);
              }
            }
          : null
      }
      onReveal={() => {
        // `files-reveal-active` → RevealFile: dock the explorer and reveal
        // there (the editor surface carries no tree of its own anymore).
        rightPaneStore.revealInFilesPanel(chatId, path);
      }}
      onToggleWordWrap={() => {
        uiSettings.updateImmediate({ filesWordWrap: !settings.filesWordWrap });
      }}
      wordWrap={settings.filesWordWrap}
      onRetrySave={doc === null ? null : () => doc.save()}
    />
  );

  let body: React.ReactNode;
  if (snapshot.phase.kind === "loading") {
    body = <p className="files-note files-note-faint">Loading file…</p>;
  } else if (snapshot.phase.kind === "error") {
    body = (
      <div className="files-note">
        <p className="files-note-error">{snapshot.phase.message}</p>
        <button type="button" className="btn btn-ghost" onClick={() => doc?.load()}>
          Retry
        </button>
      </div>
    );
  } else if (snapshot.file !== null && snapshot.phase.kind === "readOnly" && snapshot.file.text == null) {
    body = <p className="files-note files-note-faint">{readOnlyMessage(snapshot.phase.reason)}</p>;
  } else if (markdownBlocks !== null) {
    body = (
      <div className="files-markdown-scroll">
        <MarkdownView
          blocks={markdownBlocks}
          documentPath={path}
          truncated={markdownClip.truncated}
          editable={snapshot.editable}
          onToggleTask={onToggleTask}
          onOpenPath={(target) => rightPaneStore.addFileSurface(chatId, target)}
          loadImage={client !== null && checkoutId !== null ? loadImage : null}
        />
      </div>
    );
  } else if (showEditor) {
    body = (
      <EditorContextMenu textareaRef={editorInputRef} editable>
        <div className="files-editor-body">
          <CodeView
            text={snapshot.text}
            path={path}
            editable
            onChange={(text) => doc?.edit(text)}
            codeFontSize={settings.codeFontSize}
            wordWrap={settings.filesWordWrap}
            autoFocus={markdownFocus}
            inputRef={editorInputRef}
            review={editorReview}
          />
        </div>
      </EditorContextMenu>
    );
  } else {
    const truncated = snapshot.file !== null ? truncatedMessage(snapshot.file) : null;
    body = (
      <div className="files-editor-body">
        {truncated !== null && <div className="files-truncated-banner" role="status">{truncated}</div>}
        <CodeView
          text={snapshot.text}
          path={path}
          editable={false}
          onChange={() => {}}
          codeFontSize={settings.codeFontSize}
          wordWrap={settings.filesWordWrap}
        />
      </div>
    );
  }

  return (
    <div className="files-viewer-column">
      <div className="surface-toolbar files-viewer-toolbar" role="toolbar" aria-label="File viewer">
        <div className="files-viewer-toolbar-leading">{toolbar}</div>
      </div>
      <div className="files-viewer">
        <CloseLifecycleBanner
          chatId={chatId}
          surface={surface}
          snapshot={snapshot}
          doc={doc}
        />
        <PhaseBanner
          snapshot={snapshot}
          doc={doc}
          confirmingReload={confirmingReload}
          onRequestReload={() => {
            // request_reload: dirty buffers ask before discarding.
            if (snapshot.dirty) {
              setConfirmingReload(true);
            } else {
              doc?.reloadFromDisk();
            }
          }}
          onCancelReload={() => setConfirmingReload(false)}
          onConfirmReload={() => {
            setConfirmingReload(false);
            doc?.reloadFromDisk();
          }}
          onKeepEditing={() => {
            setConfirmingReload(false);
            doc?.keepEditing();
          }}
        />
        <div className="files-viewer-body">{body}</div>
      </div>
    </div>
  );
}

/**
 * The close-request banner (preview.rs:2082-2140): pending saves say so;
 * a blocked close offers Retry / Keep Open / Discard Changes.
 */
function CloseLifecycleBanner({
  chatId,
  surface,
  snapshot,
  doc,
}: {
  readonly chatId: string;
  readonly surface: { kind: "file"; id: string };
  readonly snapshot: FileDocumentSnapshot;
  readonly doc: FileDocument | null;
}) {
  if (!rightPaneStore.isCloseRequested(surface)) {
    return null;
  }
  const blocked = doc !== null && doc.blocksLifecycleClose();
  return (
    <div className="files-banner files-banner-lifecycle" role="alert">
      <span className="files-banner-detail">
        {blocked ? "Changes could not be saved safely." : "Saving changes before closing…"}
      </span>
      {blocked && (
        <span className="files-banner-actions">
          <button type="button" className="files-banner-action" onClick={() => doc?.save()}>
            Retry
          </button>
          <button
            type="button"
            className="files-banner-action files-banner-action-muted"
            onClick={() => rightPaneStore.cancelFileClose(chatId, surface)}
          >
            Keep Open
          </button>
          <button
            type="button"
            className="files-banner-action files-banner-action-danger"
            onClick={() => {
              doc?.discardChanges();
              rightPaneStore.completeFileClose(chatId, surface);
            }}
          >
            Discard Changes
          </button>
        </span>
      )}
    </div>
  );
}

/**
 * The external-change / reload-confirmation banner (preview.rs:2181-2241):
 * "This file changed outside Zeron." with Keep Editing / Reload from Disk,
 * or the two-step "Discard unsaved changes?" with Cancel / Discard & Reload.
 */
function PhaseBanner({
  snapshot,
  doc,
  confirmingReload,
  onRequestReload,
  onCancelReload,
  onConfirmReload,
  onKeepEditing,
}: {
  readonly snapshot: FileDocumentSnapshot;
  readonly doc: FileDocument | null;
  readonly confirmingReload: boolean;
  readonly onRequestReload: () => void;
  readonly onCancelReload: () => void;
  readonly onConfirmReload: () => void;
  readonly onKeepEditing: () => void;
}) {
  const external =
    snapshot.phase.kind === "externallyModified" || snapshot.phase.kind === "conflict";
  if (!external && !confirmingReload) {
    return null;
  }
  return (
    <div className="files-banner files-banner-external" role={snapshot.phase.kind === "conflict" ? "alert" : "status"}>
      <span className="files-banner-detail">
        {confirmingReload ? "Discard unsaved changes?" : "This file changed outside Zeron."}
      </span>
      <span className="files-banner-actions">
        {confirmingReload ? (
          <>
            <button type="button" className="files-banner-action files-banner-action-muted" onClick={onCancelReload}>
              Cancel
            </button>
            <button type="button" className="files-banner-action" onClick={onConfirmReload}>
              Discard & Reload
            </button>
          </>
        ) : (
          <>
            <button type="button" className="files-banner-action files-banner-action-muted" onClick={onKeepEditing}>
              Keep Editing
            </button>
            <button type="button" className="files-banner-action" onClick={onRequestReload}>
              Reload from Disk
            </button>
          </>
        )}
      </span>
    </div>
  );
}

// ── Images ──────────────────────────────────────────────────────────────

type ImageState =
  | { readonly kind: "loading" }
  | { readonly kind: "loaded"; readonly load: WorkspaceImageLoad }
  | { readonly kind: "error"; readonly message: string };

function ImageViewer({
  chatId,
  client,
  path,
}: {
  readonly chatId: string;
  readonly client: WorkspaceFilesClient | null;
  readonly path: string;
}) {
  const settings = useUiSettings();
  const [state, setState] = useState<ImageState>({ kind: "loading" });

  useEffect(() => {
    if (client === null) {
      return;
    }
    let cancelled = false;
    let url: string | null = null;
    setState({ kind: "loading" });
    void (async () => {
      try {
        const load = await loadWorkspaceImage(client, path);
        if (cancelled) {
          URL.revokeObjectURL(load.url);
          return;
        }
        url = load.url;
        setState({ kind: "loaded", load });
      } catch (error) {
        if (!cancelled) {
          setState({ kind: "error", message: error instanceof Error ? error.message : String(error) });
        }
      }
    })();
    return () => {
      cancelled = true;
      if (url !== null) {
        URL.revokeObjectURL(url);
      }
    };
  }, [client, path]);

  const toolbar = (
    <ViewerToolbar
      path={path}
      markdown={false}
      showingMarkdown={false}
      snapshot={null}
      onToggleMarkdown={null}
      onReveal={() => {
        rightPaneStore.revealInFilesPanel(chatId, path);
      }}
      onToggleWordWrap={() => {
        uiSettings.updateImmediate({ filesWordWrap: !settings.filesWordWrap });
      }}
      wordWrap={settings.filesWordWrap}
      onRetrySave={null}
    />
  );

  return (
    <div className="files-viewer-column">
      <div className="surface-toolbar files-viewer-toolbar" role="toolbar" aria-label="File viewer">
        <div className="files-viewer-toolbar-leading">{toolbar}</div>
      </div>
      <div className="files-viewer">
        <div className="files-viewer-body files-image-body">
          {state.kind === "loading" && <p className="files-note files-note-faint">Loading image…</p>}
          {state.kind === "error" && <p className="files-note files-note-error">{state.message}</p>}
          {state.kind === "loaded" && (
            <ImageView
              src={state.load.url}
              natural={{ width: state.load.width, height: state.load.height }}
              alt={fileName(path)}
            />
          )}
        </div>
      </div>
    </div>
  );
}
