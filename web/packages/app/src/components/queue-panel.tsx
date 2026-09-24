import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactElement,
  type DragEvent as ReactDragEvent,
} from "react";
import type { EngineClient } from "@zeron/engine-client";
import { Icon, type IconName } from "@zeron/icons";
import type { QueuedMessage } from "@zeron/proto";
import { ATTACHMENT_ONLY_TEXT, bytesToImageDataUrl } from "../lib/attachments";
import { describeQueueError, mintEditorInstanceId } from "../lib/queue-actions";
import {
  availableQueuePrimaryAction,
  modifierSendCompactLabel,
  modifierSendLabel,
  oneLine,
  queueAttachmentLabels,
  queueAttachmentSummary,
  queueDragOffsets,
  queueDropIndex,
  queueLatestShortcutVisible,
  queuePreviewLimit,
  queueVisibleText,
  visibleQueueRows,
} from "../lib/queue-row-logic";
import { withQueuePreviewGate } from "../lib/queue-thumbnail-gate";
import { getAttachmentSnapshot, loadAttachment, useAttachmentImage } from "../state/attachment-cache";
import { sidebarNotice } from "../state/notice";
import { isMacPlatform } from "../state/shortcuts";
import { useQueueStore } from "../state/queue-store-context";
import { Tooltip } from "./ui/Tooltip";
import { Lightbox } from "./lightbox";

/**
 * The message-queue panel — the web peer of `crates/ui/src/queue.rs`
 * (`queue_panel_surface` :223, `queue_row` :382). A frosted tray docked
 * directly behind the composer pill: top-rounded glass, a 30vh edge-faded
 * row list with contained wheel scrolling, and borderless rows (36px,
 * flush) whose only chrome is a hover wash. Each row shows a drag marker,
 * up to `queue_preview_limit()` 40×28 attachment thumbnails, the
 * one-line text (replaced by the delivery-gate string when gated), and a
 * trailing cluster of icon buttons — Discard, Edit, and ONE primary
 * action, Send now (spec decision 3: no Steer-now anywhere; no Hold chip —
 * `holdForTurnEnd` is engine-internal).
 *
 * Editing a row acquires the host's 60s lease, moves the row's text and
 * attachments into the composer (the chat page owns the lease lifecycle
 * and the 20s renewal heartbeat), and swaps the row's children to an
 * inline Save/Cancel pair. Drag reorders through `MOVE_QUEUED_MESSAGE`
 * with the 150ms EASE_OUT slide (`queue_drag_offsets`) on the dragged row
 * and every displaced row.
 */

interface QueuePanelProps {
  /** The engine client — queue-thumbnail and edit-staging loads. */
  readonly client: EngineClient;
  /** The host's verified device id — used as the edit-lease owner. */
  readonly editorDeviceId: string;
  /** Open the composer for editing this row (text + attachments seeded). */
  readonly onEditRow: (message: QueuedMessage) => void;
  /** The currently-edited row id (the composer is feeding text into it). */
  readonly editingRowId: string | null;
  /** True while the lease-closing RPC is in flight — the row says "Saving…". */
  readonly editFinishing: boolean;
  /** The composer's measured column width — drives `queuePreviewLimit`. */
  readonly composerWidth: number | null;
  /**
   * The host's `MESSAGE_QUEUE_ACTIONS_V1` support (the engine-capability
   * stand-in for the desktop's host registry until ticket 31's fleet).
   */
  readonly hostSupportsActions: boolean;
  /** The row's inline Save — the composer's `commit_queue_edit` path. */
  readonly onSaveEdit: () => void;
  /** Cancel the open edit (Escape / the row's inline Cancel / pre-action). */
  readonly onEditCancel: () => void;
}

/** `motion::FADE_QUICK` (150ms) — the JS mirror of `--rb-motion-fade-quick`. */
const FADE_QUICK_MS = 150;

export function QueuePanel({
  client,
  editorDeviceId,
  onEditRow,
  editingRowId,
  editFinishing,
  composerWidth,
  hostSupportsActions,
  onSaveEdit,
  onEditCancel,
}: QueuePanelProps) {
  const store = useQueueStore();
  const snapshot = useSyncExternalStore(
    useCallback((listener: () => void) => store.subscribe(listener), [store]),
    useCallback(() => store.getSnapshot(), [store]),
  );
  const rows = snapshot.rows;
  const reducedMotion = usePrefersReducedMotion();
  const busy = useRef(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  // ── Row-removal bookkeeping (`queue_removing`, queue.rs:1128) ──────────
  // Optimistically marked here; the row leaves when the watch echoes the
  // host's splice.
  const [removing, setRemoving] = useState<ReadonlySet<string>>(() => new Set());

  // ── The exit fade (`motion::fade_quick`, composer.rs:7459-7467) ────────
  // The desktop's fade is mount-only (the panel unmounts instantly when
  // the queue empties); the ticket asks for the exit leg too — hold the
  // last rows for one 150ms fade window, then unmount. Reduced motion
  // skips the hold entirely.
  const [exitRows, setExitRows] = useState<readonly QueuedMessage[] | null>(null);
  const prevRowsRef = useRef<readonly QueuedMessage[] | null>(null);
  useLayoutEffect(() => {
    const previous = prevRowsRef.current;
    prevRowsRef.current = rows;
    if (rows.length > 0) {
      if (exitRows !== null) {
        setExitRows(null);
      }
      return;
    }
    if (exitRows !== null || previous === null || previous.length === 0) {
      return;
    }
    if (reducedMotion) {
      return;
    }
    setExitRows(previous);
    const timer = setTimeout(() => setExitRows(null), FADE_QUICK_MS);
    return () => clearTimeout(timer);
  }, [rows, exitRows, reducedMotion]);

  const renderRows = rows.length > 0 ? rows : exitRows ?? EMPTY_ROWS;
  const fading = rows.length === 0 && exitRows !== null;

  // ── The row-list viewport (30vh, edge-faded, wheel-contained) ───────────
  const [listScroll, setListScroll] = useState({ top: 0, height: 0 });
  const remeasure = useCallback(() => {
    const el = listRef.current;
    if (el === null) {
      return;
    }
    setListScroll((current) => {
      const top = el.scrollTop;
      const height = el.clientHeight;
      return current.top === top && current.height === height ? current : { top, height };
    });
  }, []);
  useLayoutEffect(() => {
    remeasure();
  }, [remeasure, renderRows.length]);
  // gpui's scroll offset runs negative downward; `scrollTop` is positive.
  const [visibleFirst, visibleLast] = visibleQueueRows(-listScroll.top, listScroll.height, renderRows.length);

  // ── The drag state machine (`queue_drag` + `update_queue_drag_over`) ───
  // `from` is set by the source row's dragstart (the payload's `from`);
  // `over` recomputes on every dragover through `queue_drop_index`; the
  // rows paint at their `queue_drag_offsets` targets and the CSS transition
  // tweens them (150ms EASE_OUT, snapping under reduced motion). `dragend`
  // fires even without a drop, so a pointer released outside the panel
  // never leaves a stale gap (`cancel_queue_drag`).
  const [drag, setDrag] = useState<{ from: number; over: number; prevOver: number } | null>(null);
  const dragRef = useRef(drag);
  dragRef.current = drag;

  const onRowDragStart = useCallback((from: number) => {
    setDrag({ from, over: from, prevOver: from });
  }, []);
  const onRowDragEnd = useCallback(() => {
    setDrag(null);
  }, []);
  const onPanelDragOver = useCallback(
    (event: ReactDragEvent<HTMLDivElement>) => {
      const current = dragRef.current;
      if (current === null) {
        return;
      }
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      const el = listRef.current;
      if (el === null) {
        return;
      }
      const rect = el.getBoundingClientRect();
      const panelY = event.clientY - rect.top + el.scrollTop;
      const over = queueDropIndex(panelY, renderRows.length);
      if (over !== current.over) {
        setDrag({ from: current.from, over, prevOver: current.over });
      }
    },
    [renderRows.length],
  );
  const onPanelDrop = useCallback(
    (event: ReactDragEvent<HTMLDivElement>) => {
      const current = dragRef.current;
      if (current === null) {
        return;
      }
      event.preventDefault();
      setDrag(null);
      const from = current.from;
      const to = current.over;
      const id = renderRows[from]?.id;
      if (id === undefined || from === to) {
        return;
      }
      void store
        .move(id, to)
        .then((changed) => {
          if (!changed) {
            sidebarNotice.set("Couldn't reorder the queue");
            store.resubscribe();
          }
        })
        .catch(() => {
          sidebarNotice.set("Couldn't reorder the queue");
          store.resubscribe();
        });
    },
    [renderRows, store],
  );

  // ── Row actions ─────────────────────────────────────────────────────────

  const onSendNow = useCallback(
    (row: QueuedMessage) => {
      if (editingRowId === row.id) {
        onEditCancel();
      }
      void store
        .sendNow(row.id)
        .then((sent) => {
          if (!sent) {
            sidebarNotice.set("Couldn't send that message");
          }
        })
        .catch(() => {
          sidebarNotice.set("Couldn't send that message");
        });
    },
    [store, editingRowId, onEditCancel],
  );

  const onEdit = useCallback(
    async (row: QueuedMessage) => {
      if (busy.current) {
        return;
      }
      busy.current = true;
      try {
        const instanceId = mintEditorInstanceId();
        const outcome = await store.beginEdit(row.id, instanceId);
        if (outcome.kind === "locked") {
          sidebarNotice.set("That queued message is being edited on another device");
          return;
        }
        if (outcome.kind === "missing") {
          sidebarNotice.set("That queued message is no longer available");
          return;
        }
        // Pre-load the row's attachments so the composer's strip stages
        // them from the shared cache (`begin_queue_edit`'s loaded set,
        // queue.rs:1300-1314). A failed load skips that one — the commit
        // still preserves it engine-side.
        await Promise.all(
          outcome.attachments.map((path) =>
            loadAttachment(client, editorDeviceId, path).catch(() => {}),
          ),
        );
        let seedText = queueVisibleText(outcome.text, outcome.attachments);
        if (outcome.attachments.length > 0 && seedText === ATTACHMENT_ONLY_TEXT) {
          seedText = "";
        }
        onEditRow({ ...row, text: seedText, attachments: [...outcome.attachments] });
      } catch {
        sidebarNotice.set("Connect to the chat host to edit this message");
      } finally {
        busy.current = false;
      }
    },
    [store, client, editorDeviceId, onEditRow],
  );

  const onRemove = useCallback(
    (row: QueuedMessage) => {
      if (removing.has(row.id)) {
        return;
      }
      if (editingRowId === row.id) {
        onEditCancel();
      }
      setRemoving((current) => new Set(current).add(row.id));
      void store
        .remove(row.id)
        .then((removed) => {
          if (!removed) {
            sidebarNotice.set("That message had already left the queue");
            store.resubscribe();
          }
        })
        .catch(() => {
          sidebarNotice.set("Couldn't remove the message");
          store.resubscribe();
        })
        .finally(() => {
          setRemoving((current) => {
            const next = new Set(current);
            next.delete(row.id);
            return next;
          });
        });
    },
    [store, removing, editingRowId, onEditCancel],
  );

  // ── The latest-row shortcut reveal (`queue_shortcut_revealed`) ──────────
  // The desktop reveals the ⌘↵/⌃↵ cap while the platform modifier is held
  // AND the composer is empty AND no queued-row edit is open (the pickers'
  // open state lives inside the composer and is not visible here — a
  // documented approximation, see the ticket's Comments). The composer's
  // emptiness is sampled from the DOM: the panel is mounted inside the
  // composer's own tree.
  const [revealShortcut, setRevealShortcut] = useState(false);
  const composerIsEmpty = useCallback(() => {
    const composer = rootRef.current?.closest(".composer");
    if (!(composer instanceof HTMLElement)) {
      return false;
    }
    const input = composer.querySelector<HTMLTextAreaElement>("textarea.composer-input");
    if (input === null || input.value.length > 0) {
      return false;
    }
    return composer.querySelector(".composer-attachments-strip") === null;
  }, []);
  useEffect(() => {
    const modHeld = (event: KeyboardEvent): boolean =>
      event.key === "Meta" || event.key === "Control";
    const sample = (held: boolean): void => {
      setRevealShortcut(held && composerIsEmpty() && editingRowId === null);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (modHeld(event) || event.metaKey || event.ctrlKey) {
        sample(true);
      }
    };
    const onKeyUp = (event: KeyboardEvent): void => {
      sample(modHeld(event) ? false : event.metaKey || event.ctrlKey);
    };
    const onBlur = (): void => setRevealShortcut(false);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
    };
  }, [composerIsEmpty, editingRowId]);

  const previewLimit = queuePreviewLimit(composerWidth);

  if (renderRows.length === 0) {
    return null;
  }

  const count = renderRows.length;
  const dragNow = drag;

  return (
    <div
      ref={rootRef}
      className="queue-panel"
      role="list"
      aria-label="Queued messages"
      data-open={fading ? "false" : "true"}
      onDragOver={onPanelDragOver}
      onDrop={onPanelDrop}
    >
      <div className="queue-panel-list" ref={listRef} onScroll={remeasure}>
        {renderRows.map((row, ix) => {
          const offset =
            dragNow === null
              ? 0
              : queueDragOffsets(ix, dragNow.from, dragNow.prevOver, dragNow.over)[1];
          return (
            <QueueRow
              key={row.id}
              row={row}
              index={ix}
              count={count}
              client={client}
              deviceId={editorDeviceId}
              previewLimit={previewLimit}
              beingEdited={editingRowId === row.id}
              beingRemoved={removing.has(row.id)}
              editFinishing={editFinishing}
              hostSupportsActions={hostSupportsActions}
              revealShortcut={revealShortcut}
              offset={offset}
              thumbnailVisible={ix >= visibleFirst && ix < visibleLast}
              onSendNow={onSendNow}
              onEdit={onEdit}
              onRemove={onRemove}
              onSaveEdit={onSaveEdit}
              onCancelEdit={onEditCancel}
              onDragStartRow={onRowDragStart}
              onDragEndRow={onRowDragEnd}
            />
          );
        })}
      </div>
    </div>
  );
}

const EMPTY_ROWS: readonly QueuedMessage[] = [];

interface QueueRowProps {
  readonly row: QueuedMessage;
  readonly index: number;
  readonly count: number;
  readonly client: EngineClient;
  readonly deviceId: string;
  readonly previewLimit: number;
  readonly beingEdited: boolean;
  readonly beingRemoved: boolean;
  readonly editFinishing: boolean;
  readonly hostSupportsActions: boolean;
  readonly revealShortcut: boolean;
  /** The drag-slide target offset in px (`queue_drag_offsets`). */
  readonly offset: number;
  /** Whether this row is inside the scrolled viewport (thumbnail windowing). */
  readonly thumbnailVisible: boolean;
  readonly onSendNow: (row: QueuedMessage) => void;
  readonly onEdit: (row: QueuedMessage) => void;
  readonly onRemove: (row: QueuedMessage) => void;
  readonly onSaveEdit: () => void;
  readonly onCancelEdit: () => void;
  readonly onDragStartRow: (index: number) => void;
  readonly onDragEndRow: () => void;
}

/**
 * One queued message (`queue_row`, queue.rs:382): a quiet drag marker, the
 * text (or the delivery-gate string), and a trailing cluster with exactly
 * one primary action. The row is NOT a card — borderless and transparent
 * at idle; only hover/editing/removing states paint a wash or fade. The
 * entire row (not just the marker) is the drag hitbox.
 */
function QueueRow(props: QueueRowProps) {
  const {
    row,
    index,
    count,
    client,
    deviceId,
    previewLimit,
    beingEdited,
    beingRemoved,
    editFinishing,
    hostSupportsActions,
    revealShortcut,
    offset,
    thumbnailVisible,
    onSendNow,
    onEdit,
    onRemove,
    onSaveEdit,
    onCancelEdit,
    onDragStartRow,
    onDragEndRow,
  } = props;

  const gate = row.deliveryGate ?? null;
  const deliveryBlocked = gate !== null;
  const interactionBlocked = deliveryBlocked || beingRemoved;
  const draggable = !beingEdited && !interactionBlocked;
  const resolvedPrimary = availableQueuePrimaryAction(interactionBlocked, hostSupportsActions);

  const text = useMemo(() => {
    if (gate !== null && !beingEdited) {
      return gate.kind === "editing" ? `Editing on ${gate.ownerDeviceId}` : "Needs review";
    }
    return oneLine(queueVisibleText(row.text, row.attachments ?? []));
  }, [gate, beingEdited, row.text, row.attachments]);

  const attachments = row.attachments ?? [];
  const labels = useMemo(() => queueAttachmentLabels(attachments), [attachments]);
  const summary = queueAttachmentSummary(labels);
  const onlyImages = text === ATTACHMENT_ONLY_TEXT;
  const title = onlyImages ? summary : text;

  const dragMarker = (
    <div
      className="queue-row-marker"
      data-blocked={draggable ? undefined : "true"}
      aria-hidden="true"
    >
      <Icon name="queueDragHandle" size={13} />
    </div>
  );

  return (
    <div
      className="queue-row"
      role="listitem"
      data-queue-id={row.id}
      data-compact={previewLimit === 1 ? "true" : "false"}
      data-editing={beingEdited ? "true" : undefined}
      data-removing={beingRemoved ? "true" : undefined}
      style={{ transform: `translateY(${offset}px)` }}
      draggable={draggable}
      onDragStart={(event) => {
        if (!draggable) {
          event.preventDefault();
          return;
        }
        event.dataTransfer.setData("application/x-zeron-queue-id", row.id);
        event.dataTransfer.effectAllowed = "move";
        onDragStartRow(index);
      }}
      onDragEnd={() => onDragEndRow()}
    >
      {beingEdited ? (
        <>
          {/* The marker's 14px spacer keeps text alignment while the drag
              glyph stands down (queue.rs:607). */}
          <div className="queue-row-marker-spacer" aria-hidden="true" />
          <div className="queue-row-editing-text">
            {editFinishing ? "Saving…" : "Editing in composer"}
          </div>
          <div className="queue-row-actions">
            <QueueActionButton
              label="Save to queue"
              glyph="queueCheck"
              enabled={!editFinishing}
              onClick={onSaveEdit}
            />
            <QueueActionButton
              label="Cancel"
              glyph="queueClose"
              enabled={!editFinishing}
              onClick={onCancelEdit}
            />
          </div>
        </>
      ) : (
        <>
          {dragMarker}
          {attachments.slice(0, previewLimit).map((path, ix) => (
            <QueueThumbnail
              key={path}
              client={client}
              deviceId={deviceId}
              path={path}
              visible={thumbnailVisible}
            />
          ))}
          {attachments.length > previewLimit ? (
            <div
              className="queue-row-overflow"
              aria-label={`${attachments.length - previewLimit} more attachments; edit message to view all`}
            >
              +{attachments.length - previewLimit}
            </div>
          ) : null}
          <div className="queue-row-text">
            <div className="queue-row-title" title={title}>
              {title}
            </div>
            {labels.length > 0 && !onlyImages ? (
              <div className="queue-row-summary" title={summary}>
                {summary}
              </div>
            ) : null}
          </div>
          <div className="queue-row-actions">
            <QueueActionButton
              label={beingRemoved ? "Removing…" : "Remove"}
              glyph="trashBinMinimalistic"
              enabled={!beingRemoved}
              onClick={() => onRemove(row)}
            />
            <QueueActionButton
              label="Edit"
              glyph="pen"
              enabled={!beingRemoved}
              onClick={() => void onEdit(row)}
            />
            <QueuePrimaryButton
              enabled={resolvedPrimary !== null && !beingRemoved}
              tooltip={resolvedPrimary !== null ? "Send now (interrupt)" : "Waiting for provider capabilities"}
              showShortcut={queueLatestShortcutVisible(
                index,
                count,
                revealShortcut,
                resolvedPrimary !== null && !beingRemoved,
              )}
              compact={previewLimit === 1}
              onClick={() => onSendNow(row)}
            />
          </div>
        </>
      )}
    </div>
  );
}

/**
 * The row's small 40×28 attachment preview (`queue_thumbnail`,
 * queue.rs:841-874): 1px hairline frame, `ink(0.035)` plate, inner 38×26
 * cover-fit image. Loads go through the SAME cache the transcript uses,
 * throttled by the process-wide preview gate (queue.rs:262-265) and
 * windowed to the scrolled viewport (`visible_queue_rows`). Click opens
 * the full preview in the lightbox (queue.rs:827-840).
 */
function QueueThumbnail({
  client,
  deviceId,
  path,
  visible,
}: {
  readonly client: EngineClient;
  readonly deviceId: string;
  readonly path: string;
  readonly visible: boolean;
}) {
  const snapshot = useAttachmentImage(deviceId, path);
  const [preview, setPreview] = useState<{ name: string; src: string } | null>(null);
  const loaded = snapshot.state === "loaded" && snapshot.image !== null;

  useEffect(() => {
    if (!visible || snapshot.state !== "loading") {
      return;
    }
    void withQueuePreviewGate(() => loadAttachment(client, deviceId, path));
  }, [visible, snapshot.state, client, deviceId, path]);

  // The cache's backoff countdown elapses without a re-render — this row
  // owns its own re-attempt, still under the gate.
  const retryIn = snapshot.state === "error" ? snapshot.retryIn : 0;
  useEffect(() => {
    if (!visible || retryIn <= 0) {
      return;
    }
    const timer = setTimeout(() => {
      void withQueuePreviewGate(() => loadAttachment(client, deviceId, path));
    }, retryIn);
    return () => clearTimeout(timer);
  }, [visible, retryIn, client, deviceId, path]);

  return (
    <button
      type="button"
      className="queue-thumb"
      data-state={snapshot.state}
      aria-label={loaded ? `Preview ${snapshot.image?.name ?? path}` : "Open attachment preview"}
      onClick={() => {
        const image = getAttachmentSnapshot(deviceId, path).image;
        if (image !== null) {
          setPreview({ name: image.name, src: bytesToImageDataUrl(image.mime, image.bytes) });
        }
      }}
    >
      {loaded && snapshot.image !== null ? (
        <img
          className="queue-thumb-img"
          src={bytesToImageDataUrl(snapshot.image.mime, snapshot.image.bytes)}
          alt=""
          draggable={false}
        />
      ) : (
        <Icon name="queuePaperclip" size={14} className="queue-thumb-placeholder" />
      )}
      {preview !== null && (
        <Lightbox name={preview.name} src={preview.src} onClose={() => setPreview(null)} />
      )}
    </button>
  );
}

/**
 * A trailing glyph button (`queue_action`, queue.rs:908-959): 28×28, radius
 * 5, opacity 0.72 idle → 1.0 + `ink(0.07)` wash on hover; disabled → arrow
 * cursor + opacity 0.45 (still hoverable — the tooltip carries the label).
 */
function QueueActionButton({
  label,
  glyph,
  enabled,
  onClick,
}: {
  readonly label: string;
  readonly glyph: IconName;
  readonly enabled: boolean;
  readonly onClick: () => void;
}) {
  return (
    <QueueActionTooltip
      label={label}
      trigger={
        <button
          type="button"
          className="queue-row-action"
          data-enabled={enabled ? "true" : "false"}
          aria-label={label}
          aria-disabled={enabled ? undefined : "true"}
          onClick={() => {
            if (enabled) {
              onClick();
            }
          }}
        >
          <Icon name={glyph} size={13} />
        </button>
      }
    />
  );
}

/**
 * Send now — the row's single primary action
 * (`queue_primary_action_button`, queue.rs:979-1033): 28px icon-only under
 * the narrow composer, 72px icon+label otherwise, 11.5px label. While the
 * modifier is held on an empty composer, the LAST row's button shows the
 * shortcut cap instead of the icon/label.
 */
function QueuePrimaryButton({
  enabled,
  tooltip,
  showShortcut,
  compact,
  onClick,
}: {
  readonly enabled: boolean;
  readonly tooltip: string;
  readonly showShortcut: boolean;
  readonly compact: boolean;
  readonly onClick: () => void;
}) {
  const isMac = isMacPlatform();
  return (
    <QueueActionTooltip
      label={tooltip}
      trigger={
        <button
          type="button"
          className="queue-row-primary"
          data-compact={compact ? "true" : "false"}
          data-enabled={enabled ? "true" : "false"}
          data-shortcut={showShortcut ? "true" : "false"}
          aria-label={tooltip}
          aria-disabled={enabled ? undefined : "true"}
          onClick={() => {
            if (enabled) {
              onClick();
            }
          }}
        >
          {showShortcut ? (
            compact ? modifierSendCompactLabel(isMac) : modifierSendLabel(isMac)
          ) : compact ? (
            <Icon name="queueSend" size={13} />
          ) : (
            "Send now"
          )}
        </button>
      }
    />
  );
}

/**
 * The action tooltip (`QueueActionTooltip`, queue.rs:54-72): 8×5 padding,
 * 6px radius, 1px border, `surface_overlay` plate, 10.5px muted text —
 * over the shared `Tooltip` primitive with the queue family's 350ms show
 * delay (queue.rs:933).
 */
function QueueActionTooltip({
  label,
  trigger,
}: {
  readonly label: string;
  readonly trigger: ReactElement;
}) {
  return <Tooltip label={label} delay={350} popupClassName="queue-action-tooltip" trigger={trigger} />;
}

/** `prefers-reduced-motion` as live state (the row slides snap under it). */
function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(
    () =>
      typeof window !== "undefined" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = (): void => setReduced(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);
  return reduced;
}
