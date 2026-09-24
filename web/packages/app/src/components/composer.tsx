import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ClipboardEvent as ReactClipboardEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { Icon } from "@zeron/icons";
import type { Chat, FileSearchMatch, HarnessDescriptor, HarnessId, UserInputAnswer } from "@zeron/proto";
import { MESSAGE_QUEUE_ATTACHMENTS_V1, MESSAGE_QUEUE_V1 } from "@zeron/proto";
import { encodeScopedId, methods, RpcError } from "@zeron/engine-client";
import type { EngineSession } from "../state/engine-session";
import type { TranscriptStore } from "../state/transcript-store";
import { useEngineStatus, useNow } from "../state/hooks";
import { useFleetSnapshot } from "../state/fleet";
import { PickerCatalog } from "../state/picker-catalog";
import { ESCAPE_PRIORITY, registerEscapeSurface } from "../state/escape";
import { effectiveIndicator } from "../lib/view";
import {
  noticeLabelForTone,
  noticeToneForMessage,
} from "../lib/notice-chip";
import { NoticeChip } from "./notice-chip";
import { chatDrafts, composerDefaults, draftFromChat, rememberedModelFor } from "../lib/composer-draft";
import { useDraftModelReconciliation } from "../lib/composer-reconciliation";
import { offeredHarnesses } from "../lib/model-rows";
import {
  ATTACHMENT_ONLY_TEXT,
  AttachmentUploadError,
  formatByName,
  formatToMime,
  stageBytes,
  stageFile,
  uploadAttachments,
  type StagedAttachment,
} from "../lib/attachments";
import {
  describeSendError,
  buildChatConfig,
  mintMessageId,
  persistChatConfig,
  queueMessage,
  sendInterrupt,
  sendRun,
  type DraftConfig,
} from "../lib/composer-actions";
import {
  attachmentStripHeight,
  COMPOSER_MAX_WIDTH,
  COMPOSER_WIDTH_EPSILON,
  COMPACT_TOTAL_HEIGHT,
  composerFlip,
  composerTotalHeight,
  composerWidthChanged,
  flipMorphDone,
  flipMorphHeight,
  flipMorphProgress,
  flipMorphStep,
  inputDragScrollDelta,
  inputOverflowEdges,
  INPUT_LINE_HEIGHT,
  modelHandoffPosition,
  modelSlotOffset,
  modelTravel,
  PILL_BORDER_V,
  RESIZE_SETTLE_MS,
  ROUTE_SNAP_MS,
  routeInputGeometry,
  type FlipMorph,
} from "../lib/composer-flip";
import {
  beginInterrupt,
  composerHasContent,
  modifiedSubmitTarget,
  resolveEnterAction,
  resolveSendCwd,
  retainLiveInterrupts,
  sendBlocked,
  sendButtonMode,
  shouldPublishOptimisticEcho,
} from "../lib/composer-send";
import { menuStep } from "../lib/picker-search";
import { dockHeight, routeChromeOpacities, type DockFrame } from "../lib/composer-dock";
import { createChat, waitForChatRow } from "../lib/chat-actions";
import { echoStore } from "../state/transcript-store";
import { useUiSettings } from "../state/ui-settings";
import { useIsPhone } from "../state/media";
import {
  seedAttachment,
  beginUploadProgress,
  endUploadProgress,
  setUploadProgress,
  getAttachmentSnapshot,
} from "../state/attachment-cache";
import {
  localFileLink,
  MENTION_TOOLTIP_DELAY_MS,
  MENTION_TOOLTIP_HEIGHT,
  mentionErrorMessage,
  mentionResponseIsCurrent,
  mentionTooltipPromote,
  mentionTooltipReduce,
  mentionToken,
  TextProjection,
  type CompletionToken,
  type MentionTooltipPhase,
  type MentionTooltipTarget,
} from "../lib/mentions";
import { parseSlashCommands, refilterSlash, slashErrorMessage, slashToken, type SlashCache } from "../lib/slash";
import {
  AUTO_ADVANCE_MS,
  COMPOSER_REST_PLACEHOLDER,
  WIZARD_SAFETY_NET_MS,
  Wizard,
  escapeDismissesCompletion,
  inputRequestResolved,
  pendingInputRequest,
  wizardCommitThenAdvance,
  wizardEscapeGoesBack,
  wizardPlaceholder,
} from "../lib/wizard";
import { ComposerPickers } from "./composer-pickers";
import { Tooltip, virtualAnchorAt } from "./ui/Tooltip";
import { NewThreadGitSelectors, NewThreadTargetSelectors } from "./composer/new-thread-selectors";
import { AttachmentStrip } from "./attachments/attachment-strip";
import { CommentsChip } from "./review-comments/comments-chip";
import { MentionPopup } from "./composer/mention-popup";
import { SlashPopup } from "./composer/slash-popup";
import { ComposerWizard } from "./composer/wizard";
import { reviewCommentStore, useReviewComments } from "../state/review-comments";
import { commentStripHeight, withComments } from "../lib/review-comments";

/**
 * The composer — the desktop's `crates/ui/src/composer.rs` ported to React:
 * the centred 768px column (failure notice, queue-degraded caption, the
 * queue tray tucked behind the pill, the 26px-radius pill itself, and the
 * 24px session-footer slot), the width-driven compact↔expanded flip with
 * hysteresis and a 180ms height morph (text glides, controls stay pinned to
 * the stationary bottom edge), auto-grow that clamps the textarea BOX to
 * 76–260 and the PILL to 124–308, and the Send / Queue / Stop send path
 * (never "Steer" — spec decision 3: a busy chat gets `QueueMessage` with
 * `holdForTurnEnd: true`).
 */

/** The failure notice; `key` scopes it to one chat (null = global). */
interface FailureNotice {
  readonly message: string;
  readonly key: string | null;
}

/** `FileMentionState` (composer.rs:3944-3959) — the `@` popup's state. */
interface MentionUiState {
  readonly token: CompletionToken | null;
  readonly results: readonly FileSearchMatch[];
  readonly active: number | null;
  readonly loading: boolean;
  readonly error: string | null;
}

/** `SlashState` (composer.rs:3930-3942) — the `/` popup's state. */
interface SlashUiState {
  readonly token: CompletionToken | null;
  readonly filtered: readonly number[];
  readonly active: number | null;
  /** The harness the popup is showing commands for (the cache key). */
  readonly harness: HarnessId | null;
  readonly loading: boolean;
  readonly error: string | null;
}

/** A dismissed completion's memory (composer.rs `dismissed`): the token's
 * range plus its full text — the caret may move within the SAME unchanged
 * token while the popup stays closed; any edit re-enables completion. */
interface DismissedToken {
  readonly start: number;
  readonly end: number;
  readonly text: string;
}

/** The animated geometry one layout pass resolves (see the evaluate pass). */
interface PillLayout {
  readonly pillHeight: number;
  readonly boxHeight: number;
  readonly textPad: number;
  readonly clusterInset: number;
  readonly clusterDy: number;
  readonly textGlide: number;
  /** The model chip's handoff offset (`model_offset`, e0c1e936). */
  readonly modelLeft: number;
  /** The model chip's handoff opacity (`model_opacity`). */
  readonly modelOpacity: number;
  readonly morphing: boolean;
}

const REST_LAYOUT: PillLayout = {
  pillHeight: COMPACT_TOTAL_HEIGHT,
  boxHeight: COMPACT_TOTAL_HEIGHT - PILL_BORDER_V,
  textPad: 12,
  clusterInset: 8,
  clusterDy: 0,
  textGlide: 0,
  modelLeft: 0,
  modelOpacity: 1,
  morphing: false,
};

/**
 * The evaluate pass's no-op gate (ticket 64 §2.4): `setLayout` returns the
 * PRIOR state only when every PillLayout field is equal, so observer ticks
 * and repeated evaluates that resolve the same geometry publish nothing —
 * real height/mode/morph changes still land.
 */
function pillLayoutEquals(a: PillLayout, b: PillLayout): boolean {
  return (
    a.pillHeight === b.pillHeight &&
    a.boxHeight === b.boxHeight &&
    a.textPad === b.textPad &&
    a.clusterInset === b.clusterInset &&
    a.clusterDy === b.clusterDy &&
    a.textGlide === b.textGlide &&
    a.modelLeft === b.modelLeft &&
    a.modelOpacity === b.modelOpacity &&
    a.morphing === b.morphing
  );
}

interface ComposerProps {
  readonly session: EngineSession;
  readonly chat: Chat;
  readonly catalog: PickerCatalog;
  /**
   * The chat's live transcript store — the SAME store the transcript view
   * renders (one `WatchDocMessages` stream per open chat). The wizard's
   * latch (`pending_input_request` / `input_request_resolved`) reads it;
   * null only while the host has no session.
   */
  readonly transcript: TranscriptStore | null;
  /**
   * The measured conversation-column width, clamped to 768 — the desktop's
   * `set_available_width` feed. Null before the first measurement.
   */
  readonly availableWidth: number | null;
  /**
   * Ticket 64 §2.4's live width channel: the page's clamped composer
   * target, fed by the column's ResizeObserver on EVERY geometry tick —
   * outside React, so a deferred page publication (the blank-canvas sidebar
   * glide) never starves the evaluate pass. The strip width budget reads it
   * ahead of the published `availableWidth` prop.
   */
  readonly liveAvailableWidth?: { readonly current: number | null };
  /**
   * The queue panel (ticket 16 owns the body), rendered in the column's
   * tray slot — tucked 18px behind the pill per `QUEUE_COMPOSER_OVERLAP`.
   */
  readonly queueSlot?: ReactNode;
  /** The session footer row (the 24px slot under the pill). */
  readonly footerSlot?: ReactNode;
  /**
   * The queued row currently being edited in this composer — `null` when
   * the composer is free. When set, the textarea seeds with the row's text,
   * the row's attachments stage into the strip (from the attachment cache;
   * the chat page pre-loads them before opening the edit), the pre-edit
   * draft (text + staged) is stashed and handed back when the lease closes
   * (`queue_edit_draft`, queue.rs:1394-1398), and a submit commits the row
   * through `onEditFinish` (the lease protocol itself is ticket 16's).
   */
  readonly editingMessage?: { id: string; text: string; attachments?: readonly string[] } | null;
  readonly onEditFinish?: (outcome: {
    action: "commit" | "cancel" | "discard" | "releaseUnchanged";
    text: string;
    /** The live staged set on commit — re-uploaded by the lease closer. */
    staged?: readonly StagedAttachment[];
  }) => void;
  /** Escape while editing a queued row (the container binding, composer.rs:7473). */
  readonly onEditCancel?: () => void;
  /**
   * The queue row's inline Save handle (ticket 16): assigned the composer's
   * `commit_queue_edit` (queue.rs:1430) while a queued-row edit is open, so
   * the row's Save and the composer's submit share the one commit path.
   */
  readonly editCommitRef?: React.MutableRefObject<(() => void) | null>;
  /**
   * Mod+Enter with a truly empty composer activates the most recently
   * queued row (composer.rs:6023). The action itself lives with the queue
   * store (ticket 16); this ticket wires the call site only.
   */
  readonly activateLatestQueued?: () => void;
  /**
   * The shell's dock frame (ticket 15, `composer_dock.rs::DockFrame`):
   * while present, the dock's shared clock owns the pill's height
   * (`dock_height(amount)`) and both morphs stand down (`set_dock_frame`
   * kills them, composer.rs:4140-4149); the frame's `selectors`/`footer`
   * channels drive the new-thread chrome crossfade. Null when the host
   * mounts the composer without a dock (the pre-15 chat page).
   */
  readonly dockFrame?: DockFrame | null;
  /**
   * `ComposerEvent::NewThreadTransitionStarted`'s web half: the first send
   * off the new-thread canvas mints the chat and the host navigates to it
   * (the desktop's `select_chat` commit) — the route observation then drives
   * the dock. Called once the row exists, BEFORE the send leaves, with the
   * RAW minted id; the host scopes it for the route (§2.3) while the
   * composer keeps the raw id on the wire.
   */
  readonly onNewThreadLaunched?: (chatId: string) => void;
  /**
   * Where the composer publishes `dock_clearance_correction`
   * (composer.rs:7583) each layout pass — the shell's bottom-stack
   * measurement adds it so the transcript's clearance reserves the
   * DESTINATION footprint and never pumps mid-route.
   */
  readonly dockCorrectionRef?: { current: number };
  /**
   * The dock glide's live frame channel (ticket 57b, §2.3): the page's rAF
   * pump writes the frame it just ticked HERE, outside React, so the
   * evaluate pass consumes the live `amount`/`active` per frame without a
   * re-render. Null at render time (the `dockFrame` prop is authoritative
   * between glides); set only inside the pump's frame callback, immediately
   * before the evaluate it invokes through `dockEvaluateRef`.
   */
  readonly liveDockFrame?: { current: DockFrame | null };
  /**
   * The pump's evaluate channel: the composer parks its evaluate pass here
   * (identity-stable by design — every input is a ref) so the page's rAF
   * loop re-runs it once per animation frame with ZERO React state: during
   * a glide the pill's height/radius, the box height, and the clearance
   * correction are DOM writes; only the settle republishes `layout`.
   */
  readonly dockEvaluateRef?: { current: (() => void) | null };
}

export function Composer({
  session,
  chat,
  catalog,
  transcript,
  availableWidth,
  liveAvailableWidth,
  queueSlot,
  footerSlot,
  editingMessage,
  onEditFinish,
  onEditCancel,
  editCommitRef,
  activateLatestQueued,
  dockFrame = null,
  liveDockFrame,
  dockEvaluateRef,
  onNewThreadLaunched,
  dockCorrectionRef,
}: ComposerProps) {
  // The MERGED fleet snapshot: the composer's per-chat status lookups read
  // scoped rows across engines; the calls themselves go through the routed
  // session's client (the chat's owning engine).
  const snapshot = useFleetSnapshot();
  const engineStatus = useEngineStatus(session);
  const now = useNow(10_000);
  // `ComposerSendBehavior` — which Enter submits. Default "enter": bare
  // Enter sends, Mod+Enter is `ModifiedSubmit`.
  const sendBehavior = useUiSettings().composerSendBehavior;
  // The phone layer (≤768px, state/media.ts) flips a bare Enter to a native
  // newline (ticket 75) — a LIVE media match read at render, so a viewport
  // crossing re-arms the key policy without remounting the input, clearing
  // the draft, or touching the saved preference.
  const isPhone = useIsPhone();
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  // The text-width mirror: a hidden `white-space: pre` twin whose offsetWidth
  // is the unwrapped width of the widest line — the desktop's
  // `measured_text_width` (composer.rs:1996). Measuring the TEXT WIDTH (never
  // the post-flip scrollHeight, which differs per mode and would feed back
  // into the decision) is what makes the flip layout-stable.
  const measureRef = useRef<HTMLDivElement | null>(null);
  // The paperclip lives in the actions cluster (composer.rs), so the strip
  // hands its picker up here rather than drawing its own attach button.
  const attachRef = useRef<(() => void) | null>(null);
  const lastChatIdRef = useRef(chat.id);
  // Focus returns to the draft after the native file dialog closes (both
  // Attach and Cancel — the web's cancelled input fires no event, so the
  // window regaining focus is the signal, composer.rs::open_file_picker).
  const focusPendingRef = useRef(false);
  // Interrupts in flight, idempotent per chat (composer.rs:6707-6741).
  const interruptingRef = useRef<Set<string>>(new Set());
  // The pickers' open state — the pill's mouse-down focus defers to open
  // menus (composer.rs:7701-7709).
  const [pickersOpen, setPickersOpen] = useState(false);

  // ── Completions + wizard (ticket 14, composer.rs 3902-3991/5884) ──────
  // The live caret/selection, tracked through onChange/onSelect — the token
  // machines read it exactly as the desktop's `on_input_edited` does on
  // `Edited | CursorMoved`.
  const [selection, setSelectionState] = useState<[number, number]>([0, 0]);
  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  const pendingCaretRef = useRef<number | null>(null);
  const [inputFocused, setInputFocused] = useState(false);
  // While an IME composition is active the raw textarea text paints (the
  // mirror would hide the marked text); the token machines pause.
  const [composing, setComposing] = useState(false);
  const composingRef = useRef(composing);
  composingRef.current = composing;

  const [mention, setMention] = useState<MentionUiState>({
    token: null,
    results: [],
    active: null,
    loading: false,
    error: null,
  });
  const mentionRef = useRef(mention);
  mentionRef.current = mention;
  const mentionRequestRef = useRef(0);
  const mentionSearchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mentionDismissedRef = useRef<{ start: number; end: number; text: string } | null>(null);

  const [slash, setSlash] = useState<SlashUiState>({
    token: null,
    filtered: [],
    active: null,
    harness: null,
    loading: false,
    error: null,
  });
  const slashRef = useRef(slash);
  slashRef.current = slash;
  const slashRequestRef = useRef(0);
  const slashDismissedRef = useRef<{ start: number; end: number; text: string } | null>(null);
  /** `slash_cache` (composer.rs:4026): one ListCommands per harness per
   * composer lifetime; filtering is local per keystroke. */
  const slashCacheRef = useRef<SlashCache>(new Map());

  // The wizard: one instance + a render generation (the Wizard mutates in
  // place, like the desktop's entity); `answered_requests` and the latch's
  // safety-net tick force re-checks of the lifecycle effect.
  const wizardRef = useRef<Wizard | null>(null);
  const [wizardGen, setWizardGen] = useState(-1);
  const [answeredGen, setAnsweredGen] = useState(0);
  const answeredRef = useRef<Set<string>>(new Set());
  const advanceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const safetyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // The mention path tooltip: the ported phase machine + its 420ms timer.
  const [tooltipPhase, setTooltipPhaseState] = useState<MentionTooltipPhase>({ kind: "hidden" });
  const tooltipPhaseRef = useRef(tooltipPhase);
  tooltipPhaseRef.current = tooltipPhase;
  const tooltipGenRef = useRef(0);
  const tooltipTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const tooltipElRef = useRef<HTMLDivElement | null>(null);
  const mirrorRef = useRef<HTMLDivElement | null>(null);
  const wizardPanelRef = useRef<HTMLDivElement | null>(null);

  const wizardActive = wizardGen >= 0 && wizardRef.current !== null;
  const wizardActiveRef = useRef(wizardActive);
  wizardActiveRef.current = wizardActive;

  // The transcript snapshot the wizard's latch reads (one store per chat —
  // the same stream the transcript view renders).
  const transcriptSnapshot = useSyncExternalStore(
    useCallback(
      (listener: () => void) => (transcript === null ? () => {} : transcript.subscribe(listener)),
      [transcript],
    ),
    useCallback(() => transcript?.getSnapshot() ?? null, [transcript]),
    useCallback(() => transcript?.getSnapshot() ?? null, [transcript]),
  );

  // Live status: the desktop's `run_live` is Working OR AwaitingInput.
  const statusRow = snapshot.statuses.rows.find((row) => row.chatId === chat.id);
  const indicator = effectiveIndicator(statusRow, now);
  const runLive = indicator === "working" || indicator === "awaitingInput";

  // The catalog re-renders this component too (the draft-seeding effects
  // read live lists; the pickers child subscribes on its own).
  const harnesses = useSyncExternalStore(
    useCallback((listener: () => void) => catalog.subscribe(listener), [catalog]),
    useCallback(() => catalog.getHarnesses(), [catalog]),
    useCallback(() => catalog.getHarnesses(), [catalog]),
  );

  const [text, setText] = useState(() => chatDrafts.get(chat.id));
  // The flip decision reads the live text through a ref (the evaluate pass
  // stays identity-stable so the ResizeObserver never re-binds).
  const textRef = useRef(text);
  textRef.current = text;
  // The chip projection: rebuilt per text change (the zero-link fast path
  // is `mentions.length === 0`, same as the desktop's empty projection).
  const projection = useMemo(() => new TextProjection(text), [text]);
  const projectionRef = useRef(projection);
  projectionRef.current = projection;
  const mentionsActive = projection.mentions.length > 0;
  const [draft, setDraft] = useState<DraftConfig>(() => {
    // A fresh chat seeds the sticky picks — the remembered harness and its
    // remembered model (pickers.rs:713-745, the native new-chat resolution)
    // — plus the remembered last-used reasoning as its preference layer
    // (pickers.rs:762-775); an established chat replays its config. The
    // models list follows the seeded harness so a loaded catalog pre-selects
    // the remembered model outright (an empty one defers to the model
    // reconciliation, which seeds it once the list lands).
    const rememberedHarness = chat.config === null ? composerDefaults.getSnapshot().harness : null;
    return draftFromChat(
      chat,
      harnesses.rows,
      catalog.getModels(chat.config?.harness ?? rememberedHarness ?? "claude-code").rows,
      composerDefaults.getSnapshot().reasoning,
      rememberedHarness !== null
        ? { harness: rememberedHarness, model: rememberedModelFor(rememberedHarness) }
        : null,
    );
  });
  const models = useSyncExternalStore(
    useCallback((listener: () => void) => catalog.subscribeModels(draft.harness, listener), [catalog, draft.harness]),
    useCallback(() => catalog.getModels(draft.harness), [catalog, draft.harness]),
    useCallback(() => catalog.getModels(draft.harness), [catalog, draft.harness]),
  );
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  // ── Ticket 15: the new-thread canvas branch ─────────────────────────────
  // `new_chat = selected_chat.is_none()` (composer.rs:7286) — the web's
  // canvas chat is the "" stub, so `expanded = expanded_mode || new_chat`
  // (composer.rs:7488): the canvas ALWAYS renders expanded regardless of the
  // flip state, and a mode flip there never commits (7284-7300 — auto-grow
  // still morphs; only the compact↔expanded flip is suppressed).
  const newChat = chat.id === "";
  // Route coordination must not force an established thread into the
  // two-row layout: `dock_height`'s session side reads the composer's OWN
  // expanded state, never the forced one.
  const sessionExpanded = expanded;
  // Staged attachments per chat id — survives a chat switch (the strip
  // moves with the chat). Empty for a chat the user has never staged on.
  const [stagedByChat, setStagedByChat] = useState<Record<string, readonly StagedAttachment[]>>({});
  // Staged review comments ride the same per-chat keying (state.rs:688's
  // `composer_key`) — one shared store, read here for the chip, the send
  // fold-in, and the content gate (a staged comment alone is a legal send,
  // composer.rs:505).
  const stagedComments = useReviewComments(chat.id);
  const commentCount = stagedComments.comments.length;
  // The failure notice (composer.rs:7309-7411). Chat-scoped failures
  // survive navigation and only render under their own chat.
  const [failure, setFailure] = useState<FailureNotice | null>(null);
  const staged = stagedByChat[chat.id] ?? [];

  // ── Per-chat drafts (composer.rs `drafts: HashMap<chat_key, String>`) ──
  // Swap on navigation: save the outgoing chat's text, load the incoming
  // one's. The route snap armed here keeps the first flip after a switch
  // un-animated (composer.rs:7290, ROUTE_SNAP_MS).
  const routeSnapUntilRef = useRef<number | null>(null);
  useEffect(() => {
    if (lastChatIdRef.current === chat.id) {
      return;
    }
    chatDrafts.set(lastChatIdRef.current, textRef.current);
    lastChatIdRef.current = chat.id;
    // A programmatic draft swap is a new document: the full value assignment
    // resets the browser's own undo stack (the desktop's `set_text` clears
    // its stacks — the accepted undo-coalescing divergence).
    setText(chatDrafts.get(chat.id));
    setExpanded(false);
    setFailure(null);
    routeSnapUntilRef.current = performance.now() + ROUTE_SNAP_MS;
  }, [chat.id]);

  // When the edit row changes (the chat page started/cancelled editing a
  // queued row), seed the textarea with the row's text so the user can type
  // a replacement; the pre-edit draft (text + staged attachments) is
  // restored when the lease closes (composer.rs `clear_queue_edit_local`'s
  // `queue_edit_draft` hand-back, queue.rs:1394-1398/1471-1476).
  const lastEditingIdRef = useRef<string | null>(null);
  const preEditTextRef = useRef<string | null>(null);
  const preEditStagedRef = useRef<readonly StagedAttachment[] | null>(null);
  useEffect(() => {
    const id = editingMessage?.id ?? null;
    if (id === lastEditingIdRef.current) {
      return;
    }
    lastEditingIdRef.current = id;
    if (editingMessage !== null && editingMessage !== undefined) {
      preEditTextRef.current = textRef.current;
      preEditStagedRef.current = staged;
      // The row's own attachments take the strip while editing
      // (`begin_queue_edit`'s loaded set, queue.rs:1399) — staged from the
      // shared attachment cache, which the chat page pre-loads before
      // opening the edit. A cache miss skips that one (the commit still
      // preserves it engine-side; see ticket 16's Comments).
      const deviceId = session.client.engineInfo?.deviceId ?? null;
      const seeded =
        editingMessage.attachments
          ?.map((path) => stagedFromCache(deviceId, path))
          .filter((att): att is StagedAttachment => att !== null) ?? [];
      setStagedByChat((current) => {
        const next = { ...current };
        if (seeded.length === 0) {
          delete next[chat.id];
        } else {
          next[chat.id] = seeded;
        }
        return next;
      });
      setText(editingMessage.text);
    } else {
      if (preEditTextRef.current !== null) {
        setText(preEditTextRef.current);
        preEditTextRef.current = null;
      }
      const restore = preEditStagedRef.current;
      if (restore !== null) {
        preEditStagedRef.current = null;
        setStagedByChat((current) => {
          const next = { ...current };
          if (restore.length === 0) {
            delete next[chat.id];
          } else {
            next[chat.id] = restore;
          }
          return next;
        });
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editingMessage, chat.id]);

  // Reconcile the draft with chat.config + the loaded catalog (locked once
  // persisted; sticky defaults otherwise — pickers.rs:713-796).
  useEffect(() => {
    const persisted = chat.config;
    if (persisted !== null) {
      setDraft((current) =>
        current.harness === persisted.harness &&
        current.model === persisted.model &&
        current.reasoning === persisted.reasoning &&
        current.sandbox === persisted.sandbox
          ? current
          : {
              harness: persisted.harness,
              model: persisted.model,
              reasoning: persisted.reasoning,
              sandbox: persisted.sandbox,
              modelOptions: { ...(persisted.modelOptions ?? {}) },
            },
      );
      return;
    }
    if (harnesses.rows.length === 0) {
      return;
    }
    setDraft((current) => {
      if (harnesses.rows.some((h: HarnessDescriptor) => h.id === current.harness)) {
        return current;
      }
      const remembered = composerDefaults.getSnapshot().harness;
      const offered = offeredHarnesses(harnesses.rows);
      const next =
        remembered !== null && harnesses.rows.some((h: HarnessDescriptor) => h.id === remembered)
          ? remembered
          : offered[0]?.id ?? harnesses.rows[0]?.id;
      if (next === undefined) {
        return current;
      }
      return {
        harness: next,
        model: null,
        // A corrected harness keeps the remembered level as the preference
        // layer (native falls back to it via effective_reasoning); the
        // reconciliation below re-derives it against the new harness's
        // effective ladder once models resolve.
        reasoning: composerDefaults.getSnapshot().reasoning,
        sandbox: "workspace-write",
        modelOptions: {},
      };
    });
  }, [chat.config, harnesses.rows]);

  // Once a harness is picked, ensure the model catalog is loaded and seed
  // the draft with the remembered model (pickers.rs:748-796).
  useEffect(() => {
    if (!harnesses.loaded) {
      return;
    }
    if (harnesses.rows.length === 0) {
      return;
    }
    void catalog.loadModels(draft.harness);
  }, [catalog, draft.harness, harnesses.loaded, harnesses.rows.length]);

  // Model/descriptor reconciliation: seed the draft's model and re-derive
  // reasoning against the EFFECTIVE ladder (model levels when nonempty, else
  // the matching descriptor's) — observing BOTH live inputs so a descriptor
  // that lands after the models re-resolves the selection instead of leaving
  // a stale model-only clamp behind. The extracted owner carries the logic;
  // it returns the prior draft when nothing changed (no setState loop).
  useDraftModelReconciliation(models.rows, harnesses.rows, setDraft);

  // ── The width-driven flip + height morph ───────────────────────────────
  //
  // One layout pass per effect run / textarea resize: measure the unwrapped
  // text width against the compact-mode wrap capacity (learned while
  // compact, shifted by the container delta while expanded — never the
  // post-flip measured width), decide the flip, then advance the pill's
  // height toward the live target through the morph state machine. The
  // rAF loop re-runs the pass while a morph is in flight.
  const [layout, setLayout] = useState<PillLayout>(REST_LAYOUT);
  const [tick, setTick] = useState(0);
  const [reducedMotion, setReducedMotion] = useState(prefersReducedMotion);
  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReducedMotion(query.matches);
    onChange();
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  const epochRef = useRef(0);
  const flipEpochRef = useRef(0);
  const compactCapacityRef = useRef(0);
  const expandedAnchorRef = useRef(0);
  const lastSeenWidthRef = useRef(0);
  const widthChangedAtRef = useRef<number | null>(null);
  const settleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const heightMorphRef = useRef<FlipMorph | null>(null);
  const flipMorphRef = useRef<FlipMorph | null>(null);
  // `model_handoff_*` (composer.rs:4136-4138, 7728-7732): the chip's handoff
  // phase — the position, the value captured when the flip morph last
  // (re)started, and the morph identity it was captured against (a fresh
  // arm is a new object, a cleared one is null — identity IS the comparison).
  const modelHandoffPositionRef = useRef(1);
  const modelHandoffFromRef = useRef(1);
  const modelHandoffMorphRef = useRef<FlipMorph | null>(null);
  const lastTargetRef = useRef(0);
  const lastRenderedRef = useRef(0);
  const rafRef = useRef<number | null>(null);
  const availableWidthRef = useRef<number | null>(null);
  const evaluateRef = useRef<() => void>(NOOP);
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  // The dock frame + the canvas flags, read through refs so the evaluate
  // pass stays identity-stable (the ResizeObserver never re-binds).
  const dockFrameRef = useRef<DockFrame | null>(dockFrame);
  dockFrameRef.current = dockFrame;
  const newChatRef = useRef(newChat);
  newChatRef.current = newChat;
  const sessionExpandedRef = useRef(sessionExpanded);
  sessionExpandedRef.current = sessionExpanded;
  const lastDockAmountRef = useRef<number | null>(null);
  const stagedCountRef = useRef(staged.length);
  stagedCountRef.current = staged.length;
  const commentCountRef = useRef(commentCount);
  commentCountRef.current = commentCount;
  const reducedMotionRef = useRef(reducedMotion);
  reducedMotionRef.current = reducedMotion;
  const availableWidthRef2 = useRef(availableWidth);
  availableWidthRef2.current = availableWidth;
  // The glide's channels write these directly (ticket 57b heights, ticket 74
  // inner geometry): the pill root, the input box and the actions row carry
  // their animated height/radius/padding/offset values as CSS custom
  // properties — the JSX consumes them with a stale-safe fallback, so a
  // mid-glide render cannot clobber the live values.
  const pillRef = useRef<HTMLDivElement | null>(null);
  const inputBoxRef = useRef<HTMLDivElement | null>(null);
  const actionsRef = useRef<HTMLDivElement | null>(null);
  // The model chip's handoff slot — measured per pass (the desktop's
  // `model_bounds` canvas) and carrying its glide vars while the dock drives.
  const modelSlotRef = useRef<HTMLDivElement | null>(null);
  // The morph loop's liveness, render-tracked: a glide that starts mid-morph
  // must still publish the loop's death (the evaluate otherwise skips the
  // state publish while the dock owns the height, which would leave the
  // composer's own rAF clock ticking through the whole glide).
  const morphLoopLiveRef = useRef(false);
  morphLoopLiveRef.current = layout.morphing;
  // The pump's evaluate channel (ticket 57b): the page's rAF loop calls the
  // parked pass once per frame — every input is a ref, so the identity the
  // pump holds stays valid however stale the closure.
  if (dockEvaluateRef !== undefined) {
    dockEvaluateRef.current = () => evaluateRef.current();
  }

  evaluateRef.current = () => {
    const el = textareaRef.current;
    const mirror = measureRef.current;
    if (el === null || mirror === null) {
      return;
    }
    // While the wizard borrows the input the pill is not rendered; the flip
    // machinery stands down and the wizard's own auto-grow owns the height.
    if (wizardActiveRef.current) {
      return;
    }
    const nowMs = performance.now();
    epochRef.current += 1;
    const epoch = epochRef.current;
    // Content measurement: the textarea carries no vertical padding of its
    // own (the box does), so an `auto` height reads the wrapped content.
    el.style.height = "auto";
    const wrappedLines = Math.max(1, Math.round(el.scrollHeight / INPUT_LINE_HEIGHT));
    const contentHeight = wrappedLines * INPUT_LINE_HEIGHT;
    const textWidth = mirror.offsetWidth;
    const hasNewline = textRef.current.includes("\n");
    const lastWidth = el.offsetWidth;
    // Only measurements taken *after* the last flip may drive the next one
    // (at most one flip per layout pass — a flip invalidates the widths).
    const measured = epoch > flipEpochRef.current && lastWidth > 0;
    if (measured) {
      // A same-mode width change is an interactive window/pane resize:
      // defer collapse until sizes settle. The last-seen reset on a
      // committed flip means the mode change's width jump is NOT read as
      // one.
      if (
        lastSeenWidthRef.current > 0 &&
        Math.abs(lastWidth - lastSeenWidthRef.current) > COMPOSER_WIDTH_EPSILON
      ) {
        widthChangedAtRef.current = nowMs;
      }
      lastSeenWidthRef.current = lastWidth;
      if (expandedRef.current) {
        if (expandedAnchorRef.current <= 0) {
          expandedAnchorRef.current = lastWidth;
        }
      } else {
        // The compact pill's content box is the layout-stable capacity
        // both thresholds measure against (composer.rs:7229-7232; the
        // web's textarea content width already excludes its paddings, so
        // no extra inset is subtracted).
        compactCapacityRef.current = lastWidth;
      }
    }
    const resizing =
      widthChangedAtRef.current !== null && nowMs - widthChangedAtRef.current < RESIZE_SETTLE_MS;
    if (resizing && settleTimerRef.current === null) {
      // Re-evaluate once the settle window has passed (composer.rs:7237).
      settleTimerRef.current = setTimeout(() => {
        settleTimerRef.current = null;
        evaluateRef.current();
      }, RESIZE_SETTLE_MS + 20);
    }
    // Layout-stable compact capacity: measured directly while compact;
    // while expanded, the learned value shifted by the container resize.
    const capacity = !expandedRef.current
      ? lastWidth > 0
        ? lastWidth
        : Number.POSITIVE_INFINITY
      : compactCapacityRef.current > 0
        ? expandedAnchorRef.current > 0 && lastWidth > 0
          ? compactCapacityRef.current + (lastWidth - expandedAnchorRef.current)
          : compactCapacityRef.current
        : Number.POSITIVE_INFINITY;
    const nextMode = composerFlip(expandedRef.current, textWidth, capacity, hasNewline, resizing);
    // A mode flip on the new-thread canvas is never committed (composer.rs:7284-7300):
    // the canvas is always expanded, so the compact↔expanded flip machinery
    // stands down there — auto-grow (the height morph below) still runs.
    const committed = !newChatRef.current && nextMode !== expandedRef.current && measured;
    const mode = newChatRef.current ? true : committed ? nextMode : expandedRef.current;
    if (committed) {
      flipEpochRef.current = epoch;
      expandedAnchorRef.current = 0;
      lastSeenWidthRef.current = 0;
      setExpanded(nextMode);
    }
    // `strip_width_hint` (composer.rs:7511): the pill's content width, in
    // both modes. Ticket 64 §2.4: the page's LIVE clamped target leads (the
    // column observer feeds it per tick, even while the published
    // `availableWidth` prop defers); the prop-parked ref is the fallback.
    const stripWidthHint =
      (liveAvailableWidth?.current ?? availableWidthRef2.current ?? COMPOSER_MAX_WIDTH) - 2 * 16 - 2;
    // `comment_strip_height` (composer.rs:7524): the comments chip rides the
    // same arithmetic strip budget as the attachments.
    const stripH =
      attachmentStripHeight(stagedCountRef.current, stripWidthHint) +
      commentStripHeight(commentCountRef.current);
    // `dock_height` (composer.rs:7489-7500): with a dock frame installed the
    // shared clock owns the pill's height across the route; without one the
    // mode's own height applies. The LIVE frame during a glide (the pump
    // wrote it before invoking this pass — ticket 57b), else the prop-parked
    // one; they agree whenever nothing is in flight.
    const frame =
      liveDockFrame !== undefined && liveDockFrame.current !== null
        ? liveDockFrame.current
        : dockFrameRef.current;
    const dockDriven = frame !== null && frame.active;
    const dockAmount = frame !== null ? Math.min(Math.max(frame.amount, 0), 1) : 0;
    const baseHeight =
      frame !== null
        ? dockHeight(dockAmount, contentHeight, sessionExpandedRef.current)
        : mode
          ? composerTotalHeight(contentHeight)
          : COMPACT_TOTAL_HEIGHT;
    const target = baseHeight + stripH;
    const routeSnap = routeSnapUntilRef.current !== null && nowMs < routeSnapUntilRef.current;
    // Two morphs, as on the desktop: the HEIGHT morph animates the pill
    // toward the live target (auto-grow retargets mid-flight), the FLIP
    // morph drives the inner geometry handoff (paddings, insets, glide).
    heightMorphRef.current = flipMorphStep(
      heightMorphRef.current,
      Math.abs(target - lastTargetRef.current) > 0.5,
      lastRenderedRef.current,
      nowMs,
      reducedMotionRef.current,
      routeSnap,
    );
    flipMorphRef.current = flipMorphStep(
      flipMorphRef.current,
      committed,
      lastRenderedRef.current,
      nowMs,
      reducedMotionRef.current,
      routeSnap,
    );
    // `set_dock_frame` (composer.rs:4140-4149): an ACTIVE frame kills both
    // morphs (the shared clock owns the height), and any change in the
    // frame's `amount` sets `dock_height_changed`, which suppresses both
    // morphs for that frame. Settled at the destination, typing flips keep
    // their local clock.
    if (frame !== null && (frame.active || frame.amount !== lastDockAmountRef.current)) {
      heightMorphRef.current = null;
      flipMorphRef.current = null;
    }
    lastDockAmountRef.current = frame !== null ? frame.amount : null;
    lastTargetRef.current = target;
    const heightMorph = heightMorphRef.current;
    const pillHeight = heightMorph !== null ? flipMorphHeight(heightMorph, target, nowMs) : target;
    const flipMorph = flipMorphRef.current;
    const morphT =
      flipMorph !== null && !flipMorphDone(flipMorph, nowMs) ? flipMorphProgress(flipMorph, nowMs) : 1;
    lastRenderedRef.current = pillHeight;
    // `dock_clearance_correction` (composer.rs:7583-7589): the shell reserves
    // the DESTINATION footprint (docked?1:0), never the animated height, so
    // the transcript's clearance does not pump during the route change.
    if (dockCorrectionRef !== undefined) {
      dockCorrectionRef.current =
        frame !== null
          ? dockHeight(frame.docked ? 1 : 0, contentHeight, sessionExpandedRef.current) + stripH - pillHeight
          : 0;
    }
    // The frame's inner geometry (ticket 74, composer.rs:7589-7632): ONE
    // clock drives every inner channel — while the dock frame is active and
    // the session's own mode is compact, that's the shared dock amount
    // (`1 − amount` expanded, `amount` compact); otherwise the local flip's.
    // The compact route floors keep one input line plus its padding alive as
    // the pill sweeps down to 49px; an expanded destination morphs exactly
    // like a typing flip.
    const flipAnimating = flipMorph !== null && !flipMorphDone(flipMorph, nowMs);
    const geometry = routeInputGeometry({
      renderedExpanded: mode,
      sessionExpanded: sessionExpandedRef.current,
      dockActive: dockDriven,
      dockAmount,
      flipProgress: morphT,
      flipFrom: flipAnimating ? flipMorph.from : null,
      pillHeight,
      baseHeight,
      stripHeight: stripH,
      // `dock_height(0.0)` (composer.rs:7794) — the undocked hero height,
      // the route glide's `from` (`lerp` at amount 0 ignores the session).
      undockedHeight: dockHeight(0, contentHeight, sessionExpandedRef.current),
    });
    const { boxHeight, textPad, inputHeight } = geometry;
    // e0c1e936's model handoff (composer.rs:7726-7769): the chip fades
    // between its two horizontal anchors on the SAME clock as the height
    // morph — the position rides the dock amount on a compact route, else
    // lerps from the phase captured when the flip morph last (re)started
    // through EASE_IN_OUT over the morph's RAW timeline, so reversals
    // continue from the current phase instead of restarting. The offset
    // then lands the invisible mid-flip relocation against the measured
    // slot distance (`model_travel`).
    if (modelHandoffMorphRef.current !== flipMorphRef.current) {
      modelHandoffFromRef.current = modelHandoffPositionRef.current;
      modelHandoffMorphRef.current = flipMorphRef.current;
    }
    const modelCompactTarget = mode ? 0 : 1;
    const handoffPosition = modelHandoffPosition({
      from: modelHandoffFromRef.current,
      compactTarget: modelCompactTarget,
      morph: flipMorphRef.current,
      dockActive: dockDriven,
      sessionExpanded: sessionExpandedRef.current,
      dockAmount,
      nowMs,
    });
    modelHandoffPositionRef.current = handoffPosition;
    const surfaceWidth = pillRef.current?.offsetWidth ?? (stripWidthHint + PILL_BORDER_V);
    const modelWidth = modelSlotRef.current?.offsetWidth ?? 0;
    const modelSlot = modelSlotOffset(
      handoffPosition,
      modelCompactTarget,
      modelTravel(surfaceWidth, modelWidth, geometry.clusterInset),
    );
    el.style.height = `${inputHeight}px`;
    // The scrollability gate, not an inline overflowY: the CSS owns the
    // overflow (`[data-scrollable="true"]` → `overflow-y: auto`, bar
    // hidden), so the overflowing input still wheel-scrolls while a
    // non-overflowing one chains its wheel to the transcript
    // (`on_scroll_wheel`, composer.rs:2898-2933).
    el.dataset["scrollable"] = mode && contentHeight > inputHeight ? "true" : "false";
    // The scroll fade mask: only SETTLED overflow at an edge gets the ramp —
    // the settled viewport is the committed target's, not the animating
    // box's (`input_overflow_edges`, composer.rs:181-192).
    const settledViewport = geometry.settledViewport;
    const [fadeTop, fadeBottom] = inputOverflowEdges(contentHeight, settledViewport, inputHeight, el.scrollTop);
    el.dataset["fadeTop"] = mode && fadeTop ? "true" : "false";
    el.dataset["fadeBottom"] = mode && fadeBottom ? "true" : "false";
    const morphing = (heightMorph !== null && !flipMorphDone(heightMorph, nowMs)) || flipAnimating;
    if (dockDriven) {
      // The glide's channels are DOM writes (ticket 57b, §2.3; ticket 74 for
      // the inner ones): the pill's outer height, its radius
      // (`26 − 4·dock_amount`, composer.rs:7603 — the frost blur's mask
      // follows the radius), the box height, and the ROUTE-CLOCK inner
      // values (text padding, cluster offset/inset, compact text glide) all
      // ride CSS custom properties the JSX below consumes with a stale-safe
      // fallback — a single active frame must not mix the live heights with
      // last publish's padding (composer.rs:7592-7632 derives every channel
      // from the same frame). All are written in BOTH modes so a mid-glide
      // mode switch never catches a channel missing. The settle evaluate's
      // republish (the non-driven branch) plus the `[layout]` effect's var
      // removal hand the values back to the state.
      pillRef.current?.style.setProperty("--rb-dock-pill-height", `${pillHeight}px`);
      pillRef.current?.style.setProperty("--rb-dock-pill-radius", `${(26 - 4 * dockAmount).toFixed(2)}px`);
      inputBoxRef.current?.style.setProperty("--rb-dock-box-height", `${boxHeight}px`);
      inputBoxRef.current?.style.setProperty("--rb-dock-text-pad", `${geometry.textPad}px`);
      inputBoxRef.current?.style.setProperty("--rb-dock-text-glide", `${-geometry.textGlide}px`);
      // The cluster dy lives on the PILL: the actions row AND the detached
      // paperclip (a body sibling of the row) both consume it, so it must
      // ride an ancestor they share (composer.rs:7805/7864 — every control
      // wrapper carries the same `top: -cluster_dy`).
      pillRef.current?.style.setProperty("--rb-dock-cluster-dy", `${-geometry.clusterDy}px`);
      actionsRef.current?.style.setProperty("--rb-dock-cluster-inset", `${geometry.clusterInset}px`);
      modelSlotRef.current?.style.setProperty("--rb-dock-model-left", `${modelSlot.left}px`);
      modelSlotRef.current?.style.setProperty("--rb-dock-model-opacity", `${modelSlot.opacity}`);
      if (!morphing && !morphLoopLiveRef.current) {
        // A pure glide frame: the imperative writes above (plus the
        // textarea height and the datasets already applied) carry
        // everything — skip the state publish so the glide runs ZERO React
        // frames. A glide that caught a live morph still publishes once,
        // landing the loop's death (`morphing` false) so the composer's own
        // rAF clock stops.
        return;
      }
    }
    // Ticket 64 §2.4's unchanged-layout bailout: publish only when a field
    // actually moved — the observer/evaluate cadence may run freely, and an
    // identical resolution returns the PRIOR state (no React publish). Real
    // height/mode/morph changes still land: every PillLayout field compared.
    const nextLayout: PillLayout = {
      pillHeight,
      boxHeight,
      textPad,
      clusterInset: geometry.clusterInset,
      clusterDy: geometry.clusterDy,
      // Collapse/route text glide: the decaying offset walks the compact
      // text down from its expanded resting place (composer.rs:7793-7800).
      textGlide: geometry.textGlide,
      modelLeft: modelSlot.left,
      modelOpacity: modelSlot.opacity,
      morphing,
    };
    setLayout((previous) => (pillLayoutEquals(previous, nextLayout) ? previous : nextLayout));
  };

  useLayoutEffect(() => {
    evaluateRef.current();
    // The dock's amount moves the pill height every frame of the route
    // glide — re-run the pass per frame change, not only per text/width
    // change (the desktop recomputes per render). The per-FRAME driver is
    // the page's pump (it invokes the pass through `dockEvaluateRef`,
    // ticket 57b); the prop change re-runs it at the discrete publishes
    // (the flip, a chrome mount crossing, the settle).
  }, [text, expanded, availableWidth, staged.length, commentCount, tick, dockFrame?.amount, dockFrame?.active]);

  // The glide vars fall with the state publish: the commit that republishes
  // `layout` while the dock is no longer driving re-applies the JSX
  // fallbacks settled, and removing the vars in the SAME commit (pre-paint)
  // keeps the pill from flashing the pre-glide fallback. During a glide no
  // publish lands (the evaluate's dock branch skips it), so the vars
  // survive the whole glide by construction.
  useLayoutEffect(() => {
    const frame = liveDockFrame !== undefined ? liveDockFrame.current : null;
    if ((frame ?? dockFrameRef.current)?.active !== true) {
      pillRef.current?.style.removeProperty("--rb-dock-pill-height");
      pillRef.current?.style.removeProperty("--rb-dock-pill-radius");
      pillRef.current?.style.removeProperty("--rb-dock-cluster-dy");
      inputBoxRef.current?.style.removeProperty("--rb-dock-box-height");
      inputBoxRef.current?.style.removeProperty("--rb-dock-text-pad");
      inputBoxRef.current?.style.removeProperty("--rb-dock-text-glide");
      actionsRef.current?.style.removeProperty("--rb-dock-cluster-inset");
      modelSlotRef.current?.style.removeProperty("--rb-dock-model-left");
      modelSlotRef.current?.style.removeProperty("--rb-dock-model-opacity");
    }
    // `dockFrameRef` is render-assigned; the live ref is pump-owned. This
    // effect only needs to run when a publish landed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout]);

  // The rAF loop: keep frames coming while a morph is in flight (the
  // desktop's `window.request_animation_frame`, shell.rs `motion_active`).
  useEffect(() => {
    if (!layout.morphing) {
      return;
    }
    rafRef.current = requestAnimationFrame(() => setTick((value) => value + 1));
    return () => {
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
    };
  }, [layout.morphing, tick]);

  // The desktop re-evaluates per layout pass — on the web only a
  // conversation-column resize moves the textarea's width mid-keystroke.
  useEffect(() => {
    const el = textareaRef.current;
    if (el === null) {
      return;
    }
    const observer = new ResizeObserver(() => evaluateRef.current());
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // The shell's available-width feed (composer.rs::set_available_width):
  // only an epsilon-exceeding move of the CLAMPED width re-evaluates.
  useEffect(() => {
    const width = Math.min(Math.max(availableWidth ?? 0, 0), COMPOSER_MAX_WIDTH);
    if (!composerWidthChanged(availableWidthRef.current, width)) {
      return;
    }
    availableWidthRef.current = width;
    evaluateRef.current();
  }, [availableWidth]);

  useEffect(
    () => () => {
      if (settleTimerRef.current !== null) {
        clearTimeout(settleTimerRef.current);
      }
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
      }
    },
    [],
  );

  // ── Drag-selection autoscroll (composer.rs:270-284) ────────────────────
  // A native textarea does not autoscroll on drag past its edge: drive
  // `el.scrollTop` from a pointermove listener while the primary button is
  // down, at the 16ms cadence, by the edge-proportional capped delta.
  useEffect(() => {
    const el = textareaRef.current;
    if (el === null) {
      return;
    }
    let timer: ReturnType<typeof setInterval> | null = null;
    const onPointerMove = (event: PointerEvent): void => {
      if (event.buttons !== 1) {
        if (timer !== null) {
          clearInterval(timer);
          timer = null;
        }
        return;
      }
      const bounds = el.getBoundingClientRect();
      if (event.clientY >= bounds.top && event.clientY <= bounds.bottom) {
        if (timer !== null) {
          clearInterval(timer);
          timer = null;
        }
        return;
      }
      const delta = inputDragScrollDelta(event.clientY, bounds.top, bounds.bottom, INPUT_LINE_HEIGHT);
      if (timer !== null || delta === 0) {
        return;
      }
      timer = setInterval(() => {
        el.scrollTop = Math.min(Math.max(el.scrollTop - delta, 0), el.scrollHeight);
      }, 16);
    };
    const stop = (): void => {
      if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    };
    el.addEventListener("pointermove", onPointerMove);
    el.addEventListener("pointerup", stop);
    el.addEventListener("pointercancel", stop);
    return () => {
      stop();
      el.removeEventListener("pointermove", onPointerMove);
      el.removeEventListener("pointerup", stop);
      el.removeEventListener("pointercancel", stop);
    };
  }, []);

  // ── Interrupt release (composer.rs:5771-5781) ──────────────────────────
  // The pending set releases only when the chat settles.
  useEffect(() => {
    retainLiveInterrupts(interruptingRef.current, (chatId) => {
      const row = snapshot.statuses.rows.find((entry) => entry.chatId === chatId);
      const live = effectiveIndicator(row, now);
      return live === "working" || live === "awaitingInput";
    });
  }, [snapshot, now]);

  // ── Atomic chip editing (TextProjection, composer.rs:1158-1262) ────────
  // A programmatic edit applies text + caret through the controlled value;
  // the caret lands after the re-render via `pendingCaretRef`.
  const applyEdit = useCallback(
    (start: number, end: number, replacement: string, caretAfter: number): void => {
      const current = textRef.current;
      setText(current.slice(0, start) + replacement + current.slice(end));
      pendingCaretRef.current = caretAfter;
    },
    [],
  );
  useLayoutEffect(() => {
    const el = textareaRef.current;
    const target = pendingCaretRef.current;
    if (el === null || target === null) {
      return;
    }
    pendingCaretRef.current = null;
    el.setSelectionRange(target, target);
    setSelectionState([target, target]);
  }, [text]);

  /** Selection normalization through `normalize_range` — the atomic caret
   * contract enforced on every native selection change (clicks, arrows,
   * word motion): a caret inside a chip snaps to the nearer edge, a
   * selection overlapping a chip swallows it whole. */
  const normalizeSelection = useCallback((): void => {
    const el = textareaRef.current;
    if (el === null) {
      return;
    }
    const projectionNow = projectionRef.current;
    if (projectionNow.mentions.length === 0) {
      return;
    }
    const start = el.selectionStart ?? 0;
    const end = el.selectionEnd ?? 0;
    const next = projectionNow.normalizeRange(start, end);
    if (next.start !== start || next.end !== end) {
      el.setSelectionRange(next.start, next.end, el.selectionDirection);
    }
  }, []);

  const onInputSelect = useCallback((): void => {
    normalizeSelection();
    const el = textareaRef.current;
    if (el !== null) {
      setSelectionState([el.selectionStart ?? 0, el.selectionEnd ?? 0]);
    }
  }, [normalizeSelection]);

  const onInputChange = useCallback((event: React.ChangeEvent<HTMLTextAreaElement>): void => {
    setText(event.target.value);
    setSelectionState([event.target.selectionStart ?? 0, event.target.selectionEnd ?? 0]);
  }, []);

  // The mirror scrolls with the textarea; any scroll also invalidates the
  // path tooltip (composer.rs's `invalidate_mention_tooltip` on scroll).
  useEffect(() => {
    const el = textareaRef.current;
    if (el === null) {
      return;
    }
    const onScroll = (): void => {
      if (mirrorRef.current !== null) {
        mirrorRef.current.scrollTop = el.scrollTop;
      }
      invalidateTooltip();
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── Completion state machines (on_input_edited / update_slash) ─────────

  const setTooltipPhase = useCallback((phase: MentionTooltipPhase): void => {
    tooltipPhaseRef.current = phase;
    setTooltipPhaseState(phase);
  }, []);

  /** `invalidate_mention_tooltip` (composer.rs:2040): bump the generation,
   * clear the timer, hide. Any edit, scroll, mouse-down or drag calls it. */
  const invalidateTooltip = useCallback((): void => {
    if (tooltipTimerRef.current !== null) {
      clearTimeout(tooltipTimerRef.current);
      tooltipTimerRef.current = null;
    }
    if (tooltipPhaseRef.current.kind !== "hidden") {
      setTooltipPhase({ kind: "hidden" });
    }
  }, [setTooltipPhase]);

  /** `reset_mention` (composer.rs:4996): tear the whole completion down,
   * bumping the generation so queued RPC replies are dropped. */
  const resetMention = useCallback(
    (dismissed: DismissedToken | null): void => {
      mentionRequestRef.current += 1;
      if (mentionSearchTimerRef.current !== null) {
        clearTimeout(mentionSearchTimerRef.current);
        mentionSearchTimerRef.current = null;
      }
      mentionDismissedRef.current = dismissed;
      invalidateTooltip();
      setMention((current) =>
        current.token === null && current.results.length === 0 && !current.loading && current.error === null
          ? current
          : { token: null, results: [], active: null, loading: false, error: null },
      );
    },
    [invalidateTooltip],
  );

  /** `reset_slash` (composer.rs:5501). */
  const resetSlash = useCallback(
    (dismissed: DismissedToken | null): void => {
      slashRequestRef.current += 1;
      slashDismissedRef.current = dismissed;
      setSlash((current) =>
        current.token === null && current.filtered.length === 0 && !current.loading && current.error === null
          ? current
          : { ...current, token: null, filtered: [], active: null, loading: false, error: null },
      );
    },
    [],
  );

  /** `dismiss_mention`/`dismiss_slash` (composer.rs:5161/5463): record the
   * token (range + text) so the caret may keep moving inside it while the
   * popup stays closed. */
  const dismissCompletion = useCallback((): void => {
    const slashTokenNow = slashRef.current.token;
    if (slashTokenNow !== null) {
      resetSlash({
        start: slashTokenNow.start,
        end: slashTokenNow.end,
        text: textRef.current.slice(slashTokenNow.start, slashTokenNow.end),
      });
      return;
    }
    const mentionTokenNow = mentionRef.current.token;
    if (mentionTokenNow !== null) {
      resetMention({
        start: mentionTokenNow.start,
        end: mentionTokenNow.end,
        text: textRef.current.slice(mentionTokenNow.start, mentionTokenNow.end),
      });
    }
  }, [resetMention, resetSlash]);

  /** `refilter_slash` (composer.rs:5430): the local re-rank for the open
   * token's query. */
  const refilterSlashFor = useCallback((query: string, harness: HarnessId): void => {
    const commands = slashCacheRef.current.get(harness) ?? [];
    const { filtered, active } = refilterSlash(query, commands);
    setSlash((current) => {
      if (
        current.active === active &&
        current.filtered.length === filtered.length &&
        current.filtered.every((value, ix) => value === filtered[ix])
      ) {
        return current;
      }
      return { ...current, filtered, active };
    });
  }, []);

  // `on_input_edited` (composer.rs:5007-5148) + `update_slash`
  // (composer.rs:5344-5427): both token machines run per text/caret change,
  // exactly as the desktop runs them on `Edited | CursorMoved`.
  useEffect(() => {
    if (composingRef.current) {
      return;
    }
    if (wizardActiveRef.current) {
      // A wizard-mounted input never completes (composer.rs:5008-5016).
      if (mentionRef.current.token !== null) {
        resetMention(null);
      }
      if (slashRef.current.token !== null) {
        resetSlash(null);
      }
      return;
    }
    const currentText = text;
    const caretNow = selectionRef.current[0];

    // ── slash: whole-prompt prefixes only ──
    const slashTokenNow = slashToken(currentText, caretNow);
    const slashDismissed = slashDismissedRef.current;
    const slashStillDismissed =
      slashTokenNow !== null &&
      slashDismissed !== null &&
      slashTokenNow.start === slashDismissed.start &&
      slashTokenNow.end === slashDismissed.end &&
      currentText.slice(slashTokenNow.start, slashTokenNow.end) === slashDismissed.text;
    if (slashStillDismissed) {
      setSlash((current) => (current.token === null ? current : { ...current, token: null }));
    } else {
      slashDismissedRef.current = null;
      const harness = draft.harness;
      const harnessChanged = slashRef.current.harness !== harness;
      const sameToken = tokensEqual(slashTokenNow, slashRef.current.token);
      if (!slashStillDismissed && sameToken && !harnessChanged) {
        refilterSlashFor(slashTokenNow === null ? "" : slashTokenNow.query, harness);
      } else {
        const cached = slashCacheRef.current.get(harness);
        setSlash({
          token: slashTokenNow,
          filtered: [],
          active: null,
          harness,
          loading: false,
          error: null,
        });
        if (slashTokenNow === null) {
          // closed
        } else if (cached !== undefined) {
          refilterSlashFor(slashTokenNow.query, harness);
        } else {
          // First open for this harness: ONE ListCommands, targeted like
          // file search (the chat's host device owns the agent binary).
          slashRequestRef.current += 1;
          const request = slashRequestRef.current;
          setSlash((current) => ({ ...current, loading: true }));
          refilterSlashFor(slashTokenNow.query, harness);
          if (session.client.state !== "connected") {
            setSlash((current) => ({ ...current, loading: false }));
          } else {
            const params: Record<string, unknown> = { harness, targetDeviceId: chat.deviceId };
            void session.client
              .call<unknown>(methods.LIST_COMMANDS, params)
              .then((reply) => {
                if (slashRequestRef.current !== request) {
                  return;
                }
                const commands = parseSlashCommands(reply);
                if (commands === null) {
                  setSlash((current) => ({ ...current, loading: false }));
                  return;
                }
                slashCacheRef.current.set(harness, commands);
                setSlash((current) => ({ ...current, loading: false }));
                refilterSlashFor(slashTokenNow.query, harness);
              })
              .catch((error: unknown) => {
                if (slashRequestRef.current !== request) {
                  return;
                }
                const kind = error instanceof RpcError ? error.kind : "failed";
                setSlash((current) => ({
                  ...current,
                  loading: false,
                  error: slashErrorMessage(kind),
                }));
              });
          }
        }
      }
    }

    // ── mention: `@` at a token boundary ──
    const mentionTokenNow = mentionToken(currentText, caretNow);
    const mentionDismissed = mentionDismissedRef.current;
    const mentionStillDismissed =
      mentionTokenNow !== null &&
      mentionDismissed !== null &&
      mentionTokenNow.start === mentionDismissed.start &&
      mentionTokenNow.end === mentionDismissed.end &&
      currentText.slice(mentionTokenNow.start, mentionTokenNow.end) === mentionDismissed.text;
    if (mentionStillDismissed) {
      setMention((current) => (current.token === null ? current : { ...current, token: null }));
      return;
    }
    mentionDismissedRef.current = null;
    if (tokensEqual(mentionTokenNow, mentionRef.current.token)) {
      return;
    }
    mentionRequestRef.current += 1;
    if (mentionSearchTimerRef.current !== null) {
      clearTimeout(mentionSearchTimerRef.current);
      mentionSearchTimerRef.current = null;
    }
    // Refining an already-open menu keeps the stale rows visible until the
    // new response lands (composer.rs:5046-5056); a fresh open clears them.
    const refining = mentionRef.current.token !== null && mentionTokenNow !== null;
    setMention({
      token: mentionTokenNow,
      results: refining ? mentionRef.current.results : [],
      active: refining ? mentionRef.current.active : null,
      loading: mentionTokenNow !== null,
      error: null,
    });
    invalidateTooltip();
    if (mentionTokenNow === null) {
      return;
    }
    const request = mentionRequestRef.current;
    const query = mentionTokenNow.query;
    const chatId = chat.id;
    const deviceId = chat.deviceId;
    // A short debounce prevents one full workspace walk per keystroke
    // (composer.rs:5101-5106); the generation check below drops replies
    // whose query has since changed.
    mentionSearchTimerRef.current = setTimeout(() => {
      mentionSearchTimerRef.current = null;
      void (async () => {
        const params = { query, chatId, targetDeviceId: deviceId };
        let reply: unknown;
        let failure: RpcError | null = null;
        try {
          reply = await session.client.call<unknown>(methods.SEARCH_FILES, params);
        } catch (error) {
          // One retry rides out a cold relay dial to the host device
          // (composer.rs:5110-5118).
          if (
            error instanceof RpcError &&
            (error.kind === "transport" || error.kind === "closed")
          ) {
            await delay(250);
            if (mentionRequestRef.current !== request) {
              return;
            }
            try {
              reply = await session.client.call<unknown>(methods.SEARCH_FILES, params);
            } catch (retryError) {
              failure = retryError instanceof RpcError ? retryError : new RpcError("failed", String(retryError));
            }
          } else {
            failure = error instanceof RpcError ? error : new RpcError("failed", String(error));
          }
        }
        if (
          !mentionResponseIsCurrent(
            { request: mentionRequestRef.current, token: mentionRef.current.token },
            request,
          )
        ) {
          return;
        }
        if (failure !== null) {
          setMention({
            token: mentionRef.current.token,
            results: [],
            active: null,
            loading: false,
            error: mentionErrorMessage(failure.kind),
          });
          return;
        }
        const results = parseFileMatches(reply);
        if (results === null) {
          setMention((current) => ({ ...current, loading: false }));
          return;
        }
        setMention({
          token: mentionRef.current.token,
          results,
          active: results.length > 0 ? 0 : null,
          loading: false,
          error: null,
        });
      })();
    }, 80);
    // text/selection drive the machines; the harness dep keeps the slash
    // cache keyed when the draft's harness changes. The debounce timer is
    // cleared explicitly on token change/reset/unmount — NOT via a cleanup,
    // so an unchanged token's in-flight search survives caret moves.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, selection, draft.harness, chat.id, chat.deviceId, session.client]);

  // The desktop force-closes both popups on every render while the wizard is
  // active or the input is not focused (composer.rs:7176-7185).
  useEffect(() => {
    if (wizardActive || !inputFocused) {
      if (mentionRef.current.token !== null) {
        resetMention(null);
      }
      if (slashRef.current.token !== null) {
        resetSlash(null);
      }
    }
  }, [wizardActive, inputFocused, resetMention, resetSlash]);

  // ── Completion actions ─────────────────────────────────────────────────

  /** `accept_mention` (composer.rs:5173): replace the token range with the
   * strict local Markdown link, caret after the trailing separator. */
  const acceptMention = useCallback(
    (ix: number): void => {
      const token = mentionRef.current.token;
      if (token === null) {
        return;
      }
      const result = mentionRef.current.results[ix];
      if (result === undefined) {
        return;
      }
      const link = localFileLink(result.path, result.isDir);
      const currentText = textRef.current;
      const existing = separatorAfter(currentText, token.end);
      const inserted = existing !== null ? link : `${link} `;
      const caretAfter = token.start + inserted.length + (existing?.length ?? 0);
      applyEdit(token.start, token.end, inserted, caretAfter);
      resetMention(null);
    },
    [applyEdit, resetMention],
  );

  /** `accept_slash` (composer.rs:5475): plain `/name` text — no link, no
   * chip projection. */
  const acceptSlash = useCallback(
    (rowIx: number): void => {
      const token = slashRef.current.token;
      if (token === null) {
        return;
      }
      const state = slashRef.current;
      const commandIx = state.filtered[rowIx];
      const command =
        commandIx === undefined || state.harness === null
          ? undefined
          : slashCacheRef.current.get(state.harness)?.[commandIx];
      if (command === undefined) {
        return;
      }
      const replacement = `/${command.name}`;
      const currentText = textRef.current;
      const existing = separatorAfter(currentText, token.end);
      const inserted = existing !== null ? replacement : `${replacement} `;
      const caretAfter = token.start + inserted.length + (existing?.length ?? 0);
      applyEdit(token.start, token.end, inserted, caretAfter);
      resetSlash(null);
    },
    [applyEdit, resetSlash],
  );

  const acceptCompletion = useCallback(
    (ix?: number): void => {
      const slashTokenNow = slashRef.current.token;
      if (slashTokenNow !== null) {
        acceptSlash(ix ?? slashRef.current.active ?? 0);
        return;
      }
      acceptMention(ix ?? mentionRef.current.active ?? 0);
    },
    [acceptMention, acceptSlash],
  );

  /** `move_mention`/`move_slash` (composer.rs:5150/5452): the shared
   * `popover::menu_step` walk; the cursor row scrolls into view. */
  const moveCompletion = useCallback((delta: number): void => {
    if (slashRef.current.token !== null) {
      const count = slashRef.current.filtered.length;
      setSlash((current) => ({ ...current, active: menuStep(current.active, count, delta) }));
      return;
    }
    const count = mentionRef.current.results.length;
    setMention((current) => ({ ...current, active: menuStep(current.active, count, delta) }));
  }, []);

  // ── The mention path tooltip (composer.rs:2046-2161) ────────────────────

  /** `start_mention_tooltip_wait` (composer.rs:2057). */
  const startTooltipWait = useCallback(
    (target: MentionTooltipTarget): void => {
      tooltipGenRef.current += 1;
      const generation = tooltipGenRef.current;
      if (tooltipTimerRef.current !== null) {
        clearTimeout(tooltipTimerRef.current);
      }
      setTooltipPhase({ kind: "waiting", target, generation });
      tooltipTimerRef.current = setTimeout(() => {
        tooltipTimerRef.current = null;
        const live = projectionRef.current.mentions.some(
          (chip) =>
            chip.link.start === target.start &&
            chip.link.end === target.end &&
            mentionTargetPath(chip) === target.path,
        );
        const next = mentionTooltipPromote(tooltipPhaseRef.current, generation, live);
        if (next !== tooltipPhaseRef.current) {
          setTooltipPhase(next);
        }
      }, MENTION_TOOLTIP_DELAY_MS);
    },
    [setTooltipPhase],
  );

  /** Hit-test the pointer against the mirror's chip spans (the mirror sits
   * under the pointer-transparent textarea, so this is manual). */
  const chipAt = useCallback((x: number, y: number): MentionTooltipTarget | null => {
    const mirror = mirrorRef.current;
    if (mirror === null) {
      return null;
    }
    for (const chip of Array.from(mirror.querySelectorAll<HTMLElement>("[data-chip-index]"))) {
      for (const rect of Array.from(chip.getClientRects())) {
        if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
          const ix = Number(chip.dataset["chipIndex"]);
          const projected = projectionRef.current.mentions[ix];
          if (projected !== undefined) {
            return {
              start: projected.link.start,
              end: projected.link.end,
              path: mentionTargetPath(projected),
            };
          }
        }
      }
    }
    return null;
  }, []);

  // `on_mention_pointer_move` (composer.rs:2087) + the window-level
  // visibility check (`check_mention_tooltip_visibility`, composer.rs:2142):
  // jitter over the same chip cannot restart the delay or flicker the
  // tooltip; the pointer inside the tooltip while visible keeps it; a drag
  // invalidates.
  useEffect(() => {
    if (!mentionsActive) {
      return;
    }
    const onPointerMove = (event: PointerEvent): void => {
      if (event.buttons !== 0) {
        invalidateTooltip();
        return;
      }
      const target = chipAt(event.clientX, event.clientY);
      const tooltipRect = tooltipElRef.current?.getBoundingClientRect() ?? null;
      const inPopup =
        tooltipRect !== null &&
        event.clientX >= tooltipRect.left &&
        event.clientX <= tooltipRect.right &&
        event.clientY >= tooltipRect.top &&
        event.clientY <= tooltipRect.bottom;
      const next = mentionTooltipReduce(
        tooltipPhaseRef.current,
        target,
        inPopup,
        tooltipGenRef.current + 1,
      );
      if (next === tooltipPhaseRef.current) {
        return;
      }
      if (next.kind === "waiting") {
        startTooltipWait(next.target);
      } else {
        if (tooltipTimerRef.current !== null) {
          clearTimeout(tooltipTimerRef.current);
          tooltipTimerRef.current = null;
        }
        setTooltipPhase(next);
      }
    };
    window.addEventListener("pointermove", onPointerMove);
    return () => window.removeEventListener("pointermove", onPointerMove);
  }, [mentionsActive, chipAt, invalidateTooltip, startTooltipWait, setTooltipPhase]);

  // Any edit or mouse-down invalidates (composer.rs:2034-2044).
  useEffect(() => {
    if (mentionsActive) {
      invalidateTooltip();
    }
  }, [text, mentionsActive, invalidateTooltip]);
  useEffect(() => {
    if (!mentionsActive) {
      return;
    }
    const onPointerDown = (): void => invalidateTooltip();
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [mentionsActive, invalidateTooltip]);

  // The tooltip's virtual anchor, recomputed per phase change (the mirror
  // chip's rect — the composer-side half of `ui/Tooltip`'s virtual-anchor
  // mode, ticket 18). GPUI positions the popup 1px off the chip: above when
  // there is room (chip.top − 24 − 1), else flush below so the pointer can
  // enter it — the side picks which; the shared positioner owns the exact
  // geometry.
  const tooltipAnchor = useMemo(() => {
    if (tooltipPhase.kind !== "visible") {
      return null;
    }
    const mirror = mirrorRef.current;
    if (mirror === null) {
      return null;
    }
    for (const chip of Array.from(mirror.querySelectorAll<HTMLElement>("[data-chip-index]"))) {
      const ix = Number(chip.dataset["chipIndex"]);
      const projected = projectionRef.current.mentions[ix];
      if (
        projected === undefined ||
        projected.link.start !== tooltipPhase.target.start ||
        projected.link.end !== tooltipPhase.target.end ||
        mentionTargetPath(projected) !== tooltipPhase.target.path
      ) {
        continue;
      }
      const rect = Array.from(chip.getClientRects())[0];
      if (rect === undefined) {
        continue;
      }
      // GPUI positions the popup 1px off the chip: above when there is
      // room (chip.top − 24 − 1), else flush below so the pointer can enter
      // it. The flush-below arm parks the anchor 1px INSIDE the chip's
      // bottom edge with a zero gap — the same 1px overlap the hand-rolled
      // div painted, without relying on a negative side offset.
      const above = rect.top - MENTION_TOOLTIP_HEIGHT - 1;
      const hasRoomAbove = above >= 0;
      return {
        anchor: virtualAnchorAt(rect.left, hasRoomAbove ? rect.top : rect.bottom - 1),
        side: hasRoomAbove ? ("top" as const) : ("bottom" as const),
        path: tooltipPhase.target.path,
      };
    }
    return null;
    // The anchor derives from live DOM geometry; the mirror + phase drive it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tooltipPhase, projection, inputFocused, selection, text, mentionsActive]);

  // ── The question wizard (composer.rs:5884-6861) ─────────────────────────

  const setWizard = useCallback((wizard: Wizard | null): void => {
    wizardRef.current = wizard;
    setWizardGen(wizard === null ? -1 : 0);
  }, []);

  const clearAdvanceTimer = useCallback((): void => {
    if (advanceTimerRef.current !== null) {
      clearTimeout(advanceTimerRef.current);
      advanceTimerRef.current = null;
    }
  }, []);

  /** `wizard_finish` (composer.rs:6801): queue `RespondInput` on the action
   * path (never the send path), retire the panel, and arm the 2s safety
   * net against a host that rejects the command. */
  const wizardFinish = useCallback(
    (answers: readonly UserInputAnswer[]): void => {
      const wizard = wizardRef.current;
      if (wizard === null) {
        return;
      }
      const requestId = wizard.requestId;
      setWizard(null);
      clearAdvanceTimer();
      answeredRef.current.add(requestId);
      setText("");
      chatDrafts.clear(chat.id);
      if (safetyTimerRef.current !== null) {
        clearTimeout(safetyTimerRef.current);
      }
      const params = {
        chatId: chat.id,
        command: { kind: "respondInput", requestId, answers: [...answers] },
        transfers: [],
      };
      void session.client
        .call(methods.QUEUE_COMMAND, params)
        .then(() => {
          // Safety net (composer.rs:6843-6858): the command queued, but the
          // host may still reject it. If the same request is still the live
          // pending input once it has had time to resolve, the answer
          // demonstrably didn't take — un-hide the panel.
          safetyTimerRef.current = setTimeout(() => {
            safetyTimerRef.current = null;
            const entries = transcript?.getSnapshot().entries ?? [];
            const pending = pendingInputRequest(entries);
            if (pending?.requestId === requestId && answeredRef.current.delete(requestId)) {
              setAnsweredGen((value) => value + 1);
            }
          }, WIZARD_SAFETY_NET_MS);
        })
        .catch((error: unknown) => {
          // The answer never left this device — put the panel back.
          answeredRef.current.delete(requestId);
          setAnsweredGen((value) => value + 1);
          setFailure({
            message: `Answer failed: ${error instanceof Error ? error.message : String(error)}`,
            key: chat.id,
          });
        });
    },
    [chat.id, clearAdvanceTimer, session.client, setWizard, transcript],
  );

  /** `wizard_advance` (composer.rs:6779): page on, clearing the shared
   * free-text input for the next page. */
  const wizardAdvance = useCallback((): void => {
    const wizard = wizardRef.current;
    if (wizard === null) {
      return;
    }
    const step = wizard.advance();
    if (step.kind === "done") {
      wizardFinish(step.answers);
      return;
    }
    setWizardGen((value) => value + 1);
    setText("");
  }, [wizardFinish]);

  /** `schedule_auto_advance` (composer.rs:6769). */
  const scheduleAutoAdvance = useCallback((): void => {
    clearAdvanceTimer();
    advanceTimerRef.current = setTimeout(() => {
      advanceTimerRef.current = null;
      wizardAdvance();
    }, AUTO_ADVANCE_MS);
  }, [clearAdvanceTimer, wizardAdvance]);

  /** `wizard_select` (composer.rs:6750): the placeholder follows the pick. */
  const wizardSelect = useCallback(
    (ix: number): void => {
      const wizard = wizardRef.current;
      if (wizard === null) {
        return;
      }
      const step = wizard.select(ix);
      setWizardGen((value) => value + 1);
      if (step.kind === "autoAdvance") {
        scheduleAutoAdvance();
      }
    },
    [scheduleAutoAdvance],
  );

  const wizardBack = useCallback((): void => {
    const wizard = wizardRef.current;
    if (wizard === null) {
      return;
    }
    wizard.back();
    setWizardGen((value) => value + 1);
  }, []);

  /** Enter in the borrowed input: the typed text becomes the page's answer,
   * then the page advances (composer.rs:5984-5992). */
  const wizardSubmitFromInput = useCallback((): void => {
    const wizard = wizardRef.current;
    if (wizard === null) {
      return;
    }
    wizardCommitThenAdvance(wizard, textRef.current, wizardAdvance);
  }, [wizardAdvance]);

  /** The PHONE explicit advance (ticket 75 §2.2.1) — the panel's Next/Submit
   * button and the unfocused panel Enter: bare Enter is a newline on the
   * phone layer, so these paths own the commit. Cancelling any pending
   * option auto-advance timer first keeps the action to exactly one page
   * move; the commit runs even when the input is empty so a stale typed
   * override cannot leak into an option-only answer. */
  const wizardAdvanceCommit = useCallback((): void => {
    clearAdvanceTimer();
    wizardSubmitFromInput();
  }, [clearAdvanceTimer, wizardSubmitFromInput]);

  // `on_state_changed`'s question lifecycle (composer.rs:5879-5928): open on
  // a fresh unresolved request, latch until it resolves or a newer
  // assistant entry supersedes it — never on run death. A pending question
  // must not take over an active queue edit.
  useEffect(() => {
    if (editingMessage !== null && editingMessage !== undefined) {
      return;
    }
    const entries = transcriptSnapshot?.entries ?? [];
    const pending = pendingInputRequest(entries);
    const answered = answeredRef.current;
    if (pending !== null && !answered.has(pending.requestId)) {
      const current = wizardRef.current;
      if (current === null || current.requestId !== pending.requestId) {
        resetMention(null);
        resetSlash(null);
        clearAdvanceTimer();
        setWizard(new Wizard(pending.requestId, [...pending.questions]));
      }
      return;
    }
    const wizard = wizardRef.current;
    if (wizard !== null) {
      const released =
        inputRequestResolved(entries, wizard.requestId) ||
        (entries.length > 0 && !answered.has(wizard.requestId));
      if (released) {
        setWizard(null);
        clearAdvanceTimer();
      }
    }
  }, [transcriptSnapshot, editingMessage, answeredGen, wizardGen, resetMention, resetSlash, clearAdvanceTimer, setWizard]);

  // The borrowed input's placeholder pair (composer.rs:5896-5898, 6751-6760,
  // 6810) — restored to "Do anything…" the moment the panel unmounts.
  const placeholder =
    wizardActive && wizardRef.current !== null
      ? wizardPlaceholder(wizardRef.current.pageHasPick())
      : COMPOSER_REST_PLACEHOLDER;

  // While the wizard is mounted the flip machinery stands down (the pill is
  // not rendered); the wizard's own auto-grow owns the input's height,
  // capped at five lines — and past the cap the same `data-scrollable`
  // gate (CSS-side, bar hidden) keeps the wheel scrolling the input.
  useEffect(() => {
    if (!wizardActive || textareaRef.current === null) {
      return;
    }
    const el = textareaRef.current;
    el.style.height = "auto";
    const capped = Math.min(Math.max(el.scrollHeight, 22.75), 120);
    el.style.height = `${capped}px`;
    el.dataset["scrollable"] = el.scrollHeight > 120 ? "true" : "false";
  }, [wizardActive, text, placeholder]);

  // When the wizard opens, focus lands where the desktop keeps it: the
  // input if it held focus through the swap, else the panel (digits select
  // while the input is unfocused).
  useEffect(() => {
    if (!wizardActive) {
      return;
    }
    if (inputFocused) {
      textareaRef.current?.focus();
    } else {
      wizardPanelRef.current?.focus();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wizardActive]);

  // Pending timers never outlive the composer.
  useEffect(
    () => () => {
      if (advanceTimerRef.current !== null) {
        clearTimeout(advanceTimerRef.current);
      }
      if (safetyTimerRef.current !== null) {
        clearTimeout(safetyTimerRef.current);
      }
      if (tooltipTimerRef.current !== null) {
        clearTimeout(tooltipTimerRef.current);
      }
      if (mentionSearchTimerRef.current !== null) {
        clearTimeout(mentionSearchTimerRef.current);
      }
    },
    [],
  );

  const applyDraft = useCallback((next: DraftConfig) => {
    setDraft(next);
  }, []);

  // The composer's mid-session model / reasoning / options changes persist
  // through `Mutate setChatConfig` (pickers.rs:1474) — a picker change, never
  // a send.
  const persistDraft = useCallback(
    (next: DraftConfig) => {
      void persistChatConfig(session.client, chat.id, next).catch((error: unknown) => {
        setFailure({ message: describeSendError(error), key: chat.id });
      });
    },
    [session.client, chat.id],
  );

  // ── The queue capability gate (composer.rs:6096-6110) ──────────────────
  // MESSAGE_QUEUE_V1 (and MESSAGE_QUEUE_ATTACHMENTS_V1 when the send
  // carries attachments) checked on the engine before taking the draft. On
  // failure, do not send: raise the verbatim notice and leave the draft.
  // (The desktop also checks the chat's HOST device; the web's engine-local
  // pairing has no host registry yet — the engine check stands in until
  // ticket 31's fleet.)
  const engineSupports = useCallback(
    (capability: string): boolean => {
      const capabilities = session.client.engineInfo?.capabilities ?? [];
      return capabilities.includes(capability);
    },
    [session.client],
  );

  // ── Interrupt (`interrupt_selected`, composer.rs:6707-6737) ────────────
  const interrupt = useCallback(async (): Promise<void> => {
    if (!beginInterrupt(interruptingRef.current, chat.id)) {
      return;
    }
    try {
      await sendInterrupt(session.client, chat.id);
    } catch (error) {
      interruptingRef.current.delete(chat.id);
      setFailure({ message: `Stop failed: ${describeSendError(error)}`, key: chat.id });
    }
  }, [chat.id, session.client]);

  // ── The send path (`Composer::send`, composer.rs:6032-6705) ────────────
  const send = useCallback(
    async (typed: string, queue: boolean): Promise<void> => {
      // Existing busy chats always queue; compatibility was checked before
      // taking the draft (the capability gate below).
      if (queue) {
        const capability = staged.length > 0 ? MESSAGE_QUEUE_ATTACHMENTS_V1 : MESSAGE_QUEUE_V1;
        if (!engineSupports(capability)) {
          setFailure({
            message: "Update the chat's engine to queue messages during a response.",
            key: chat.id,
          });
          return;
        }
      }
      const trimmed = typed.trim();
      // The resolved send cwd (composer.rs:6433-6440, `resolveSendCwd`): a
      // NEW chat runs from the picked space's path else `"~"` — expanded by
      // the ENGINE host-side when the run spawns (sessions.rs:342-352; the
      // web never expands it client-side); an EXISTING chat runs from its
      // stored cwd else `"."`. There is no error path — the desktop has no
      // guard for a projectless send, and neither does the web.
      const sendCwd = resolveSendCwd(chat.id === "", chat.cwd, chat.cwd);
      // New-chat mode mints the chat id on first send (shell.rs:5897-5899,
      // composer.rs:6249-6258): create the row, wait for it to land, and
      // hand the host the navigation BEFORE the wire call — the route
      // observation then drives the dock, exactly the
      // `NewThreadTransitionStarted` → `select_chat` chain.
      let chatId = chat.id;
      // The id the ROUTE and the page's stores read — the echo, the drafts,
      // the staged stash, the failure key. An existing chat's URL id is
      // already the scoped row id; a fresh mint scopes here (the same call
      // add-space's optimistic rows make) because every merged fleet row id
      // is scoped while the WIRE keeps the raw id (request-routing decodes
      // either form).
      let pageChatId = chat.id;
      if (chatId === "") {
        if (session.client.state !== "connected") {
          setFailure({ message: "Send failed: Engine not connected", key: null });
          return;
        }
        try {
          chatId = await createChat(
            session.client,
            {
              ...(chat.spaceId != null ? { spaceId: chat.spaceId } : { deviceId: chat.deviceId }),
              // §2.2 (composer.rs:6494-6544): the full wire payload. Only a
              // genuinely NEW chat writes a config — the resolved draft rides
              // here while the run carries model/reasoning/options on the
              // RunRequest itself. `branch`/`cwd` insert only when present:
              // the canvas's ref pick stays chip-local today (ticket 10's
              // checkout executes SwitchRef at pick time), and the
              // projectless `"~"` NEVER rides here — it lives on the
              // RunRequest (composer.rs:6515-6521 inserts `cwd` only for the
              // worktree-reuse override).
              config: buildChatConfig(draft),
            },
          );
          await waitForChatRow(session.cache, chatId);
        } catch (error) {
          setFailure({ message: `Send failed: ${describeSendError(error)}`, key: null });
          return;
        }
        pageChatId = encodeScopedId(session.engine.baseUrl, chatId);
        // The host scopes the id at navigation (§2.3): the raw id stays on
        // the wire, the route carries the scoped form.
        onNewThreadLaunched?.(chatId);
      }
      // Snapshot-and-clear NOW (`takeAttachments`): the strip empties the
      // instant you hit send; a failure hands the files back by id.
      const taken = staged;
      setStagedByChat((current) => {
        const next = { ...current };
        delete next[pageChatId];
        return next;
      });
      // `take_review_comments` (composer.rs:6130-6136): the comment block is
      // staged onto the composer key the draft was opened against — for the
      // new-chat canvas that is `""`, the pre-mint key.
      const takenComments = reviewCommentStore.takeComments(chat.id);
      // `typed` keeps the user's own words for the failure hand-back below
      // (restoring a folded prompt would paste the trailer as literal text).
      // The echo cleanup key: null until an echo actually goes up, so a
      // pre-flight throw (or a queued send, which never publishes) removes
      // nothing.
      let pushedEchoId: string | null = null;
      const onProgress = (uploaded: number): void => {
        setUploadProgress(uploaded);
      };
      try {
        // The mint is the try's FIRST step (§2.5's silent-crash half): ANY
        // pre-flight throw — a missing `crypto.randomUUID` on a plain-HTTP
        // LAN origin included — surfaces as the failure notice with the full
        // hand-back, never an uncaught async rejection.
        const messageId = mintMessageId();
        // The echo's attachment refs from the FIRST frame (composer.rs:6188-
        // 6233): the legacy flow's synthetic `pending/{id}/{name}` paths, so
        // the just-sent bubble's thumbnails render immediately and carry the
        // sending overlay while the upload streams; the post-upload refresh
        // swaps them for the host's absolute paths.
        const pendingPaths = taken.map((att) => `pending/${att.id}/${att.name}`);
        const echoDeviceId = session.client.engineInfo?.deviceId ?? null;
        if (echoDeviceId !== null) {
          // Seed the staged bytes under the pending refs — the echo renders
          // from local bytes, never a read-back round-trip.
          taken.forEach((att, ix) => {
            seedAttachment(echoDeviceId, pendingPaths[ix]!, {
              name: att.name,
              mime: formatToMime(att.format),
              bytes: att.bytes,
            });
          });
        }
        // The optimistic echo goes up BEFORE the wire call — gated off for a
        // queued send, whose queue row IS its representation until dispatch
        // (`should_publish_optimistic_echo`). Keyed by the PAGE id — the
        // scoped form on a first send — so the just-sent bubble renders on
        // the route this send navigated to, exactly as an existing chat's
        // echo does (the transcript reads `forChat(docId)` off the URL).
        if (shouldPublishOptimisticEcho(queue)) {
          echoStore.pushEcho({
            messageId,
            chatId: pageChatId,
            startedAtMs: Date.now(),
            text: trimmed,
            attachmentPaths: pendingPaths,
          });
          pushedEchoId = messageId;
        }
        setText("");
        chatDrafts.clear(pageChatId);
        setFailure(null);
        setBusy(true);
        // Whole-send upload accounting (`begin_upload_progress`): the percent
        // the transcript's sending-overlay ring reads lives in the shared
        // attachment store, not the strip — the strip itself has no progress UI.
        if (taken.length > 0) {
          beginUploadProgress(taken.reduce((sum, att) => sum + att.bytes.byteLength, 0));
        }
        if (queue) {
          // Queue rows keep a clean body (the host rebuilds the attachment
          // transport when it promotes the row); the bytes upload first on
          // the web's legacy blocking path. The comment block DOES ride the
          // queued text (`queue_body`, composer.rs:6559-6572) — only the
          // attachment trailer stays out.
          const uploaded = taken.length > 0 ? await uploadAttachments(session.client, taken, onProgress) : [];
          const folded = withComments(trimmed, takenComments);
          const body = folded.length > 0 ? folded : ATTACHMENT_ONLY_TEXT;
          await queueMessage(
            session.client,
            chatId,
            body,
            uploaded.map((entry) => entry.path),
          );
        } else {
          const sendResult = await sendRun(
            session.client,
            chatId,
            draft,
            trimmed,
            sendCwd,
            { mintMessageId: () => messageId },
            taken.length > 0 || takenComments.length > 0
              ? { stagedAttachments: taken, uploadProgress: onProgress, stagedReviewComments: takenComments }
              : { stagedReviewComments: takenComments },
          );
          // Refresh the echo in place with the attachment-folded prompt so
          // its state never flickers (composer.rs:6401-6423).
          if (echoStore.get(messageId) !== null) {
            echoStore.removeEcho(messageId);
            echoStore.pushEcho({
              messageId,
              chatId: pageChatId,
              startedAtMs: Date.now(),
              text: sendResult.finalPrompt,
              attachmentPaths: [...sendResult.attachmentPaths],
            });
          }
          // Seed the attachment cache so the just-sent bubble renders from
          // local bytes without a ReadAttachmentChunk round-trip.
          const deviceId = session.client.engineInfo?.deviceId ?? null;
          if (deviceId !== null) {
            taken.forEach((att, ix) => {
              const path = sendResult.attachmentPaths[ix];
              if (path === undefined) {
                return;
              }
              seedAttachment(deviceId, path, {
                name: att.name,
                mime: formatToMime(att.format),
                bytes: att.bytes,
              });
            });
          }
        }
      } catch (error) {
        // Failure: red notice, echo removed, prompt back in the draft,
        // staged files back in the stash (merged by id so anything staged
        // during the send survives). The hand-back keys the PAGE id — the
        // route this send navigated to reads its drafts/stash under the
        // scoped form.
        if (pushedEchoId !== null) {
          echoStore.removeEcho(pushedEchoId);
        }
        setText(typed);
        chatDrafts.set(pageChatId, typed);
        // The taken comments stage back under the chat's key
        // (`add_review_comment(&restore_key, …)`, composer.rs:6663-6667).
        reviewCommentStore.restoreComments(pageChatId, takenComments);
        setStagedByChat((current) => {
          const fresh = current[pageChatId] ?? [];
          const merged = [...taken.filter((att) => !fresh.some((f) => f.id === att.id)), ...fresh];
          const next = { ...current };
          if (merged.length === 0) {
            delete next[pageChatId];
          } else {
            next[pageChatId] = merged;
          }
          return next;
        });
        // The upload path's failure copy is verbatim (composer.rs:6358-6374);
        // every other failure keeps the prefixed shape.
        setFailure({
          message: error instanceof AttachmentUploadError
            ? error.message
            : `Send failed: ${describeSendError(error)}`,
          key: pageChatId,
        });
      } finally {
        if (taken.length > 0) {
          endUploadProgress();
        }
        setBusy(false);
      }
    },
    [chat.id, chat.cwd, draft, session.client, session.engine.baseUrl, staged, engineSupports, onNewThreadLaunched, commentCount],
  );

  // ── The submit dispatch (`on_submit`, composer.rs:5980-6007) ───────────
  // `commit_queue_edit` (queue.rs:1430): save the composer into the leased
  // row — an entirely empty composer (no text, no staged attachments)
  // discards the row; anything else commits the trimmed text plus the live
  // staged set through the lease. The queue row's inline Save reaches the
  // same path through `editCommitRef` (ticket 16).
  const commitQueueEdit = useCallback(() => {
    const editing = editingMessage ?? null;
    if (editing === null) {
      return;
    }
    const trimmed = text.trim();
    const empty = trimmed.length === 0 && staged.length === 0;
    onEditFinish?.(
      empty
        ? { action: "discard", text: "" }
        : { action: "commit", text: trimmed, staged },
    );
  }, [editingMessage, text, staged, onEditFinish]);

  useEffect(() => {
    if (editCommitRef === undefined) {
      return;
    }
    editCommitRef.current = commitQueueEdit;
    return () => {
      editCommitRef.current = null;
    };
  }, [editCommitRef, commitQueueEdit]);

  const submit = useCallback(async () => {
    if (busy) {
      return;
    }
    // A queued-row edit: the submit commits the row through the lease and
    // is DONE — it never also fires a new message (`commit_queue_edit`
    // returns "handled").
    const editing = editingMessage ?? null;
    if (editing !== null) {
      commitQueueEdit();
      return;
    }

    const content = composerHasContent(text, staged.length, commentCount);
    const mode = sendButtonMode(runLive, content);
    if (mode === "stop") {
      // Enter never stops a run (composer.rs on_submit, issue #406): Stop
      // mode implies an empty composer, so a stray extra Enter right after
      // sending landed an interrupt on the just-dispatched prompt and the
      // agent ate it silently. Stop stays on the button — and on Esc when
      // the escape setting is enabled.
      return;
    }
    if (!content) {
      return;
    }
    if (
      sendBlocked({
        queueEditFinishing: busy,
        requestTargetDisconnected: session.client.state !== "connected",
        reviewCommentFlushPending: false,
        // Condition 4 (composer.rs:5958-5962): the new-chat canvas with a
        // LOADED catalog that reports no agents — offline/loading must not
        // block.
        newChatNoAgents: newChat && harnesses.loaded && harnesses.rows.length === 0,
      })
    ) {
      // A blocked send is a no-op — no failure, no wire call
      // (composer.rs:6002, `_ if self.send_blocked(cx) => {}`).
      return;
    }
    await send(text, mode === "queue");
  }, [busy, text, staged, runLive, commentCount, editingMessage, onEditFinish, session.client, interrupt, send, commitQueueEdit]);

  // ── Key policy: completions → phone newline → wizard → Enter (§2.7) ────
  // `resolveEnterAction` (lib/composer-send.ts) is the Enter branch's single
  // decision owner, per ticket 75 §2.2's order. Exactly two Enter bindings
  // on the desktop (`messageEnterBindings`); Shift+Enter is always a native
  // newline. While an IME composition is active, Enter is never a submit.
  // On the phone layer a bare Enter is a native newline at EVERY saved
  // preference — the `ComposerSendBehavior` setting is read, never mutated.

  /** The atomic-motion keys a chip projection intercepts (Left/Right/
   * Backspace/Delete at a chip boundary — one press steps over or removes
   * the whole mention, composer.rs:2345-2383). Returns true when consumed. */
  const handleAtomicMotion = (event: ReactKeyboardEvent<HTMLTextAreaElement>): boolean => {
    const projectionNow = projectionRef.current;
    if (projectionNow.mentions.length === 0) {
      return false;
    }
    const el = textareaRef.current;
    if (el === null) {
      return false;
    }
    const bare = !event.metaKey && !event.ctrlKey && !event.altKey;
    const start = el.selectionStart ?? 0;
    const end = el.selectionEnd ?? 0;
    const collapsed = start === end;
    switch (event.key) {
      case "ArrowLeft": {
        if (!bare || !collapsed) {
          return false;
        }
        const boundary = projectionNow.previousBoundary(start);
        if (boundary === null) {
          return false;
        }
        event.preventDefault();
        el.setSelectionRange(boundary, boundary);
        setSelectionState([boundary, boundary]);
        return true;
      }
      case "ArrowRight": {
        if (!bare || !collapsed) {
          return false;
        }
        const boundary = projectionNow.nextBoundary(start);
        if (boundary === null) {
          return false;
        }
        event.preventDefault();
        el.setSelectionRange(boundary, boundary);
        setSelectionState([boundary, boundary]);
        return true;
      }
      case "Backspace": {
        if (!bare) {
          return false;
        }
        if (collapsed) {
          const boundary = projectionNow.previousBoundary(start);
          if (boundary !== null) {
            event.preventDefault();
            applyEdit(boundary, start, "", boundary);
            return true;
          }
        }
        return false;
      }
      case "Delete": {
        if (!bare) {
          return false;
        }
        if (collapsed) {
          const boundary = projectionNow.nextBoundary(start);
          if (boundary !== null) {
            event.preventDefault();
            applyEdit(start, boundary, "", start);
            return true;
          }
        }
        return false;
      }
      default:
        return false;
    }
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>): void => {
    if (event.nativeEvent.isComposing) {
      return;
    }
    const slashOpen = slashRef.current.token !== null;
    const completionOpen = slashOpen || mentionRef.current.token !== null;
    const completionHasSelection = slashOpen
      ? slashRef.current.active !== null
      : mentionRef.current.active !== null;

    // 1. Escape: a completion open always dismisses and stops here
    // (`escape_dismisses_completion`, composer.rs:2651).
    if (event.key === "Escape") {
      if (escapeDismissesCompletion(event.key, completionOpen)) {
        event.preventDefault();
        event.stopPropagation();
        dismissCompletion();
        return;
      }
      if (wizardActiveRef.current) {
        // The borrowed input swallows Escape; it pages back only when the
        // input is empty (wizard_escape_goes_back with inputFocused=true).
        event.preventDefault();
        event.stopPropagation();
        if (wizardEscapeGoesBack(event.key, true, textRef.current.length === 0)) {
          wizardBack();
        }
        return;
      }
      // A queue edit's cancel rides the shell's escape ladder (ticket 13);
      // anything else bubbles.
      return;
    }

    // 2. Tab accepts a selected completion (`MentionTab`, composer.rs:2642).
    if (event.key === "Tab" && completionOpen && completionHasSelection) {
      event.preventDefault();
      event.stopPropagation();
      acceptCompletion();
      return;
    }

    // 3. Arrows walk the open completion's rows (composer.rs:2385-2403).
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && completionOpen && completionHasSelection) {
      event.preventDefault();
      event.stopPropagation();
      moveCompletion(event.key === "ArrowDown" ? 1 : -1);
      return;
    }

    if (event.key === "Enter") {
      // 4. `resolveEnterAction` (lib/composer-send.ts) owns the decision —
      // ticket 75 §2.2's order: a selected completion wins exactly once; a
      // PHONE bare Enter is a native newline before the wizard and message
      // submit branches at every saved preference; wizard and
      // modified-submit policy otherwise stays as it is; a desktop-width
      // bare Enter follows the saved `ComposerSendBehavior`. The modifier
      // state is read ONCE below and handed to the resolver; the switch
      // trusts that same state, never re-deriving it from the event.
      const mod = event.metaKey || event.ctrlKey;
      const alt = event.altKey;
      const shift = event.shiftKey;
      const action = resolveEnterAction({
        phone: isPhone,
        composing: event.nativeEvent.isComposing,
        completionSelected: completionOpen && completionHasSelection,
        wizardActive: wizardActiveRef.current,
        mod,
        alt,
        shift,
        sendBehavior,
      });
      switch (action) {
        case "imeNative":
          // Unreachable (the handler's first guard owns compositions) — an
          // IME event is never consumed here either.
          return;
        case "acceptCompletion":
          // `enter_outcome` (composer.rs:1407): a live completion selection
          // always wins over submit or newline.
          event.preventDefault();
          event.stopPropagation();
          acceptCompletion();
          return;
        case "nativeNewline": {
          // The textarea's NATIVE default performs the newline — mid-text
          // insertion, selection replacement, undo and IME stay correct;
          // never preventDefault, never set the value manually. The
          // resolver took this action through its phone bare-Enter branch
          // exactly when the state it saw (`isPhone`, mod/alt/shift) says
          // so — the isolation rides that decision.
          if (isPhone && !mod && !alt && !shift) {
            // Isolate the phone newline from the wizard panel's Enter.
            event.stopPropagation();
          }
          return;
        }
        case "wizardSuppress":
          // The borrowed `"Composer"` context drops ModifiedSubmit.
          event.preventDefault();
          return;
        case "wizardSubmit":
          // The borrowed input's bare Enter submits the page
          // (composer.rs:5984-5992).
          event.preventDefault();
          event.stopPropagation();
          wizardSubmitFromInput();
          return;
        case "modifiedSubmit": {
          // Mod+Enter: `ModifiedSubmit` — submits content, activates the
          // most recently queued row on a truly empty composer, never Stop.
          event.preventDefault();
          const content = composerHasContent(text, staged.length, commentCount);
          if (modifiedSubmitTarget(content) === "submitContent") {
            void submit();
          } else {
            activateLatestQueued?.();
          }
          return;
        }
        case "submit":
          event.preventDefault();
          void submit();
          return;
      }
    }

    // 5. Atomic chip motion (only when the projection has chips).
    if (handleAtomicMotion(event)) {
      return;
    }
  };

  // ── `on_wizard_key` (composer.rs:6863-6894) on the panel wrapper ────────
  // Keys bubbling out of the free-text input must not double-handle: digits
  // select only while the input is unfocused or empty, Enter advances only
  // when the input is unfocused (its own Submit policy owns the focused
  // case), Escape pages back — swallowed either way.
  const onWizardKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    const inputFocusedNow = document.activeElement === textareaRef.current;
    const inputEmpty = textRef.current.length === 0;
    const modified = event.metaKey || event.ctrlKey || event.altKey;
    if (/^[1-9]$/.test(event.key)) {
      // A BARE digit picks an option; with a modifier the keystroke belongs
      // to an app shortcut (⌘1..⌘9 jump to sidebar rows) — never consumed.
      if (modified) {
        return;
      }
      if (!inputFocusedNow || inputEmpty) {
        // Consumed as a selection: the digit is not also typed.
        event.preventDefault();
        event.stopPropagation();
        wizardSelect(Number(event.key) - 1);
      }
      return;
    }
    if (event.key === "Enter") {
      if (!inputFocusedNow) {
        event.preventDefault();
        event.stopPropagation();
        // Phone: the same commit-then-advance as the panel's button (ticket
        // 75 §2.2.1) — a bare Enter key is a newline on the phone layer, so
        // the explicit paths own committing the shared draft. The desktop
        // path is unchanged.
        if (isPhone) {
          wizardAdvanceCommit();
        } else {
          wizardAdvance();
        }
      }
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (wizardEscapeGoesBack("escape", inputFocusedNow, inputEmpty)) {
        wizardBack();
      }
    }
  };

  // Escape while editing a queued row cancels the edit (the container
  // binding, composer.rs:7473-7483): registered on the shell's escape
  // ladder, above the desktop's shell surfaces and the bubble-phase
  // interrupt, so the key is consumed once.
  useEffect(() => {
    if (editingMessage === null || editingMessage === undefined) {
      return;
    }
    return registerEscapeSurface(ESCAPE_PRIORITY.composerQueueEdit, () => {
      onEditCancel?.();
      return true;
    });
  }, [editingMessage, onEditCancel]);

  // ── Paste of image data (composer.rs's clipboard) ──────────────────────
  // Clipboard images beat text and stage as attachments; non-image files
  // are skipped silently; only a clipboard with no files falls through to
  // the native text paste.
  const onPaste = useCallback(
    (event: ReactClipboardEvent<HTMLTextAreaElement>): void => {
      const items = event.clipboardData?.items;
      if (items === undefined) {
        return;
      }
      const files: File[] = [];
      for (const item of Array.from(items)) {
        if (item.kind !== "file") {
          continue;
        }
        const file = item.getAsFile();
        if (file !== null) {
          files.push(file);
        }
      }
      if (files.length === 0) {
        return;
      }
      event.preventDefault();
      void (async () => {
        const stagedNext: StagedAttachment[] = [];
        for (const file of files) {
          if (formatByName(file.name) === null && !file.type.startsWith("image/")) {
            // Non-image files are skipped silently.
            continue;
          }
          try {
            stagedNext.push(await stageFile(file));
          } catch {
            // Undecodable bytes are skipped just as silently.
          }
        }
        if (stagedNext.length > 0) {
          setStagedByChat((current) => ({
            ...current,
            [chat.id]: [...(current[chat.id] ?? []), ...stagedNext],
          }));
        }
      })();
    },
    [chat.id],
  );

  // ── The send button (§2.11) ─────────────────────────────────────────────
  const hasContent = composerHasContent(text, staged.length, commentCount);
  const editingActive = editingMessage !== null && editingMessage !== undefined;
  const mode: "send" | "queue" | "stop" = editingActive
    ? "send"
    : sendButtonMode(runLive, hasContent);
  const blocked =
    mode !== "stop" &&
    sendBlocked({
      queueEditFinishing: busy,
      requestTargetDisconnected: session.client.state !== "connected",
      reviewCommentFlushPending: false,
      newChatNoAgents: newChat && harnesses.loaded && harnesses.rows.length === 0,
    });

  // ── The queue-degraded caption (§2.3) ───────────────────────────────────
  // The desktop's caption gates on `chat_delivery_degraded`'s per-chat
  // room/device arms (composer.rs:7318-7336), which the web does not port;
  // the engine's own connection state stands in for the transport the
  // queue rides. (The pending-send overlay's gate is the real
  // WatchConnectivity posture now — `chatDeliveryDegraded`,
  // state/transcript-store.ts, threaded by the chat page.) It clears
  // itself the moment the path heals.
  const engineState = engineStatus?.state ?? "connecting";
  const queueDegraded = engineState !== "connected";
  const queueOffline = engineState !== "reconnecting";
  const queueNotice = queueDegraded
    ? queueOffline
      ? "Offline — messages will send when you're back online."
      : "Messages will send once the connection recovers."
    : null;

  // The chat-scoped failure filter (composer.rs:7311-7315).
  const failureVisible =
    failure !== null && (failure.key === null || failure.key === chat.id) ? failure.message : null;

  const onStage = useCallback(
    (next: readonly StagedAttachment[]) => {
      setStagedByChat((current) => ({
        ...current,
        [chat.id]: [...(current[chat.id] ?? []), ...next],
      }));
    },
    [chat.id],
  );
  const onRemove = useCallback(
    (id: string) => {
      setStagedByChat((current) => {
        const list = current[chat.id] ?? [];
        const next = list.filter((att) => att.id !== id);
        const merged = { ...current };
        if (next.length === 0) {
          delete merged[chat.id];
        } else {
          merged[chat.id] = next;
        }
        return merged;
      });
    },
    [chat.id],
  );
  const onStageError = useCallback((message: string) => {
    setFailure({ message, key: null });
  }, []);

  // The attach button drives the strip's hidden input; focus returns to the
  // draft when the dialog closes (both pick and cancel).
  const onAttachClick = useCallback(() => {
    focusPendingRef.current = true;
    attachRef.current?.();
  }, []);
  useEffect(() => {
    const onWindowFocus = (): void => {
      if (focusPendingRef.current) {
        focusPendingRef.current = false;
        textareaRef.current?.focus();
      }
    };
    window.addEventListener("focus", onWindowFocus);
    return () => window.removeEventListener("focus", onWindowFocus);
  }, []);

  // The pill's mouse-down focus (composer.rs:7701-7709): padding and action
  // controls are part of the text composer — unless an open menu keeps its
  // own keyboard/search focus.
  const onPillMouseDown = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>): void => {
      if (pickersOpen) {
        return;
      }
      if (event.button === 0 && document.activeElement !== textareaRef.current) {
        textareaRef.current?.focus();
      }
    },
    [pickersOpen],
  );

  const sendAriaLabel = mode === "stop" ? "Stop" : mode === "queue" ? "Queue" : "Send";

  // ── The chip mirror (§2.3's web note) ───────────────────────────────────
  // A textarea cannot wash a sub-range: while the draft carries mentions the
  // textarea's own text is transparent (caret included — its raw-offset
  // geometry no longer matches the display text), and this mirror under it
  // paints the projected text — chips in the mono font over the code wash —
  // plus the selection wash and the caret at its display offset.
  const [selStart, selEnd] = selection;
  const caretDisplay = selStart === selEnd && inputFocused ? projection.rawToDisplay(selStart) : null;
  const selectionDisplay =
    selStart !== selEnd
      ? { start: projection.rawToDisplay(selStart), end: projection.rawToDisplay(selEnd) }
      : null;

  const mirrorNodes = useMemo(() => {
    if (!mentionsActive) {
      return null;
    }
    const chips = projection.mentions;
    const cuts = new Set<number>([0, projection.display.length]);
    for (const chip of chips) {
      cuts.add(chip.start);
      cuts.add(chip.end);
    }
    if (selectionDisplay !== null) {
      cuts.add(selectionDisplay.start);
      cuts.add(selectionDisplay.end);
    }
    if (caretDisplay !== null) {
      cuts.add(caretDisplay);
    }
    const points = [...cuts].sort((a, b) => a - b);
    const nodes: ReactNode[] = [];
    for (let ix = 0; ix < points.length - 1; ix += 1) {
      const from = points[ix]!;
      const to = points[ix + 1]!;
      if (caretDisplay === from) {
        nodes.push(<span key={`caret-${from}`} className="composer-input-caret" />);
      }
      if (from === to) {
        continue;
      }
      const chipIx = chips.findIndex((candidate) => candidate.start <= from && to <= candidate.end);
      const selected =
        selectionDisplay !== null &&
        selectionDisplay.start <= from &&
        to <= selectionDisplay.end;
      const textPart = projection.display.slice(from, to);
      const classes = [
        chipIx >= 0 ? "mention-chip" : "",
        selected ? "composer-input-selection" : "",
      ]
        .filter((name) => name.length > 0)
        .join(" ");
      nodes.push(
        <span key={`seg-${from}`} className={classes} data-chip-index={chipIx >= 0 ? chipIx : undefined}>
          {textPart}
        </span>,
      );
    }
    if (caretDisplay === projection.display.length) {
      nodes.push(<span key="caret-end" className="composer-input-caret" />);
    }
    return nodes;
  }, [projection, mentionsActive, selectionDisplay, caretDisplay]);

  // Keep the mirror's scroll glued to the textarea's (it mounts/unmounts
  // with the mentions).
  useLayoutEffect(() => {
    if (mirrorRef.current !== null && textareaRef.current !== null) {
      mirrorRef.current.scrollTop = textareaRef.current.scrollTop;
    }
  }, [mirrorNodes]);

  // The shared input: the SAME textarea element in the pill and in the
  // wizard's free-text slot (one JSX node, one draft state — never a second
  // input).
  const inputStack = (
    <div className="composer-input-stack">
      <textarea
        ref={textareaRef}
        className="composer-input"
        rows={1}
        value={text}
        placeholder={placeholder}
        onChange={onInputChange}
        onSelect={onInputSelect}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onFocus={() => setInputFocused(true)}
        onBlur={() => setInputFocused(false)}
        onCompositionStart={() => setComposing(true)}
        onCompositionEnd={() => setComposing(false)}
        spellCheck={false}
        autoComplete="off"
        aria-label={placeholder}
        // The phone soft keyboard's return key labels a line break (MDN
        // enterkeyhint — a LABEL hint; the key policy above is the behavior
        // change). Desktop renders no attribute, as before.
        enterKeyHint={isPhone ? "enter" : undefined}
        data-mentions={mentionsActive && !composing ? "true" : "false"}
      />
      {mirrorNodes !== null && !composing && (
        <div className="composer-input-mirror" ref={mirrorRef} aria-hidden="true">
          {mirrorNodes}
        </div>
      )}
    </div>
  );

  // ── Ticket 15 render pieces ─────────────────────────────────────────────
  // `expanded = expanded_mode || new_chat` (composer.rs:7488): the rendered
  // mode is the canvas-forced one.
  const expandedRender = expanded || newChat;
  // The route chrome crossfade (`route_chrome_opacities`, composer.rs:88):
  // with a dock frame the `selectors`/`footer` channels are the pair; the
  // two never overlap, so the new-thread selector row and the session
  // footer are never BOTH mounted — the hidden one is unmounted (not just
  // hidden) so its popover triggers cannot be found twice.
  const chrome =
    dockFrame !== null
      ? { newThread: dockFrame.visuals.selectors, session: dockFrame.visuals.footer }
      : routeChromeOpacities(newChat ? 1 : 0);
  // `surface_radius = COMPOSER_RADIUS − 4 × dock_amount` (composer.rs:7603):
  // 26 at the hero, 22 docked — the frost blur's mask follows the radius.
  const surfaceRadius = dockFrame !== null ? 26 - 4 * Math.min(Math.max(dockFrame.amount, 0), 1) : null;

  return (
    <div
      className={`composer ${expandedRender ? "composer-expanded" : "composer-compact"}`}
      data-working={runLive ? "true" : undefined}
      data-editing={editingActive ? "true" : undefined}
      data-wizard={wizardActive ? "true" : undefined}
    >
      {failureVisible !== null && (
        <NoticeChip
          tone={noticeToneForMessage(failureVisible)}
          variant="plain"
          label={noticeLabelForTone(noticeToneForMessage(failureVisible))}
          message={failureVisible}
          id="composer-failure"
          role="status"
          onClick={() => setFailure(null)}
        />
      )}
      {queueNotice !== null && (
        <div
          className="composer-queue-notice"
          id="composer-queue-notice"
          data-offline={queueOffline ? "true" : "false"}
        >
          <span className="composer-queue-dot" />
          <div className="composer-queue-text">{queueNotice}</div>
        </div>
      )}
      {queueSlot !== undefined && queueSlot !== null && (
        <div className="composer-queue-tray">{queueSlot}</div>
      )}
      {/*
        The floating new-thread selector row (composer.rs:7898-7913): the
        container is relative and the row absolute so the selectors never
        change the composer's height — 20px tall, 28px above the surface's
        top, `left/right` 26 from the column's edges, right-justified. The
        chips themselves are ticket 10's.
      */}
      {chrome.newThread > 0 && (
        <div
          className="dock-target-selectors"
          id="dock-target-selectors"
          style={{ opacity: `var(--rb-dock-chrome-new, ${chrome.newThread})` }}
        >
          <NewThreadTargetSelectors />
        </div>
      )}
      <div className="composer-surface" id="composer-surface">
        {wizardActive && wizardRef.current !== null ? (
          <ComposerWizard
            wizard={wizardRef.current}
            typedEmpty={text.length === 0}
            inputSlot={inputStack}
            onSelect={wizardSelect}
            onAdvance={isPhone ? wizardAdvanceCommit : wizardAdvance}
            onBack={wizardBack}
            onKeyDown={onWizardKeyDown}
            panelRef={wizardPanelRef}
          />
        ) : (
          <>
            {/*
              The pill. ONE DOM shape for both modes; the compact row and the
              expanded column are the same tree re-laid-out by `[data-mode]`
              CSS, with the animated numbers inline.
            */}
            <div
              className="composer-pill"
              data-mode={expandedRender ? "expanded" : "compact"}
              ref={pillRef}
              style={{
                // Ticket 57b: the glide's animated height and radius ride
                // the evaluate pass's CSS vars (written per frame while the
                // dock drives); the fallback is the last PUBLISHED layout —
                // settled between glides, clobber-proof mid-glide.
                height: `var(--rb-dock-pill-height, ${layout.pillHeight}px)`,
                ...(surfaceRadius !== null
                  ? { borderRadius: `var(--rb-dock-pill-radius, ${surfaceRadius.toFixed(2)}px)` }
                  : {}),
              }}
              onMouseDown={onPillMouseDown}
            >
              <CommentsChip count={commentCount} />
              <AttachmentStrip
                chatId={chat.id}
                staged={staged}
                onStage={onStage}
                onRemove={onRemove}
                onError={onStageError}
                pickerRef={attachRef}
              />
              <div className="composer-body">
                {/*
                  The paperclip (e0c1e936): compact, FIRST in the row at
                  `pl-12` — attach LEFT / input / model + Send right
                  (composer.rs:7856-7904); expanded, absolute at the pill's
                  stationary bottom-left beside the model chip, riding the
                  same cluster-dy channel the actions row glides on
                  (composer.rs:7824-7839) — the row's pb-3 plus the 2px
                  centering slack of a 28px button in its 32px content box.
                */}
                <button
                  type="button"
                  className="composer-attach"
                  aria-label="Attach"
                  onClick={onAttachClick}
                  style={
                    expandedRender
                      ? {
                          left: 12,
                          bottom: `calc(14px + var(--rb-dock-cluster-dy, ${-layout.clusterDy}px))`,
                        }
                      : {
                          top: `var(--rb-dock-cluster-dy, ${-layout.clusterDy}px)`,
                          marginLeft: 12,
                        }
                  }
                >
                  <Icon name="paperclip" size={16} />
                </button>
                <div
                  className="composer-input-box"
                  ref={inputBoxRef}
                  style={
                    expandedRender
                      ? {
                          // Tickets 57b/74: the glide's box height and text
                          // padding ride the evaluate pass's CSS vars; the
                          // fallbacks are the last published layout.
                          height: `var(--rb-dock-box-height, ${layout.boxHeight}px)`,
                          paddingTop: `var(--rb-dock-text-pad, ${layout.textPad}px)`,
                        }
                      : // The glide var carries the already-negated offset.
                        { top: `var(--rb-dock-text-glide, ${-layout.textGlide}px)` }
                  }
                >
                  {inputStack}
                </div>
                <div
                  className="composer-actions"
                  ref={actionsRef}
                  style={
                    // The glide vars carry the already-negated dy offset.
                    expandedRender
                      ? {
                          bottom: `var(--rb-dock-cluster-dy, ${-layout.clusterDy}px)`,
                          paddingRight: `var(--rb-dock-cluster-inset, ${layout.clusterInset}px)`,
                        }
                      : {
                          top: `var(--rb-dock-cluster-dy, ${-layout.clusterDy}px)`,
                          paddingRight: `var(--rb-dock-cluster-inset, ${layout.clusterInset}px)`,
                        }
                  }
                >
                  <div
                    className="composer-model-slot"
                    ref={modelSlotRef}
                    style={{
                      left: `var(--rb-dock-model-left, ${layout.modelLeft}px)`,
                      opacity: `var(--rb-dock-model-opacity, ${layout.modelOpacity})`,
                    }}
                  >
                    {/*
                      The model chip's handoff slot (e0c1e936,
                      composer.rs:410-416): the chip fades between its two
                      horizontal anchors on the flip instead of sweeping
                      across the prompt — `left`/`opacity` ride the layout
                      pass's published values (or the glide's CSS vars), and
                      the slot shrink-wraps the chip so its measured width
                      feeds `model_travel` (the desktop's `model_bounds`
                      canvas). The card's new-chat placement is ticket 04's.
                    */}
                    <ComposerPickers
                      catalog={catalog}
                      draft={draft}
                      chatConfig={chat.config}
                      newChat={newChat}
                      onDraft={applyDraft}
                      onPersist={persistDraft}
                      escapeFocusTarget={() => textareaRef.current}
                      onOpenChange={setPickersOpen}
                    />
                  </div>
                  {/*
                    A 28px filled circle — up-arrow to send or queue, a dark
                    rounded square on the same light circle to stop
                    (`render_send_button`, composer.rs:7106). No label, no
                    tooltip; blocked sends dim to 0.35 with no click handler;
                    Stop is never blocked.
                  */}
                  <button
                    type="button"
                    className={`composer-send ${mode === "stop" ? "composer-send-stop" : ""}`}
                    onClick={() => (mode === "stop" ? void interrupt() : void submit())}
                    disabled={blocked}
                    aria-label={sendAriaLabel}
                  >
                    {mode === "stop" ? (
                      <span className="composer-stop-square" />
                    ) : (
                      <Icon name="arrowUp" size={14} />
                    )}
                  </button>
                </div>
              </div>
              {/*
                The text-width mirror: `white-space: pre` +
                `width: max-content` make its offsetWidth the unwrapped
                widest-line width. The font stack MUST match
                `.composer-input` so the number matches what the textarea
                wraps at.
              */}
              <div ref={measureRef} className="composer-input-measure" aria-hidden="true">
                {text}
              </div>
            </div>
            {/*
              The two completion popups — mutually exclusive by token shape
              (`/` at offset 0 vs `@` at a token boundary), so at most one
              is ever mounted. Absolutely positioned children of the
              composer surface, spanning the pill's width above it.
            */}
            {mention.token !== null && (
              <MentionPopup
                token={mention.token}
                results={mention.results}
                active={mention.active}
                loading={mention.loading}
                error={mention.error}
                onAccept={(ix) => {
                  setMention((current) => (current.active === ix ? current : { ...current, active: ix }));
                  acceptMention(ix);
                }}
                onDismiss={dismissCompletion}
                onCardMouseDown={() => textareaRef.current?.focus()}
              />
            )}
            {slash.token !== null && (
              <SlashPopup
                token={slash.token}
                commands={slash.harness !== null ? slashCacheRef.current.get(slash.harness) ?? [] : []}
                filtered={slash.filtered}
                active={slash.active}
                loading={slash.loading}
                error={slash.error}
                onAccept={(rowIx) => {
                  setSlash((current) => (current.active === rowIx ? current : { ...current, active: rowIx }));
                  acceptSlash(rowIx);
                }}
                onDismiss={dismissCompletion}
                onCardMouseDown={() => textareaRef.current?.focus()}
              />
            )}
          </>
        )}
        {/* The hovered chip's path tooltip (§2.3, ticket 18): the shared
            `ui/Tooltip` in virtual-anchor mode — 24px tall, 480px max, mono
            11px, 1px off the chip (above when there is room, flush below
            otherwise). The phase machine (the manual mirror hit-test the
            pointer-transparent textarea forces) drives the mount; the
            family owns the portal, the popup, and the positioning. */}
        {tooltipAnchor !== null && (
          <Tooltip
            label={tooltipAnchor.path}
            open
            anchor={tooltipAnchor.anchor}
            placement={{
              side: tooltipAnchor.side,
              align: "start",
              sideOffset: tooltipAnchor.side === "top" ? 1 : 0,
            }}
            popupClassName="mention-tooltip"
            popupRef={tooltipElRef}
          />
        )}
      </div>
      {/*
        The 24px footer slot (composer.rs:7936-7985, SESSION_FOOTER_HEIGHT):
        two absolutely-inset layers that never overlap — Layer A is the
        new-thread git selectors (checkout + ref chips for the run target,
        `px` 10), Layer B the session footer row. `route_chrome_opacities`
        guarantees one is exactly 0, so the zero one is UNMOUNTED (its
        popover triggers cannot be found twice). Non-Git canvas targets
        collapse the slot to nothing (bottom_slot = session_chrome there).
      */}
      {(chrome.newThread > 0 || chrome.session > 0) && footerSlot !== undefined && (
        <div className="composer-footer-slot">
          {chrome.newThread > 0 && (
            <div
              className="composer-footer-layer composer-footer-layer-a"
              style={{ opacity: `var(--rb-dock-chrome-new, ${chrome.newThread})` }}
            >
              <NewThreadGitSelectors />
            </div>
          )}
          {chrome.session > 0 && (
            <div
              className="composer-footer-layer composer-footer-layer-b"
              style={{ opacity: `var(--rb-dock-chrome-session, ${chrome.session})` }}
            >
              {footerSlot}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function NOOP(): void {}

/** Stage a queued row's already-committed attachment from the shared cache
 *  (`begin_queue_edit`'s loaded set, queue.rs:1300-1314). Null on a cache
 *  miss — the bytes only live engine-side until a load. */
function stagedFromCache(deviceId: string | null, path: string): StagedAttachment | null {
  if (deviceId === null) {
    return null;
  }
  const snapshot = getAttachmentSnapshot(deviceId, path);
  if (snapshot.state !== "loaded" || snapshot.image === null) {
    return null;
  }
  return stageBytes(snapshot.image.name, snapshot.image.bytes);
}

/** Whether two completion tokens are equal (range + query). */
function tokensEqual(
  a: CompletionToken | null,
  b: CompletionToken | null,
): boolean {
  if (a === b) {
    return true;
  }
  if (a === null || b === null) {
    return false;
  }
  return a.start === b.start && a.end === b.end && a.query === b.query;
}

/** The non-newline whitespace following a token, if any (`replace_mention`'s
 * existing-separator rule, composer.rs:1893-1895). */
function separatorAfter(text: string, at: number): string | null {
  if (at >= text.length) {
    return null;
  }
  const ch = text[at];
  return ch !== undefined && ch !== "\n" && ch !== "\r" && /\s/.test(ch) ? ch : null;
}

/** The tooltip identity's path form: the full workspace-relative path, with
 * a trailing `/` for directories (composer.rs:3486-3491). */
function mentionTargetPath(chip: { link: { path: string; isDir: boolean } }): string {
  return `${chip.link.path}${chip.link.isDir ? "/" : ""}`;
}

/** Decode a `SearchFiles` reply, tolerating a malformed payload. */
function parseFileMatches(reply: unknown): readonly FileSearchMatch[] | null {
  if (!Array.isArray(reply)) {
    return null;
  }
  const matches: FileSearchMatch[] = [];
  for (const entry of reply) {
    if (typeof entry !== "object" || entry === null) {
      return null;
    }
    const candidate = entry as Record<string, unknown>;
    if (typeof candidate.path !== "string" || typeof candidate.isDir !== "boolean") {
      return null;
    }
    matches.push({ path: candidate.path, isDir: candidate.isDir });
  }
  return matches;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

/** `prefers-reduced-motion` at first paint (snap every morph). */
function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}
