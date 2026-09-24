import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
} from "react";
import type { EngineClient, EngineStatus } from "@zeron/engine-client";
import { methods } from "@zeron/engine-client";
import { Icon } from "@zeron/icons";
import { motion } from "@zeron/theme";
import { cubicBezierEval, useBottomClearance } from "../state/layout";
import { DESKTOP_QUERY, useMediaQuery } from "../state/media";
import type { ContextUsage, FetchToolBlobReply, SessionMessageEntry } from "@zeron/proto";
import {
  echoStore,
  pendingSendStatus,
  savedViewportCache,
  TranscriptStore,
  type PendingSend,
} from "../state/transcript-store";
import { transcriptFoldCache } from "../state/transcript-fold-state";
import { NoticeChip } from "./notice-chip";
import { useUiSettings } from "../state/ui-settings";
import {
  CODE_BLOCK_LINE_HEIGHT_BASELINE,
  codeBlockLineHeight,
  diffLineHeight,
} from "../lib/typography";
import { useNow } from "../state/hooks";
import { withAttachments } from "../lib/attachments";
import { parseMarkdown, blockFlatText, type Block, type InlineRun } from "../lib/markdown";
import {
  COPIED_CLEAR_MS,
  OWN_SEND_SCROLL_SLACK_PX,
  OWN_SEND_TOP_INSET_PX,
  SELECTION_SCROLL_TICK_MS,
  TITLEBAR_HEIGHT,
  TRANSCRIPT_FADE_BAND,
  USER_COLLAPSED_LINES,
  USER_HOLD_DELAY_MS,
  USER_LINE_HEIGHT,
  captureSavedViewport,
  chipsHeight,
  flavourSeed,
  flavourWord,
  formatElapsed,
  formatTimestamp,
  ownTurnReleasedForRestore,
  parseForRow,
  resolveViewportAnchor,
  rowsForEntry,
  selectionDragAutoscrolls,
  SelectionDragTracker,
  selectionScrollStep,
  sendingBridge,
  SPACE_LG,
  topGapFor,
  toolGroupCollapses,
  userMessageNeedsCollapse,
  userResizeCurve,
  userResizeDurationMs,
  visibleRowWindow,
  type SavedViewport,
  type SentMentionSpan,
  type TranscriptRow,
} from "../lib/transcript";
import {
  computeToolMeasurementKeys,
  EMPTY_TOOL_GEOMETRY_STATE,
  pruneStaleToolMeasurements,
  toolGroupGeometry,
  type ToolGroupEstimateContext,
} from "../lib/tool-group-geometry";
import {
  armToolFoldCompensation,
  automaticToolFoldCompensationArms,
  toolFoldCompensationDone,
  toolFoldCompensationWrite,
  type ToolFoldCompensation,
} from "../lib/tool-fold-scroll";
import { railTicks, type RailTick } from "../lib/rail";
import { OVERDRAW_PX } from "../lib/stick-spring";
import { ChatArrivalWindow } from "../lib/chat-arrival";
import { VeilTracker } from "../lib/veil";
import type { ChatIndicator } from "../lib/view";
import type { MessageBadge } from "../lib/badges";
import { MarkdownBlockView, MarkdownSurfaceProvider, CodeBlock, InlineRunView, type MarkdownSurface, type VeilChunk } from "./markdown";
import { MessageBadges } from "./badges";
import { MessageRail } from "./message-rail";
import { StickController } from "./stick-controller";
import { ToolGroupRow, type SubagentOpen } from "./tool-group";
import { ToolGroupMotionStore } from "../lib/tool-motion";
import { UserAttachments } from "./attachments/user-attachments";
import { MatrixSpinner } from "./glyph-spinner";
import { WorkingTrailer, type WorkingTrailerState } from "./working-trailer";

/** Line cap for a FETCHED full output (defensive; desktop FULL_OUTPUT_MAX_LINES). */
const FULL_OUTPUT_MAX_LINES = 400;

/** Row 0's gap (transcript.rs:5359): the titlebar chrome + breathing room. */
const FIRST_ROW_GAP = TITLEBAR_HEIGHT + SPACE_LG + 10;
/** The subagent override's row-0 gap — its surface already pads the titlebar. */
const FIRST_ROW_GAP_SUBAGENT = SPACE_LG;
/** Collapsed bubble geometry (transcript.rs:1771): 5 lines + the ellipsis line. */
const USER_COLLAPSED_TEXT_HEIGHT = USER_COLLAPSED_LINES * USER_LINE_HEIGHT;
const USER_COLLAPSED_HEIGHT = USER_COLLAPSED_TEXT_HEIGHT + USER_LINE_HEIGHT;
/** Trailer estimate for unmounted last rows: `pt(16)` + one 12px line. */
const TRAILER_ESTIMATE_HEIGHT = 28;
/** Desktop widths — the underlay layout and the clearance pad apply here.
 *  `DESKTOP_QUERY` and the `useMediaQuery` hook live in `state/media.ts`
 *  now — the one shared breakpoint module (ticket 49). */

/** The jump pill's visibility + action, published up to the chat page. */
export interface JumpButtonState {
  readonly shown: boolean;
  readonly jump: () => void;
}

/**
 * The chat transcript — virtualization at block granularity over the row model
 * of `../lib/transcript.ts` (the desktop's `rows_for_entry` port), with the
 * stick-to-bottom spring and the own-turn runway driving the scroller. Rows
 * are accounted by measured heights (estimates until rendered, 320px
 * overdraw), so a thousand-message chat keeps a bounded DOM and a stable
 * viewport.
 *
 * Memoized (ticket 57b): every prop is stable across the page's mid-glide
 * commits (the flip, a chrome mount crossing, the settle — the discrete
 * publishes of the de-Reacted dock pump), so the pump's phase events and the
 * async loads that re-render the page no longer descend into the transcript
 * tree; the row renders ride the store's own subscription.
 */
export const TranscriptView = memo(function TranscriptView({
  client,
  docId,
  deviceId,
  onContextUsage,
  onRetryDelivery,
  onJumpChange,
  alignTop = false,
  indicator = null,
  turnStartedAt = null,
  store: sharedStore = null,
  onOpenSubagent,
  markdownSurface = null,
  deliveryDegraded = false,
}: {
  client: EngineClient;
  docId: string;
  deviceId: string | null;
  /**
   * Replicated context occupancy, published as it changes. The composer's
   * footer draws it (the desktop's `render_footer`), but this store owns the
   * only subscription that carries it — a second watch for one number would
   * be a second live stream per open chat.
   */
  onContextUsage?: (usage: ContextUsage | null) => void;
  /**
   * The working-trailer's failed-send branch: `retry_send` — restart the
   * grace clocks and re-deliver. Skipped entirely when the engine is not
   * connected.
   */
  onRetryDelivery?: () => void;
  /**
   * The jump-to-bottom button's live state (`transcript.jump_button_shown()`).
   * The pill itself lives over the composer — outside this component — so the
   * chat page owns where it renders; this callback is how it learns.
   */
  onJumpChange?: (state: JumpButtonState) => void;
  /**
    * The subagent override instance (`Transcript::for_doc`): aligns to the TOP,
    * never holds an own-turn runway, owns a top-only fade gated on overflow,
    * and reads its liveness off the doc itself. Ticket 19 mounts this in the
    * right pane.
    */
  alignTop?: boolean;
  /** The chat's live indicator (`indicator_for`) — gates the working trailer. */
  indicator?: ChatIndicator | null;
  /** The session row's `started_at` in epoch ms (the trailer's timer base). */
  turnStartedAt?: number | null;
  /**
   * A store owned by the host (the chat page passes the ONE transcript the
   * composer's wizard also reads, so a chat carries a single
   * `WatchDocMessages` stream). Null (default): this view owns its store —
   * the subagent pane's shape.
   */
  store?: TranscriptStore | null;
  /**
   * A spawn chip's open (`TranscriptEvent::OpenSubagent`): the host registers
   * the right-pane tab under the chat that owns the pane. Default: a no-op
   * (an unbound host can't open tabs).
   */
  onOpenSubagent?: (payload: SubagentOpen) => void;
  /**
   * The markdown host hooks (`RenderOptions` on the desktop): the chat's
   * workspace root and the internal-link action. Null (default, the subagent
   * dialog): workspace links render inert.
   */
  markdownSurface?: MarkdownSurface | null;
  /**
   * `chat_delivery_degraded`'s web arm (state.rs:877, minimal): the routed
   * engine's `WatchConnectivity` posture, threaded by the chat page from
   * the session's watch cache. Degraded delivery keeps the pending-send
   * overlay quiet ("Queued", never a false "Not delivered") however long
   * the send has waited. False (default): the subagent pane — a doc has
   * no pending sends.
   */
  deliveryDegraded?: boolean;
}) {
  const [store, setStore] = useState<TranscriptStore | null>(null);
  useEffect(() => {
    if (sharedStore !== null) {
      return;
    }
    const created = new TranscriptStore(client, docId);
    setStore(created);
    return () => {
      created.dispose();
      setStore((current) => (current === created ? null : current));
    };
  }, [client, docId, sharedStore]);
  // The desktop transcript renders NOTHING while it has no document — the
  // shell owns the empty/loading case, not this component.
  const active = sharedStore ?? store;
  if (active === null) {
    return null;
  }
  return (
    <TranscriptSurface
      key={active.docId}
      store={active}
      client={client}
      deviceId={deviceId}
      onContextUsage={onContextUsage}
      onRetryDelivery={onRetryDelivery}
      onJumpChange={onJumpChange}
      alignTop={alignTop}
      indicator={indicator}
      turnStartedAt={turnStartedAt}
      onOpenSubagent={onOpenSubagent}
      markdownSurface={markdownSurface}
      deliveryDegraded={deliveryDegraded}
    />
  );
});

function TranscriptSurface({
  store,
  client,
  deviceId,
  onContextUsage,
  onRetryDelivery,
  onJumpChange,
  alignTop,
  indicator,
  turnStartedAt,
  onOpenSubagent,
  markdownSurface,
  deliveryDegraded,
}: {
  store: TranscriptStore;
  client: EngineClient;
  deviceId: string | null;
  onContextUsage?: (usage: ContextUsage | null) => void;
  onRetryDelivery?: () => void;
  onJumpChange?: (state: JumpButtonState) => void;
  alignTop: boolean;
  indicator: ChatIndicator | null;
  turnStartedAt: number | null;
  onOpenSubagent?: (payload: SubagentOpen) => void;
  markdownSurface: MarkdownSurface | null;
  deliveryDegraded: boolean;
}) {
  const subscribe = useCallback((listener: () => void) => store.subscribe(listener), [store]);
  const getSnapshot = useCallback(() => store.getSnapshot(), [store]);
  const snapshot = useSyncExternalStore(subscribe, getSnapshot);

  const usage = snapshot.contextUsage;
  useEffect(() => {
    onContextUsage?.(usage);
  }, [usage, onContextUsage]);

  // `parse_for_row` state: incremental while live, handoff on settle, cache
  // for settled trees. Bounded like the entry cache below.
  const parseStateRef = useRef(new Map<string, { text: string; live: boolean; tree: ReturnType<typeof parseMarkdown> }>());
  const entryRowsCacheRef = useRef(new Map<string, { entry: SessionMessageEntry; rows: TranscriptRow[] }>());

  // Rows are rebuilt per entry only when the entry's identity changes; the
  // parse state keeps settled markdown trees shared across stream ticks.
  const rows = useMemo(() => {
    const cache = entryRowsCacheRef.current;
    const parseState = parseStateRef.current;
    // Bound the caches: a long-lived session re-parses after a prune.
    if (cache.size > 4096) {
      cache.clear();
    }
    if (parseState.size > 4096) {
      parseState.clear();
    }
    const out: TranscriptRow[] = [];
    for (const entry of snapshot.entries) {
      let hit = cache.get(entry.id);
      if (hit === undefined || hit.entry !== entry) {
        hit = {
          entry,
          rows: rowsForEntry(entry, {
            parse: (key, text, live) => parseForRow(parseState, key, text, live).tree,
          }),
        };
        cache.set(entry.id, hit);
      }
      out.push(...hit.rows);
    }
    return out;
  }, [snapshot.entries]);

  // The optimistic echo overlay (`AppState.echoes`). Each still-unconfirmed
  // send renders as an ORDINARY user bubble at the end of the list — the
  // position the real row will take the moment the host writes it back — so
  // the confirmation is a silent swap rather than a visible hand-off. Never a
  // section of its own.
  const docId = store.docId;
  const subscribeEchoes = useCallback((listener: () => void) => echoStore.subscribe(listener), []);
  const echoSnapshot = useCallback(() => echoStore.forChat(docId), [docId]);
  const pendingSends = useSyncExternalStore(subscribeEchoes, echoSnapshot);

  // One clock: the grace window needs ~10s resolution, a live trailer 1s.
  const lastEntry = snapshot.entries[snapshot.entries.length - 1];
  const subagentLive =
    alignTop &&
    snapshot.loaded &&
    lastEntry !== undefined &&
    (lastEntry.status === "streaming" || lastEntry.role === "user");
  const trailerTicks = indicator === "working" || pendingSends.length > 0 || subagentLive;
  const now = useNow(trailerTicks ? 1000 : ECHO_TICK_MS);

  // The tool groups' reveal epochs (`sync`, transcript.rs:4059-4116): rows
  // that ARRIVE after the replay baseline stagger in; the first POPULATED
  // frame after attach is that baseline — it clears every reveal and strips
  // the group folds' tween clocks, so replaying history and switching chats
  // never re-animate an existing task tree. The store is per-surface (the
  // desktop's per-Transcript fields): a subagent tab's sync can never touch
  // this chat's reveals.
  //
  // The chat-switch arrival window (ticket 58): ONE predicate shared by every
  // arrival gate — the scroller's restore/spring, the tool groups' fold
  // tweens, the shimmer arming. Armed at this surface's first loaded commit
  // (the `key={active.docId}` remount IS the chat switch — the outlet hands
  // the view the new store only once its first frame has landed), cleared
  // when the measurement cascade quiesces or its hard cap passes. The
  // desktop's switch is atomic (state.rs:1740-1792, shell.rs:1837-1862,
  // composer.rs:5849-5874); this window is the web's equivalent gate.
  const chatArrival = useMemo(() => new ChatArrivalWindow(), [store]);
  // Ticket 68 (decision option 1): the chat's remembered explicit fold pins
  // are reinstalled INTO the fresh motion store during the render that
  // creates it — before any rows render, before the scroller's first
  // layout effect, and therefore before the viewport restore's height
  // estimates read the pins ("restore fold choices before calculating the
  // restored viewport"). Restored pins are settled (no tween clocks) and
  // ride the shared geometry resolver, so a remembered closed pin overrides
  // autoOpen/arrivalPending without forking the effective-open formula.
  // The outgoing chat's pins are captured at surface teardown, before the
  // store is reset — the LRU lives in `state/transcript-fold-state.ts`.
  const engineKey = client.engineKey ?? "";
  const toolMotion = useMemo(() => {
    const motion = new ToolGroupMotionStore(chatArrival);
    if (!alignTop) {
      const saved = transcriptFoldCache.restore(engineKey, docId);
      if (saved !== null) {
        motion.restoreExplicitFolds(saved);
      }
    }
    return motion;
  }, [chatArrival, engineKey, docId, alignTop]);
  useEffect(() => {
    // StrictMode's simulated remount re-runs this effect after the cleanup
    // below reset the KEPT store instance; re-applying the pins keeps that
    // double mount as faithful as a real remount (idempotent — the snapshot
    // re-sets the same keys).
    if (!alignTop) {
      const saved = transcriptFoldCache.restore(engineKey, docId);
      if (saved !== null) {
        toolMotion.restoreExplicitFolds(saved);
      }
    }
    return () => {
      if (!alignTop) {
        transcriptFoldCache.capture(engineKey, docId, toolMotion.captureExplicitFolds());
      }
      toolMotion.reset();
      // StrictMode reuses this surface after its simulated cleanup. Its
      // emptied motion store needs the same current-history baseline as a
      // real return to a warm chat.
      consumedBaselineRef.current = 0;
      const current = store.getSnapshot();
      mountBaselineEntriesRef.current = current.loaded ? current.entries : null;
    };
  }, [toolMotion, engineKey, docId, alignTop, store]);
  // The reveal baseline (ticket 69) rides the store's durable accepted-reset
  // epoch, never an observed pending render: a generation swap commits the
  // pending window and the reset's populated frame in ONE task, so React may
  // never render the intermediate snapshot — and a same-generation resubscribe
  // resets without the generation moving at all. Each published baseline is
  // consumed exactly once, in a layout effect so the corrected rows land
  // before paint: FIRST the reset's own entries as the replay baseline (the
  // desktop's populated-baseline consume, transcript.rs:4063-4113 — reveal
  // and tween clocks clear, every reset tool counts as history), THEN the
  // current rows as the live delta, so a reset coalesced with later deltas in
  // one React batch still classifies genuinely post-reset tools as arrivals.
  const consumedBaselineRef = useRef(0);
  // A warm store can contain deltas newer than its last reset. Everything
  // already present at mount is history; only subsequent deltas arrive.
  const mountBaselineEntriesRef = useRef(snapshot.loaded ? snapshot.entries : null);
  useLayoutEffect(() => {
    const baseline = snapshot.baseline;
    if (baseline !== null && baseline.epoch > consumedBaselineRef.current) {
      consumedBaselineRef.current = baseline.epoch;
      // Ticket 80 — the authoritative reset IS an arrival. On the canvas
      // route the surface mounts blank, the cache seed's loaded commit arms
      // the window, and the live reset can land after that window fell —
      // its settle cascade then armed the stick spring and replayed fold
      // flips visibly mid-dock-fade. Re-arming on every accepted reset
      // gives the seed→reset handoff the same atomic settle a chat→chat
      // switch gets from its mount commit (reconnect resets get it too,
      // which is equally correct: settling must never visibly glide).
      if (baseline.provenance === "reset") {
        chatArrival.arm(performance.now());
      }
      toolMotion.sync(baselineRows(mountBaselineEntriesRef.current ?? baseline.entries), true);
      mountBaselineEntriesRef.current = null;
      toolMotion.sync(rows, false);
      return;
    }
    // `replay === "pending"` marks a transient window (a resubscribe waiting
    // for its reset): rows are kept through it, and a genuinely empty pending
    // frame (a fresh mount) is harmless.
    toolMotion.sync(rows, false, snapshot.replay === "pending");
  }, [rows, snapshot.baseline, snapshot.replay, toolMotion]);

  const allRows = useMemo(() => {
    if (pendingSends.length === 0) {
      return rows;
    }
    // Belt-and-braces against a duplicated bubble: the ack rides the same
    // frame that adds the real row, but the two live in different stores, so
    // never render an echo for an id the doc already carries.
    const confirmed = new Set(snapshot.entries.map((entry) => entry.id));
    const echoRows: TranscriptRow[] = [];
    for (const send of pendingSends) {
      if (confirmed.has(send.messageId)) {
        continue;
      }
      const built = rowsForEntry(echoEntry(send, deviceId), {
        pending: true,
        parse: (key, text, live) => parseMarkdown(text, live),
      });
      const row = built[0];
      if (row === undefined || row.rowKind.kind !== "user") {
        continue;
      }
      const undelivered = pendingSendStatus(send, now, deliveryDegraded) === "undelivered";
      echoRows.push({
        ...row,
        // Keep the diff key sensitive to the status flip so the row repaints.
        version: row.version * 2 + (undelivered ? 1 : 0),
        rowKind: { ...row.rowKind, undelivered },
      });
    }
    return echoRows.length === 0 ? rows : [...rows, ...echoRows];
  }, [rows, snapshot.entries, pendingSends, deviceId, now, deliveryDegraded]);

  // The rail's data (rail.rs:74-99): one tick per user prompt, doc order,
  // then the un-deduped optimistic echoes — matching row order.
  const ticks = useMemo(
    () => railTicks(snapshot.entries, pendingSends.map((send) => echoEntry(send, deviceId))),
    [snapshot.entries, pendingSends, deviceId],
  );

  // ── The working trailer's state (render_working_trailer, §2.11) ─────────
  const trailerState = useMemo<WorkingTrailerState>(() => {
    if (alignTop) {
      // A subagent doc has no Session row — liveness rides the doc itself:
      // the last entry streams, or a trailing user entry awaits its reply.
      // Frozen snapshots never spin.
      if (!subagentLive) {
        return { kind: "none" };
      }
      const elapsed = Math.max(0, Math.floor((now - lastEntry!.createdAt) / 1000));
      return {
        kind: "working",
        word: flavourWord(flavourSeed(docId), elapsed),
        elapsed: formatElapsed(elapsed),
      };
    }
    // Failed-send state first: past the grace window the trailer IS the retry
    // affordance, whatever the indicator fell back to. Degraded delivery
    // (chatDeliveryDegraded, threaded by the page) holds every send pending,
    // so an outage never fabricates this state.
    if (pendingSends.some((send) => pendingSendStatus(send, now, deliveryDegraded) === "undelivered")) {
      return { kind: "undelivered", onRetry: onRetryDelivery ?? (() => {}) };
    }
    if (indicator !== "working") {
      return { kind: "none" };
    }
    // The send→turn bridge: "Sending…" with no timer while the session row
    // still carries the previous turn's start; degraded delivery says so
    // instead of faking progress (transcript.rs:5273-5279 — "Queued — will
    // send automatically").
    const sendStarted =
      pendingSends.find((send) => pendingSendStatus(send, now, deliveryDegraded) === "pending")?.startedAtMs ?? null;
    if (sendingBridge(sendStarted, turnStartedAt)) {
      return deliveryDegraded ? { kind: "queued" } : { kind: "sending" };
    }
    const elapsed =
      turnStartedAt !== null ? Math.max(0, Math.floor((now - turnStartedAt) / 1000)) : 0;
    return {
      kind: "working",
      word: flavourWord(flavourSeed(docId), elapsed),
      elapsed: formatElapsed(elapsed),
    };
  }, [alignTop, subagentLive, lastEntry, now, docId, pendingSends, indicator, turnStartedAt, onRetryDelivery, deliveryDegraded]);

  return (
    <MarkdownSurfaceProvider
      value={
        markdownSurface ?? { workspaceRoot: null, openWorkspaceFile: () => {} }
      }
    >
      <TranscriptScroller
        rows={allRows}
        ticks={ticks}
        streaming={snapshot.streaming}
        loaded={snapshot.loaded}
        replay={snapshot.replay}
        error={snapshot.error}
        client={client}
        deviceId={deviceId}
        docId={docId}
        alignTop={alignTop}
        trailer={trailerState}
        onJumpChange={onJumpChange}
        onOpenSubagent={onOpenSubagent}
        toolMotion={toolMotion}
        chatArrival={chatArrival}
      />
    </MarkdownSurfaceProvider>
  );
}

/** How often the echo overlay re-checks the grace window. */
const ECHO_TICK_MS = 10_000;

/**
 * A pending send dressed as the transcript entry the host will eventually
 * write — same id (that is the whole dedupe contract), same author, same
 * attachment-refs trailer — so the row model builds an ordinary user bubble
 * from it and the confirmed row replaces it with no visual change.
 */
function echoEntry(send: PendingSend, deviceId: string | null): SessionMessageEntry {
  return {
    id: send.messageId,
    role: "user",
    parts: [
      {
        kind: "text",
        id: `${send.messageId}#echo`,
        text: withAttachments(send.text, send.attachmentPaths),
      },
    ],
    createdAt: send.startedAtMs,
    deviceId: deviceId ?? "",
  };
}

/**
 * The accepted baseline's rows (ticket 69): derived through the real row
 * model, but with an empty markdown tree — `rowsForEntry` consults the parser
 * only for text parts, so tool-group identity and counts (the only thing the
 * reveal baseline reads) are exact, and the live parse caches are never
 * perturbed by replayed history.
 */
const BASELINE_PARSE_TREE = parseMarkdown("", false);

function baselineRows(entries: readonly SessionMessageEntry[]): TranscriptRow[] {
  const out: TranscriptRow[] = [];
  for (const entry of entries) {
    out.push(...rowsForEntry(entry, { parse: () => BASELINE_PARSE_TREE }));
  }
  return out;
}

// ---------------------------------------------------------------------------
// Scroller: virtualization + stick-to-bottom + the own-turn runway
// ---------------------------------------------------------------------------

interface ScrollerProps {
  readonly rows: readonly TranscriptRow[];
  /** The rail's ticks (`railTicks` over the doc entries + pending echoes). */
  readonly ticks: readonly RailTick[];
  readonly streaming: boolean;
  readonly loaded: boolean;
  readonly replay: "pending" | "empty" | "populated";
  readonly error: string | null;
  readonly client: EngineClient;
  readonly deviceId: string | null;
  readonly docId: string;
  readonly alignTop: boolean;
  readonly trailer: WorkingTrailerState;
  readonly onJumpChange?: (state: JumpButtonState) => void;
  /** A spawn chip's open — the host registers the right-pane tab. */
  readonly onOpenSubagent?: (payload: SubagentOpen) => void;
  /** The surface's tool-group motion store (folds, reveals, blob fetches). */
  readonly toolMotion: ToolGroupMotionStore;
  /** The surface's chat-switch arrival window (ticket 58's ONE predicate). */
  readonly chatArrival: ChatArrivalWindow;
}

/** The user-bubble fold, lifted so virtualizer remounts never lose it. */
export interface UserFoldState {
  readonly open: boolean;
  readonly epoch: number;
  readonly toggledAt: number;
  readonly durationMs: number;
  /** Height at toggle time: `full_h` (was open) or `collapsed_h`. */
  readonly from: number;
  readonly expansion: number;
}

/** The fold compensation tween (`UserCollapseScroll`, transcript.rs:4430). */
interface CollapseScroll {
  readonly startedAt: number;
  readonly durationMs: number;
  readonly heightDelta: number;
  readonly rowIx: number;
  readonly initialTop: number;
  readonly targetTop: number;
}

function TranscriptScroller({
  rows,
  ticks,
  streaming,
  loaded,
  replay,
  error,
  client,
  deviceId,
  docId,
  alignTop,
  trailer,
  onJumpChange,
  onOpenSubagent,
  toolMotion,
  chatArrival,
}: ScrollerProps) {
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  // The code font size scales code-block and embedded-diff rows (render.rs /
  // changes.rs baselines, lib/typography.ts); the estimator's context carries
  // the same value the renderers paint against.
  const codeFontSize = useUiSettings().codeFontSize;
  // The configurable content column (settings.rs `transcript_width`, upstream
  // cbf2ad84): the `.trow-col` max-width lands through --rb-transcript-width,
  // and every cached row height was measured under the PREVIOUS column.
  const transcriptWidth = useUiSettings().transcriptWidth;
  // The scroller MOUNTS AND UNMOUNTS with the empty state (the transcript
  // renders nothing when empty, exactly like the desktop). The attach effect
  // below keys off this presence state so a scroller that mounts after the
  // empty period still gets the stick controller attached.
  const scrollerPresentRef = useRef(false);
  const [, bumpScrollerPresence] = useState(0);
  const scrollerRefCallback = useCallback((el: HTMLDivElement | null) => {
    scrollerRef.current = el;
    const present = el !== null;
    if (present !== scrollerPresentRef.current) {
      scrollerPresentRef.current = present;
      bumpScrollerPresence((tick) => tick + 1);
    }
  }, []);
  const heightsRef = useRef(new Map<string, number>());
  // Ticket 70's bounded cache boundary: the semantic geometry key each cached
  // tool-row measurement was taken UNDER (heightKeysRef), and the current
  // keys refreshed every render (toolKeysRef, also the observer's tag source).
  const heightKeysRef = useRef(new Map<string, string>());
  const toolKeysRef = useRef(new Map<string, string>());
  // The column width the current measurements were taken under (the web peer
  // of Transcript::content_width): a change drops every cached height —
  // `list.remeasure()` on the desktop — retaining row identity and every
  // animation/provenance state, so the ResizeObserver re-measures mounted
  // rows in place and unmounted rows fall back to estimates until remount.
  const measuredWidthRef = useRef(transcriptWidth);
  const anchorRef = useRef<{ id: string; offset: number; top: number } | null>(null);
  const rowsRef = useRef(rows);
  const positionsRef = useRef<readonly number[]>([]);
  const rowHeightsRef = useRef<readonly number[]>([]);
  const measuredTextRef = useRef(new Map<string, number>());
  const holdTimersRef = useRef(new Map<string, number>());
  const collapseScrollRef = useRef<CollapseScroll | null>(null);
  /**
   * Ticket 71 — the tool-fold viewport compensation: the clicked header (an
   * explicit fold) or the top visible row (an automatic closure while no
   * other owner is live) held at a fixed screen position for the fold
   * tween's duration. At most one compensator is live; user input and
   * explicit navigation cancel it synchronously.
   */
  const toolFoldScrollRef = useRef<ToolFoldCompensation | null>(null);
  const [toolFoldTick, bumpToolFold] = useState(0);
  const [, bumpMeasure] = useState(0);
  const [view, setView] = useState({ top: 0, height: 0 });
  const [showJump, setShowJump] = useState(false);
  const [hoveredEntry, setHoveredEntry] = useState<{ rowId: string; entryId: string } | null>(null);
  const [userFolds, setUserFolds] = useState<ReadonlyMap<string, UserFoldState>>(new Map());
  const [topFade, setTopFade] = useState(false);
  const [, bumpRunway] = useState(0);
  const [collapseTick, bumpCollapse] = useState(0);
  // The rail's container box (the wrap element) — the width is the rail's
  // visibility gate, the height clamps the hover preview card.
  const [railBox, setRailBox] = useState({ w: 0, h: 0 });
  const reduced =
    typeof globalThis.matchMedia === "function"
      ? globalThis.matchMedia("(prefers-reduced-motion: reduce)")
      : null;
  const isDesktop = useMediaQuery(DESKTOP_QUERY);

  // The engine connection drives the offline strip (§2.1); a null status is
  // the pre-first-connect state, which reads as "Reconnecting…".
  const subscribeEngineStatus = useCallback((listener: () => void) => client.onStatus(listener), [client]);
  const getEngineStatus = useCallback(() => client.status, [client]);
  const engineStatus = useSyncExternalStore(subscribeEngineStatus, getEngineStatus, () => null);

  // The shell's live bottom-chrome measurement (`set_bottom_clearance`).
  // The subagent pane has no bottom chrome to clear, so its clearance is 0;
  // at phone widths the transcript is a sibling of the stack, not an
  // underlay, so only the ordinary breathing room applies.
  const shellClearance = useBottomClearance();
  const lastRowPad = alignTop || !isDesktop ? 16 : shellClearance + TRANSCRIPT_FADE_BAND + 8;

  const stickRef = useRef<StickController | null>(null);
  if (stickRef.current === null) {
    stickRef.current = new StickController({
      onJumpVisibility: setShowJump,
      // The runway's floor is render-derived — install/retire must re-render
      // the virtualizer's height model.
      onOwnTurnChange: () => bumpRunway((tick) => tick + 1),
      // A user scroll stands down the fold compensations and any pending
      // long-press (`handle_scroll`'s synchronous cancels).
      onUserInput: () => {
        collapseScrollRef.current = null;
        toolFoldScrollRef.current = null;
        for (const timer of holdTimersRef.current.values()) {
          window.clearTimeout(timer);
        }
        holdTimersRef.current.clear();
      },
      // Explicit navigation (a user-fold toggle, the rail glide, the
      // selection auto-scroll) owns the viewport: the tool-fold
      // compensation stands down before it moves the list.
      onNavigation: () => {
        toolFoldScrollRef.current = null;
      },
      reducedMotion: reduced,
      arrival: chatArrival,
    });
  }
  const stick = stickRef.current;
  const stickOwnTurn = stick.ownTurn;

  // Prune measured heights for rows that no longer exist.
  const heights = heightsRef.current;
  if (heights.size > rows.length + 256) {
    const live = new Set(rows.map((row) => row.id));
    for (const id of heights.keys()) {
      if (!live.has(id)) {
        heights.delete(id);
        heightKeysRef.current.delete(id);
      }
    }
  }

  // A conversation-width change re-wraps every row through the CSS variable
  // without remounting any of them (transcript.rs `content_width != width`),
  // so prefix sums must not trust heights measured under the old column: drop
  // the whole cache — mounted rows re-measure through the observer on the
  // reflow, unmounted rows stand on estimates until they return.
  if (measuredWidthRef.current !== transcriptWidth) {
    measuredWidthRef.current = transcriptWidth;
    heightsRef.current.clear();
    heightKeysRef.current.clear();
  }

  // Bounded tool-row measurement validity (ticket 70 §2.4): a cached tool
  // measurement is valid only for the semantic geometry inputs it was taken
  // under — fold pins, the effective detail/invocation heights, the payload
  // selection, the affordance slot. A change in those inputs (an UNMOUNTED
  // row's in-flight blob fetch completing is the motivating case) drops ONLY
  // that row's cached height, so the analytic-exact estimate stands in until
  // the row remounts and re-measures. Unchanged inputs keep their
  // measurements; this pass fetches nothing and notifies no one.
  const toolKeys = computeToolMeasurementKeys(rows, toolMotion, diffLineHeight(codeFontSize));
  toolKeysRef.current = toolKeys;
  pruneStaleToolMeasurements(heights, heightKeysRef.current, toolKeys);

  // ── Height model ─────────────────────────────────────────────────────────
  // Row 0's gap carries the titlebar chrome (the primary instance spans
  // under the overlay titlebar); the subagent override keeps only the
  // ordinary turn gap.
  const gapFor = useCallback(
    (ix: number): number =>
      ix === 0
        ? alignTop
          ? FIRST_ROW_GAP_SUBAGENT
          : FIRST_ROW_GAP
        : topGapFor(rows[ix - 1] ?? null, rows[ix]!),
    [rows, alignTop],
  );

  // The own-turn reservation: while a runway is live the LAST row has a
  // minimum height, so the prompt can sit at the viewport top with the
  // scroll ending at the app's bottom (`set_tail_reservation`; the floor is
  // met by the bottom spacer — plain scrollable space, never painted
  // chrome). The reservation value is the desktop's
  // `own_send_inset − OWN_SEND_SCROLL_SLACK_PX − expansion`
  // (transcript.rs:3506-3509): the 2px slack reads as the app's bottom, and
  // the expansion term carries the anchor row's live Show-more fold tween
  // (:3490-3504) — revealing the prompt adds reading space (the scroll end
  // extends with it); only reply growth consumes the reservation. The floor
  // reads the CURRENT render's prefix sums, so measurement that lands
  // recomputes it in the same pass (the finalize recompute; the sums above
  // the last row never depend on the floor, so there is no cycle).
  const anchorIx =
    stickOwnTurn !== null
      ? rows.findIndex((row) => row.turnStart && row.entryId === stickOwnTurn.messageId)
      : -1;
  const lastIx = rows.length - 1;
  const viewportHeight = view.height;
  const trailerLive = trailer.kind !== "none";
  // The estimator's geometry inputs (ticket 70): the motion store through its
  // read-only view plus ONE caller-provided timestamp and the reduced flag,
  // so an estimate and the row's own render of the same state agree — time
  // is never read twice. Event handlers call this for a fresh timestamp.
  const toolEstimateContext = (): ToolGroupEstimateContext => ({
    state: toolMotion,
    now: performance.now(),
    reduced: reduced?.matches === true,
    diffLineHeight: diffLineHeight(codeFontSize),
    codeLineHeight: codeBlockLineHeight(codeFontSize),
  });
  const estimateCtx = toolEstimateContext();
  const anchorFold = anchorIx >= 0 ? userFolds.get(rows[anchorIx]!.id) ?? null : null;
  const anchorExpansion =
    anchorFold !== null
      ? userFoldExpansionHeight(anchorFold, performance.now(), reduced?.matches === true)
      : 0;

  // Row positions: prefix sums over measured heights (estimates until
  // rendered). The last row's height carries its clearance pad — and the
  // reservation floor while a runway is live. The floor rides the BOTTOM
  // SPACER (plain scrollable space, never painted chrome): the DOM row keeps
  // its natural height, so the spacer math below tracks natural heights
  // separately from the floored arithmetic.
  const positions: number[] = new Array(rows.length);
  const rowHeights: number[] = new Array(rows.length);
  const naturalHeights: number[] = new Array(rows.length);
  let total = 0;
  for (let ix = 0; ix < rows.length; ix++) {
    positions[ix] = total;
    const gap = gapFor(ix);
    const isLast = ix === lastIx;
    const natural =
      heights.get(rows[ix]!.id) ??
      estimateRowHeight(rows[ix]!, estimateCtx) + gap + (isLast ? lastRowPad : 0) + (isLast && trailerLive ? TRAILER_ESTIMATE_HEIGHT : 0);
    naturalHeights[ix] = natural;
    rowHeights[ix] = natural;
    total += natural;
  }
  const reservationFloor =
    anchorIx >= 0 && viewportHeight > 0
      ? ownTurnReservationFloor({
          anchorTop: positions[anchorIx] ?? 0,
          lastTop: positions[lastIx] ?? 0,
          viewport: viewportHeight,
          inset: StickController.ownSendInset(anchorIx),
          expansion: anchorExpansion,
        })
      : 0;
  if (lastIx >= 0 && reservationFloor > naturalHeights[lastIx]!) {
    rowHeights[lastIx] = reservationFloor;
    total += reservationFloor - naturalHeights[lastIx]!;
  }
  rowsRef.current = rows;
  positionsRef.current = positions;
  rowHeightsRef.current = rowHeights;
  // The lastTotalRef hold-open: an authoritative-empty transition (rows went
  // to 0 and stayed) must not shrink the content under the viewport — the
  // browser would clamp scrollTop to 0 and the released view would land at
  // the top (the desktop keeps its offset logical across replays; the web
  // needs the spacer). Mid-session rows never empty now (the store keeps
  // them through a resubscribe), so this only arms on a real empty state.
  const lastTotalRef = useRef(0);
  if (rows.length > 0) {
    lastTotalRef.current = total;
  }
  const layoutTotal = rows.length === 0 ? lastTotalRef.current : total;

  // The reservation no longer binds — the reply's natural content has filled
  // it (`tail_reservation_filled`).
  const lastNatural =
    lastIx >= 0
      ? (heights.get(rows[lastIx]!.id) ??
        estimateRowHeight(rows[lastIx]!, estimateCtx) + lastRowPad + (trailerLive ? TRAILER_ESTIMATE_HEIGHT : 0))
      : 0;
  const reservationFilled =
    anchorIx >= 0 &&
    viewportHeight > 0 &&
    (positions[lastIx] ?? 0) + lastNatural >=
      (positions[anchorIx] ?? 0) + viewportHeight - StickController.ownSendInset(anchorIx) + 0.5;

  // The controller's frame-time geometry: anchor position, fill state, the
  // row-top prefix sums the rail glide anchors against, and whether the row
  // list is mid-replay (a transient window — a missing anchor waits, never
  // retires the runway).
  const geometryRef = useRef({ anchor: null as { top: number; ix: number } | null, filled: false, positions: [] as readonly number[], transient: false });
  geometryRef.current = {
    anchor: anchorIx >= 0 ? { top: positions[anchorIx]!, ix: anchorIx } : null,
    filled: reservationFilled,
    positions,
    transient: !loaded || replay === "pending",
  };
  const anchorExpanded = anchorIx >= 0 && (userFolds.get(rows[anchorIx]!.id)?.open ?? false) === true;
  const anchorExpandedRef = useRef(anchorExpanded);
  anchorExpandedRef.current = anchorExpanded;

  // Shared ResizeObserver: row heights land here (border-box, so the gap
  // padding and the last row's clearance pad are included) — one bump per
  // batch.
  const observerRef = useRef<ResizeObserver | null>(null);
  if (observerRef.current === null && typeof ResizeObserver !== "undefined") {
    observerRef.current = new ResizeObserver((entries) => {
      let changed = false;
      for (const entry of entries) {
        const el = entry.target as HTMLDivElement;
        const id = el.dataset["rid"];
        if (id === undefined) {
          continue;
        }
        const height = entry.borderBoxSize?.[0]?.blockSize ?? el.getBoundingClientRect().height;
        if (Math.abs((heightsRef.current.get(id) ?? 0) - height) > 0.5) {
          heightsRef.current.set(id, height);
          // Tag tool rows with the semantic inputs this measurement
          // corresponds to (ticket 70's cache boundary).
          const toolKey = toolKeysRef.current.get(id);
          if (toolKey === undefined) {
            heightKeysRef.current.delete(id);
          } else {
            heightKeysRef.current.set(id, toolKey);
          }
          changed = true;
        }
      }
      if (changed) {
        // The settle cascade's heartbeat: extend the chat-switch arrival
        // window so its gates cover this batch's scroll corrections (ticket 58).
        chatArrival.noteMeasure(performance.now());
        bumpMeasure((tick) => tick + 1);
      }
    });
  }
  const rowElsRef = useRef(new Map<string, HTMLDivElement>());
  // The bounded height update's trigger (ticket 70 §2.4): a motion-store bump
  // can change an UNMOUNTED tool row's effective geometry (an in-flight blob
  // fetch completing, a ready-recency click) with no row-prop or DOM change
  // the scroller would otherwise observe. Re-key on the bump and re-render
  // only when some tool row's semantic inputs actually moved — the render
  // pass above then drops exactly the stale cached measurements.
  useEffect(() => {
    return toolMotion.subscribe(() => {
      const next = computeToolMeasurementKeys(rowsRef.current, toolMotion, diffLineHeight(codeFontSize));
      const prev = toolKeysRef.current;
      if (next.size !== prev.size || [...next].some(([id, key]) => prev.get(id) !== key)) {
        bumpMeasure((tick) => tick + 1);
      }
    });
  }, [toolMotion, codeFontSize]);
  const registerRow = useCallback((id: string, el: HTMLDivElement | null) => {
    const observer = observerRef.current;
    const els = rowElsRef.current;
    if (observer === null) {
      return;
    }
    if (el === null) {
      const prev = els.get(id);
      if (prev !== undefined) {
        observer.unobserve(prev);
        els.delete(id);
      }
      return;
    }
    els.set(id, el);
    observer.observe(el);
  }, []);

  // Attach/detach the stick controller, the viewport listeners, and the
  // selection drag's edge auto-scroll. Re-arms when the scroller (re)mounts —
  // an empty chat renders no scroller at all, and the first send brings one.
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (el === null) {
      return;
    }
    stick.setGeometry(() => geometryRef.current);
    stick.attach(el);
    setView({ top: el.scrollTop, height: el.clientHeight });
    // The rail's gate reads the TRANSCRIPT CONTAINER's (the wrap's) width —
    // never the viewport's (rail.rs:23, research §3.15 "Web implication").
    const wrap = el.parentElement;
    const readWrap = (): void => {
      if (wrap !== null) {
        setRailBox({ w: wrap.clientWidth, h: wrap.clientHeight });
      }
    };
    readWrap();
    const wrapResize = new ResizeObserver(readWrap);
    if (wrap !== null) {
      wrapResize.observe(wrap);
    }
    let raf = 0;
    const onScroll = (): void => {
      anchorRef.current = captureAnchor(el.scrollTop, rowsRef.current, positionsRef.current, heightsRef.current, toolEstimateContext());
      if (alignTop) {
        // The override instance's top fade is gated on real overflow
        // (transcript.rs:7648-7651): max_offset − distance_from_bottom > 1.
        const scrolledUnder = el.scrollTop > 1;
        setTopFade((current) => (current === scrolledUnder ? current : scrolledUnder));
      }
      if (raf !== 0) {
        return;
      }
      raf = requestAnimationFrame(() => {
        raf = 0;
        setView({ top: el.scrollTop, height: el.clientHeight });
      });
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    const resize = new ResizeObserver(() => {
      stick.kick();
      setView({ top: el.scrollTop, height: el.clientHeight });
    });
    resize.observe(el);

    // ── Selection drag edge auto-scroll (§3.7) ────────────────────────────
    // Native selection is acceptable on the web, but a drag pinned near the
    // top/bottom edge still needs to auto-scroll — the t² ramp at a 24ms
    // cadence. Ticket 78: armed ONLY by a primary-button press on
    // non-interactive content inside the scroller, tracked by the
    // `SelectionDragTracker` (window `pointermove` never arms — a hold with
    // micro-drift on the titlebar/composer/safe-area must not scroll the
    // chat), cleared on `pointerup` AND `pointercancel`, and the tick steps
    // only while a real text selection is active (the desktop's
    // `step_selection_scroll` rides a genuine selection drag).
    const drag = new SelectionDragTracker();
    const onPointerDown = (event: PointerEvent): void => {
      if (event.button !== 0) {
        return;
      }
      const target = event.target as Element | null;
      if (target !== null && target.closest("button, a, [role='button'], input, textarea")) {
        drag.pressInteractive();
        return;
      }
      drag.press(event.clientX, event.clientY);
    };
    const onPointerMove = (event: PointerEvent): void => {
      drag.move(event.buttons, event.clientX, event.clientY);
    };
    const clearDrag = (): void => {
      drag.clear();
    };
    el.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("pointermove", onPointerMove, { passive: true });
    window.addEventListener("pointerup", clearDrag, { passive: true });
    window.addEventListener("pointercancel", clearDrag, { passive: true });
    const selectionTimer = window.setInterval(() => {
      const position = drag.position;
      if (position === null || !selectionDragAutoscrolls(document.getSelection())) {
        return;
      }
      const rect = el.getBoundingClientRect();
      const step = selectionScrollStep({ top: rect.top, bottom: rect.bottom }, position);
      if (step === 0) {
        return;
      }
      // Auto-scroll is navigation: it releases the own-turn hold and unpins
      // (`begin_scroll_navigation` per the desktop's step_selection_scroll).
      stick.beginScrollNavigation();
      stick.writePreserving(el.scrollTop + step);
    }, SELECTION_SCROLL_TICK_MS);

    const observer = observerRef.current;
    return () => {
      el.removeEventListener("scroll", onScroll);
      el.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", clearDrag);
      window.removeEventListener("pointercancel", clearDrag);
      window.clearInterval(selectionTimer);
      wrapResize.disconnect();
      resize.disconnect();
      observer?.disconnect();
      stick.detach();
      if (raf !== 0) {
        cancelAnimationFrame(raf);
      }
    };
    // `scrollerPresentRef` flip bumps a state so this re-arms when the
    // scroller (re)mounts after the empty period.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stick, alignTop, scrollerPresentRef.current]);

  // Keep the controller's live-anchor input current.
  useEffect(() => {
    stick.setStreaming(streaming);
  }, [stick, streaming]);

  // ── Own-send: every new pending echo installs a runway (on_own_send) ────
  const ownSendsRef = useRef(new Set<string>());
  useLayoutEffect(() => {
    for (const row of rows) {
      if (row.rowKind.kind === "user" && row.rowKind.pending && !ownSendsRef.current.has(row.id)) {
        ownSendsRef.current.add(row.id);
        if (!alignTop) {
          // An echo row's id IS the client-minted message id.
          stick.onOwnSend(docId, row.id);
        }
      }
    }
  }, [rows, stick, docId, alignTop]);

  // ── Viewport memory: save on leave, restore after a populated replay ────
  const pendingViewportRef = useRef<SavedViewport | null>(null);
  const viewportInitRef = useRef(false);
  if (!viewportInitRef.current) {
    viewportInitRef.current = true;
    pendingViewportRef.current = alignTop ? null : (savedViewportCache.get(docId) ?? null);
  }
  const restoredRef = useRef(false);
  const arrivalArmedRef = useRef(false);
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (el === null || !loaded) {
      return;
    }
    // Ticket 58: arm the chat-switch arrival window at this surface's first
    // loaded commit. The surface remounts per doc (`key={active.docId}`) and
    // the outlet hands the view the new store only once its first frame has
    // landed — so this commit IS the arrival. Every arrival gate (the
    // spring, the fold tweens, the shimmer) consults the window until the
    // measurement cascade quiesces; the desktop's switch applies the same
    // state atomically in one frame (state.rs:1740-1792).
    if (!arrivalArmedRef.current) {
      arrivalArmedRef.current = true;
      chatArrival.arm(performance.now());
    }
    if (restoredRef.current) {
      return;
    }
    // Ticket 58 §2.1: resolve the saved viewport from THIS commit's
    // estimate-based prefix sums and assign it immediately — a hard
    // `scrollTop` write, no poll frames, no spring. The per-commit anchor
    // preserve below is the post-measure correction: when the anchor row's
    // measured position differs from the estimate it re-assigns (each
    // correction is a write, never a tween), exactly like the desktop's
    // `viewport_finalize` follow-up.
    restoredRef.current = true;
    applyRestoredViewport(el);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stick, loaded, replay, rows.length, alignTop, chatArrival]);

  /** Resolve and apply the saved viewport (or open at the end). */
  const applyRestoredViewport = (el: HTMLElement): void => {
    if (alignTop) {
      // The override instance opens at the TOP.
      setView({ top: el.scrollTop, height: el.clientHeight });
      return;
    }
    const saved = pendingViewportRef.current;
    if (saved === null) {
      // First fill lands at the bottom instantly (desktop opens at the end).
      stick.snapToEnd();
      setView({ top: el.scrollTop, height: el.clientHeight });
      return;
    }
    if (replay === "empty" && rowsRef.current.length === 0) {
      // An empty replay is authoritative — the chat really has no messages;
      // the saved snapshot retires and the tail is followed.
      pendingViewportRef.current = null;
      stick.snapToEnd();
      setView({ top: el.scrollTop, height: el.clientHeight });
      return;
    }
    if (saved.kind === "anchored" && rowsRef.current.length > 0) {
      // Fallbacks are enabled only after a populated replay; a loaded frame
      // with rows IS one (the store decides `replay` on the same frame).
      const offset = resolveViewportAnchor(saved.anchor, rowsRef.current, replay === "populated");
      if (offset !== null) {
        const scrollTop = positionsRef.current[offset.itemIx]! + offset.offsetInItem;
        stick.restoreViewport(
          scrollTop,
          saved.ownTurn === null ? null : ownTurnReleasedForRestore(saved.ownTurn),
          saved.distanceFromBottom,
        );
        // The restored row anchor doubles as the escape anchor: the
        // per-commit preserve keeps it stationary while late measurements
        // land (the desktop's viewport-finalize token). `top` is the
        // position the clamped write actually landed on — the delta
        // correction's zero point.
        anchorRef.current = { id: saved.anchor.rowId, offset: offset.offsetInItem, top: el.scrollTop };
        setView({ top: el.scrollTop, height: el.clientHeight });
        pendingViewportRef.current = null;
        return;
      }
    }
    pendingViewportRef.current = null;
    stick.snapToEnd();
    setView({ top: el.scrollTop, height: el.clientHeight });
  };

  // Save the outgoing chat's viewport. Empty rows never overwrite an older
  // snapshot (a partial replay must not); an unresolved pending restore
  // keeps the older entry.
  useEffect(() => {
    return () => {
      if (alignTop || pendingViewportRef.current !== null) {
        return;
      }
      const el = scrollerRef.current;
      if (el === null) {
        return;
      }
      const saved = captureSavedViewport(
        rowsRef.current,
        el.scrollTop,
        positionsRef.current,
        rowHeightsRef.current,
        stick.pinned,
        Math.max(0, el.scrollHeight - el.clientHeight - el.scrollTop),
        stick.ownTurn,
      );
      if (saved !== null) {
        savedViewportCache.save(docId, saved);
      }
    };
    // Capture at unmount: the refs are current and the next chat mounts
    // after this cleanup runs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docId, alignTop]);

  // Publish the jump button's state up to the chat page, which renders the
  // pill over the composer (`render_jump_to_bottom` floats outside the
  // transcript's fade, anchored above the composer stack). The cleanup hides
  // it again when the surface unmounts (chat switch).
  useEffect(() => {
    const state: JumpButtonState = {
      shown: showJump,
      jump: () => stickRef.current?.jumpToBottom({ anchorExpanded: anchorExpandedRef.current }),
    };
    onJumpChange?.(state);
    return () => onJumpChange?.({ ...state, shown: false });
  }, [showJump, onJumpChange]);

  // After every commit: a live runway re-arms its stepper (the desktop's
  // `own_turn_kick` on every sync — the fill-check and the post-layout
  // re-assert run per commit, never only from the rAF loop; transcript.rs
  // 7541-7549 schedules the step on every frame while an anchor is live, and
  // one commit per streamed delta is the web's equivalent cadence); pinned →
  // let the spring track growth; escaped → keep the captured anchor row
  // visually stationary across splices and measures. An animating fold owns
  // the viewport for the duration of its tween. This is the desktop's
  // escape-anchor semantics and it must never fight an active runway: the
  // guards below (own-turn hold, collapse scroll) are that contract, and the
  // height oscillation that used to shuttle the viewport between "hold
  // anchor" and "chase tail" now stops at the source (rows never empty
  // mid-session, replayed groups never re-reveal, open groups estimate their
  // open height).
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (el === null) {
      return;
    }
    // A live runway re-arms its stepper per commit (the desktop's
    // `own_turn_kick` on every sync — the fill-check and the post-layout
    // re-assert advance per streamed delta, transcript.rs:7541-7549); the
    // released reservation still needs the fill-check, so the kick fires for
    // any live anchor, held or not. While the chat-switch arrival window is
    // armed the kick writes the end directly (ticket 58 — the spring never
    // pre-empts the restore or glides through the settle cascade); a pending
    // viewport restore still owns the first frames for the un-loaded case.
    if (pendingViewportRef.current === null && (stick.ownTurn !== null || stick.pinned)) {
      stick.kick();
    }
    // A tool-fold compensator owns the viewport for its tween's duration
    // (ticket 71): the escape-anchor preserve below stands down for it —
    // one owner, no fighting writes.
    if (stick.ownTurnHeld || collapseScrollRef.current !== null || toolFoldScrollRef.current !== null) {
      return;
    }
    // Escaped (or a released runway): keep the captured anchor row visually
    // stationary across splices and measures. The correction is the CONTENT
    // DELTA — how far the anchor row's own position moved since capture —
    // added to the CURRENT scrollTop, never a teleport back to the capture
    // position: between a touch fling's scroll events the compositor keeps
    // advancing scrollTop, and a commit landing there would otherwise yank
    // the viewport back every frame (the mobile "content jumping up and
    // down" during fast swipes). With the user's own motion left untouched,
    // a still viewport gets exactly the old absolute correction.
    const anchor = anchorRef.current;
    if (stick.pinned || anchor === null) {
      return;
    }
    const ix = rows.findIndex((row) => row.id === anchor.id);
    if (ix < 0) {
      return;
    }
    const contentDelta = positions[ix]! + anchor.offset - anchor.top;
    if (Math.abs(contentDelta) > 0.5) {
      stick.writePreserving(el.scrollTop + contentDelta);
      anchorRef.current = { ...anchor, top: anchor.top + contentDelta };
    }
  });

  // ── User-fold compensation (step_user_collapse_scroll, §2.5) ────────────
  useEffect(() => {
    if (collapseScrollRef.current === null) {
      return;
    }
    let raf = 0;
    const step = (): void => {
      raf = 0;
      const scroll = collapseScrollRef.current;
      const el = scrollerRef.current;
      if (scroll === null || el === null) {
        return;
      }
      const raw = Math.min(Math.max((performance.now() - scroll.startedAt) / scroll.durationMs, 0), 1);
      const curve =
        (motion.curves[userResizeCurve(scroll.heightDelta)] as readonly [number, number, number, number]) ??
        motion.curves.easeOut!;
      const progress = cubicBezierEval(curve, raw);
      const desiredTop = scroll.initialTop + (scroll.targetTop - scroll.initialTop) * progress;
      const rowTop = (positionsRef.current[scroll.rowIx] ?? 0) - el.scrollTop;
      const correction = rowTop - desiredTop;
      if (Math.abs(correction) > 0.1) {
        stick.writePreserving(el.scrollTop + correction);
      }
      if (raw >= 1) {
        collapseScrollRef.current = null;
        return;
      }
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => {
      if (raf !== 0) {
        cancelAnimationFrame(raf);
      }
    };
    // Armed by the toggle's bump; the loop reads live refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stick, collapseTick]);

  // ── Tool-fold scroll ownership (ticket 71, step_user_collapse_scroll's
  // tool-group peer) ─────────────────────────────────────────────────────
  // The compensation keeps ONE anchor at a FIXED screen position for the
  // fold tween's duration: an explicit group/detail click anchors the
  // CLICKED header (measured from the click event, before the fold state
  // flips); an automatic closure (thought completion, auto-open expiry)
  // anchors the reading position — the top visible row — and only while no
  // other owner is live. Each frame's write corrects drift only (a browser
  // clamp near the scroll end, a leftover write), so reduced motion needs
  // no separate branch: the geometry snaps and the anchor correction land
  // in the same frame.

  /** `toggle_fold`/chip-toggle's navigation (ticket 71 A): the click owns the viewport. */
  const onToolFoldNav = useCallback(
    (nav: { rowId: string; header: HTMLElement }) => {
      const el = scrollerRef.current;
      if (el === null) {
        return;
      }
      const ix = rowsRef.current.findIndex((row) => row.id === nav.rowId);
      if (ix < 0) {
        return;
      }
      // Measured tool-row geometry, never guessed user-row heights: the
      // clicked header's rect against the scroller's, plus the live row
      // top from the virtualizer's prefix sums.
      const scrollerTop = el.getBoundingClientRect().top;
      const headerScreenY = nav.header.getBoundingClientRect().top - scrollerTop;
      const offsetInRow = headerScreenY + el.scrollTop - (positionsRef.current[ix] ?? 0);
      // Release the follow/hold FIRST — the reservation survives as
      // scrollable space; beginScrollNavigation also cancels any running
      // compensation (ours included, hence arming after it).
      stick.beginScrollNavigation();
      toolFoldScrollRef.current = armToolFoldCompensation({
        rowId: nav.rowId,
        offsetInRow,
        screenY: headerScreenY,
        now: performance.now(),
      });
      bumpToolFold((tick) => tick + 1);
    },
    [stick],
  );

  // An AUTOMATIC fold transition arms the reading-anchor compensation only
  // while no other owner is live (§2.3): a pinned tail keeps its
  // tail-follow, a held runway keeps its hold, an escaped reading position
  // keeps its per-commit preserve, and a pending viewport restore owns the
  // first frames of an unloaded chat.
  useEffect(() => {
    return toolMotion.onAutomaticFoldTransition(() => {
      if (
        !automaticToolFoldCompensationArms({
          pinned: stick.pinned,
          ownTurnHeld: stick.ownTurnHeld,
          userFoldCompensating: collapseScrollRef.current !== null,
          escapeAnchor: anchorRef.current !== null,
          pendingViewportRestore: pendingViewportRef.current !== null,
        })
      ) {
        return;
      }
      const el = scrollerRef.current;
      if (el === null) {
        return;
      }
      const anchor = captureAnchor(
        el.scrollTop,
        rowsRef.current,
        positionsRef.current,
        heightsRef.current,
        toolEstimateContext(),
      );
      if (anchor === null) {
        return;
      }
      const ix = rowsRef.current.findIndex((row) => row.id === anchor.id);
      if (ix < 0) {
        return;
      }
      toolFoldScrollRef.current = armToolFoldCompensation({
        rowId: anchor.id,
        offsetInRow: anchor.offset,
        screenY: (positionsRef.current[ix] ?? 0) + anchor.offset - el.scrollTop,
        now: performance.now(),
      });
      bumpToolFold((tick) => tick + 1);
    });
    // The loop reads live refs; the controllers are stable per mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [toolMotion, stick]);

  // The per-frame step: re-resolve the row (splices above it re-target the
  // index), write the drift correction, yield to a live pin/hold (never
  // fight tail-follow), and stand down when the tween has played out.
  useEffect(() => {
    if (toolFoldScrollRef.current === null) {
      return;
    }
    let raf = 0;
    const step = (): void => {
      raf = 0;
      const comp = toolFoldScrollRef.current;
      const el = scrollerRef.current;
      if (comp === null || el === null) {
        return;
      }
      if (stick.pinned || stick.ownTurnHeld) {
        // A live tail-follow or runway re-engaged mid-tween: it owns the
        // viewport now — cancel, never fight.
        toolFoldScrollRef.current = null;
        return;
      }
      const ix = rowsRef.current.findIndex((row) => row.id === comp.rowId);
      if (ix < 0) {
        toolFoldScrollRef.current = null;
        return;
      }
      const write = toolFoldCompensationWrite(comp, {
        rowTop: positionsRef.current[ix] ?? 0,
        scrollTop: el.scrollTop,
      });
      if (write !== null) {
        stick.writePreserving(write);
      }
      if (toolFoldCompensationDone(comp, performance.now())) {
        toolFoldScrollRef.current = null;
        return;
      }
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => {
      if (raf !== 0) {
        cancelAnimationFrame(raf);
      }
    };
    // Armed by the bump; the loop reads live refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stick, toolFoldTick]);

  /** `toggle_user_fold` (transcript.rs:4380-4440). */
  const toggleUserFold = useCallback(
    (rowId: string) => {
      const el = scrollerRef.current;
      const ix = rowsRef.current.findIndex((row) => row.id === rowId);
      if (el === null || ix < 0) {
        return;
      }
      const measured = measuredTextRef.current.get(rowId) ?? 0;
      const fullH = Math.max(measured, USER_COLLAPSED_HEIGHT);
      const currentlyOpen = userFolds.get(rowId)?.open ?? false;
      const durationMs = userResizeDurationMs(fullH - USER_COLLAPSED_HEIGHT);
      // A fold toggle is scroll navigation first: the pending viewport is
      // discarded, the hold stands down, the pin drops.
      stick.beginScrollNavigation();
      setUserFolds((current) => {
        const prev = current.get(rowId);
        const next = new Map(current);
        next.set(rowId, {
          open: !currentlyOpen,
          epoch: (prev?.epoch ?? 0) + 1,
          toggledAt: performance.now(),
          durationMs,
          from: currentlyOpen ? fullH : USER_COLLAPSED_HEIGHT,
          expansion: Math.max(0, fullH - USER_COLLAPSED_HEIGHT),
        });
        return next;
      });
      // Viewport compensation: keep the toggled row on an interpolated
      // screen-space path so the rows below it do not jump.
      const initialTop = (positionsRef.current[ix] ?? 0) - el.scrollTop;
      const viewportTop = TRANSCRIPT_FADE_BAND + 28;
      const targetHeight = currentlyOpen ? USER_COLLAPSED_HEIGHT : fullH;
      const viewportBottom = el.clientHeight - targetHeight - 12;
      const targetTop =
        viewportBottom >= viewportTop
          ? Math.min(Math.max(initialTop, viewportTop), viewportBottom)
          : viewportTop;
      if (Math.abs(targetTop - initialTop) <= 0.5) {
        return;
      }
      if (reduced?.matches) {
        const rowTop = (positionsRef.current[ix] ?? 0) - el.scrollTop;
        stick.writePreserving(el.scrollTop + (rowTop - targetTop));
        return;
      }
      collapseScrollRef.current = {
        startedAt: performance.now(),
        durationMs,
        heightDelta: Math.max(0, fullH - USER_COLLAPSED_HEIGHT),
        rowIx: ix,
        initialTop,
        targetTop,
      };
      bumpCollapse((tick) => tick + 1);
    },
    [userFolds, stick, reduced],
  );

  /** The bubble's wrapped-text measurement (0.5px threshold, §2.7). */
  const onMeasureText = useCallback((rowId: string, height: number) => {
    const prev = measuredTextRef.current.get(rowId) ?? 0;
    if (Math.abs(prev - height) > 0.5) {
      measuredTextRef.current.set(rowId, height);
    }
  }, []);

  /** Long-press bookkeeping: the scroller owns the timers so a user scroll
   *  can cancel them all (`handle_scroll` cancels the pending hold). */
  const onHoldTimer = useCallback((rowId: string, timer: number): void => {
    const prev = holdTimersRef.current.get(rowId);
    if (prev !== undefined && prev !== timer) {
      window.clearTimeout(prev);
    }
    if (timer === 0) {
      holdTimersRef.current.delete(rowId);
    } else {
      holdTimersRef.current.set(rowId, timer);
    }
  }, []);

  // Empty transcript: nothing at all, as on the desktop. The shell's new-chat
  // hero (ticket 15) is what will occupy this space.
  if (loaded && rows.length === 0 && error === null) {
    return null;
  }

  // The visible window, with the desktop's 320px overdraw on both ends.
  const { first, last } = visibleRowWindow(positions, rowHeights, view.top, view.height, OVERDRAW_PX);
  const visible = rows.slice(first, last + 1);
  // The rail's reading-line row: the raw clip top advanced past every
  // MEASURED row whose top is at or above `viewport.top + 48 + 0.5` — the
  // titlebar overlays the list and the own-turn hold parks the newest prompt
  // exactly at that inset, so crediting the raw top row kept the previous
  // tick lit for the whole runway (rail.rs:437-457). Unmeasured rows stop
  // the walk.
  const topRow = readingTopRow(rows, positions, heights, view.top, estimateCtx);
  const topPad = rows.length === 0 ? layoutTotal : (positions[first] ?? total);
  // The spacer covers everything below the last MOUNTED row — the unmounted
  // rows' arithmetic heights plus the reservation floor, minus the mounted
  // last row's natural DOM height (the floor is spacer space, not row space).
  const bottomPad =
    rows.length === 0 ? 0 : (last >= 0 ? Math.max(0, total - (positions[last]! + (naturalHeights[last] ?? 0))) : 0);

  const offlineMessage = offlineStripMessage(alignTop, engineStatus);

  return (
    <div
      className={`transcript-wrap ${alignTop ? "transcript-subagent" : ""}`}
      data-offline={offlineMessage !== null ? "1" : "0"}
    >
      {offlineMessage !== null && (
        // The engine-offline strip (§2.1): a 24px bar ABOVE the list, not an
        // overlay — it replaces the invented floating error card.
        <div className="engine-offline-strip" role="status">
          {offlineMessage}
        </div>
      )}
      <div
        className="transcript"
        ref={scrollerRefCallback}
        data-topfade={alignTop ? (topFade ? "1" : "0") : undefined}
      >
        <div style={{ height: topPad }} aria-hidden />
        {visible.map((row, offset) => {
          const ix = first + offset;
          const isLast = ix === lastIx;
          return (
            <RowShell
              key={row.id}
              row={row}
              gap={gapFor(ix)}
              bottomPad={isLast ? lastRowPad : 0}
              register={registerRow}
              onHover={setHoveredEntry}
            >
              <RowContent
                row={row}
                hovered={hoveredEntry?.entryId === row.entryId}
                onOpenSubagent={onOpenSubagent}
                client={client}
                deviceId={deviceId}
                docId={docId}
                toolMotion={toolMotion}
                fold={userFolds.get(row.id) ?? null}
                onToggleFold={toggleUserFold}
                onMeasureText={onMeasureText}
                onHoldTimer={onHoldTimer}
                onToolFoldNav={onToolFoldNav}
                reduced={reduced?.matches === true}
              />
              {isLast && <WorkingTrailer state={trailer} />}
            </RowShell>
          );
        })}
        <div style={{ height: bottomPad }} aria-hidden />
      </div>
      {/* The rail overlay (render_rail, rail.rs:471): a 26px column at
          left:16 inside the wrap, mounted only outside the subagent surface
          (rail_enabled = doc_override.is_none(), transcript.rs:2835). */}
      {!alignTop && (
        <MessageRail
          ticks={ticks}
          rows={rows}
          topRow={topRow}
          viewportHeight={view.height}
          containerWidth={railBox.w}
          containerHeight={railBox.h}
          onJumpToRow={(row) => stick.scrollToRow(row)}
        />
      )}
      {/*
        The edge fade lives on the scroller's own mask (`.transcript` in
        app.css): the desktop's per-glyph EdgeFade is a mask, not a painted
        overlay, with a 24px quadratic band under the titlebar and a bottom
        band sized to the chrome stack via `--rb-bottom-stack` — correct now
        that the transcript spans the column and scrolls under the chrome
        (the shell.rs:6025-6072 underlay port).
      */}
    </div>
  );
}

/** The offline strip's message, from the engine connection state (§2.1). */
function offlineStripMessage(alignTop: boolean, status: EngineStatus | null): string | null {
  if (alignTop) {
    // An override instance has no chat row and no engine binding of its own.
    return null;
  }
  if (status === null || status.state === "connected") {
    return null;
  }
  return status.state === "parked" || status.state === "closed"
    ? "Engine off. Cached history is read-only."
    : "Reconnecting… Cached history is read-only.";
}

/** Capture the first visible row + its pixel offset — the escape anchor. */
function captureAnchor(
  top: number,
  rows: readonly TranscriptRow[],
  positions: readonly number[],
  heights: ReadonlyMap<string, number>,
  toolGeometry: ToolGroupEstimateContext | null = null,
): { id: string; offset: number; top: number } | null {
  for (let ix = 0; ix < rows.length; ix++) {
    const rowTop = positions[ix]!;
    const bottom = rowTop + (heights.get(rows[ix]!.id) ?? estimateRowHeight(rows[ix]!, toolGeometry));
    if (bottom > top + 1) {
      return { id: rows[ix]!.id, offset: top - rowTop, top };
    }
  }
  return null;
}

/**
 * The rail's reading-line row (rail.rs:446-457): the first row whose bottom
 * crosses the scroll top, then a walk forward over MEASURED rows whose tops
 * are at or above `top + OWN_SEND_TOP_INSET_PX + 0.5`. Unmeasured rows (no
 * rendered bounds) stop the walk, leaving the raw top row.
 */
function readingTopRow(
  rows: readonly TranscriptRow[],
  positions: readonly number[],
  heights: ReadonlyMap<string, number>,
  scrollTop: number,
  toolGeometry: ToolGroupEstimateContext | null = null,
): number {
  let topRow = Math.max(rows.length - 1, 0);
  for (let ix = 0; ix < rows.length; ix++) {
    const bottom = positions[ix]! + (heights.get(rows[ix]!.id) ?? estimateRowHeight(rows[ix]!, toolGeometry));
    if (bottom > scrollTop + 0.5) {
      topRow = ix;
      break;
    }
  }
  const readTop = scrollTop + OWN_SEND_TOP_INSET_PX + 0.5;
  while (
    topRow + 1 < rows.length &&
    heights.has(rows[topRow + 1]!.id) &&
    positions[topRow + 1]! <= readTop
  ) {
    topRow++;
  }
  return topRow;
}

/** No-store estimate context: no folds, reveals, or blobs — bare row data. */
const DEFAULT_TOOL_ESTIMATE_CONTEXT: ToolGroupEstimateContext = {
  state: EMPTY_TOOL_GEOMETRY_STATE,
  now: 0,
  reduced: false,
};

/**
 * First-frame estimate per row kind; measurement corrects on render. The
 * `toolGroup` branch is analytic (the desktop needs no estimation at all —
 * its rows are analytic, transcript.rs:96-136) and, since ticket 70, shares
 * the renderer's ONE geometry resolver (`toolGroupGeometry`): a collapsed
 * group is its 26px header, a spawn-only group is its unwrapped chips (the
 * standalone 38px CHIP_HEIGHT is preserved), and a group that will render
 * OPEN — the explicit pin, else `autoOpen` OR an in-flight arrival —
 * estimates header + the 32px rail rows + each open chip's effective
 * detail/invocation/affordance additions, so mounting or replaying a long
 * group does not lurch 26 → full body. `toolGeometry` carries the surface's
 * read-only motion state and ONE timestamp per pass; null (pure callers)
 * reads bare row data.
 */
export function estimateRowHeight(
  row: TranscriptRow,
  toolGeometry: ToolGroupEstimateContext | null = null,
): number {
  const kind = row.rowKind;
  switch (kind.kind) {
    case "user": {
      const lines = Math.min(Math.max(1, kind.text.split("\n").length), 5);
      return 20 + lines * 22 + 8;
    }
    case "markdown":
    case "liveMarkdown": {
      const block = kind.tree.blocks[kind.blockIx]?.block;
      if (block !== undefined && block.kind === "codeBlock") {
        // Code rows are nowrap: the height is analytic (render.rs constants),
        // scaled by the code font size the fence renders at.
        const lineHeight = toolGeometry?.codeLineHeight ?? CODE_BLOCK_LINE_HEIGHT_BASELINE;
        return block.code.split("\n").length * lineHeight + 28;
      }
      return 30;
    }
    case "toolGroup": {
      if (!toolGroupCollapses(kind.tools)) {
        return chipsHeight(kind.tools.length);
      }
      const ctx = toolGeometry ?? DEFAULT_TOOL_ESTIMATE_CONTEXT;
      return toolGroupGeometry({
        rowId: row.id,
        tools: kind.tools,
        autoOpen: kind.autoOpen,
        state: ctx.state,
        now: ctx.now,
        reduced: ctx.reduced,
        diffLineHeight: ctx.diffLineHeight,
      }).totalHeight;
    }
    case "inputChip":
    case "errorChip":
      return 42;
  }
}

/**
 * `update_runway_minimum`'s floor (transcript.rs:3483-3513): the LAST row's
 * minimum height while a runway is live. The reservation value is
 * `inset − OWN_SEND_SCROLL_SLACK_PX − expansion` (:3506-3509) — the 2px slack
 * keeps the held layout out of the shorter-than-viewport regime, and the
 * expansion term carries the anchor row's live Show-more fold tween — so the
 * content end reaches `anchorTop + viewport − reservation` and the scroll end
 * parks the anchor at the reservation value. The space itself is plain
 * scrollable whitespace (the bottom spacer), never painted chrome.
 */
export function ownTurnReservationFloor(input: {
  readonly anchorTop: number;
  readonly lastTop: number;
  readonly viewport: number;
  readonly inset: number;
  readonly expansion: number;
}): number {
  const reservation = input.inset - OWN_SEND_SCROLL_SLACK_PX - input.expansion;
  return Math.max(0, input.anchorTop + input.viewport - reservation - input.lastTop);
}

/**
 * The anchor row's live fold-tween height (transcript.rs:3490-3504): the
 * tweened value of the prompt's expansion beyond its collapsed height —
 * `lerp(expansion − target, target, progress)` over the user-resize curve,
 * the target being the full expansion while open and 0 while closed, so an
 * opening tween grows the term and Show-less decays it back. Reduced motion
 * or a degenerate duration snaps to the target (the desktop's `_` arm).
 */
export function userFoldExpansionHeight(fold: UserFoldState, now: number, reduced: boolean): number {
  const target = fold.open ? fold.expansion : 0;
  if (reduced || fold.durationMs <= 0) {
    return target;
  }
  const raw = Math.min(Math.max((now - fold.toggledAt) / fold.durationMs, 0), 1);
  const curve = motion.curves[userResizeCurve(fold.expansion)] as
    | readonly [number, number, number, number]
    | undefined;
  const progress = cubicBezierEval(curve ?? motion.curves.easeOut!, raw);
  return fold.expansion - target + (2 * target - fold.expansion) * progress;
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

function RowShell({
  row,
  gap,
  bottomPad,
  register,
  onHover,
  children,
}: {
  row: TranscriptRow;
  gap: number;
  bottomPad: number;
  register: (id: string, el: HTMLDivElement | null) => void;
  onHover: Dispatch<SetStateAction<{ rowId: string; entryId: string } | null>>;
  children: ReactNode;
}) {
  const ref = useCallback((el: HTMLDivElement | null) => register(row.id, el), [register, row.id]);
  return (
    // Wide gutters (zeron `px-4 @3xl:px-12`) around the configurable
    // conversation-width column; the last row's bottom pad clears the chrome
    // the list scrolls under.
    <div
      ref={ref}
      data-rid={row.id}
      className="trow"
      style={{ paddingTop: gap, paddingBottom: bottomPad > 0 ? bottomPad : undefined }}
      onMouseEnter={() => onHover({ rowId: row.id, entryId: row.entryId })}
      onMouseLeave={() =>
        // Only the row that OWNS the current reveal may clear it — a stale
        // leave from an earlier row must not blank the strip the newly
        // entered row just lit (transcript.rs:5652-5662).
        onHover((current) => (current !== null && current.rowId === row.id ? null : current))
      }
    >
      <div className="trow-col">{children}</div>
    </div>
  );
}

function RowContent({
  row,
  hovered,
  onOpenSubagent,
  client,
  deviceId,
  docId,
  toolMotion,
  fold,
  onToggleFold,
  onMeasureText,
  onHoldTimer,
  onToolFoldNav,
  reduced,
}: {
  row: TranscriptRow;
  hovered: boolean;
  onOpenSubagent?: (payload: SubagentOpen) => void;
  client: EngineClient;
  deviceId: string | null;
  docId: string;
  toolMotion: ToolGroupMotionStore;
  fold: UserFoldState | null;
  onToggleFold: (rowId: string) => void;
  onMeasureText: (rowId: string, height: number) => void;
  onHoldTimer: (rowId: string, timer: number) => void;
  onToolFoldNav?: (nav: { rowId: string; header: HTMLElement }) => void;
  reduced: boolean;
}) {
  const kind = row.rowKind;
  return (
    <>
      {kind.kind === "user" && (
        <UserRow
          rowId={row.id}
          text={kind.text}
          mentions={kind.mentions}
          pending={kind.pending}
          undelivered={kind.undelivered === true}
          attachments={kind.attachments}
          badges={kind.badges}
          client={client}
          deviceId={deviceId}
          fold={fold}
          onToggle={() => onToggleFold(row.id)}
          onMeasure={(height) => onMeasureText(row.id, height)}
          onHoldTimer={onHoldTimer}
          reduced={reduced}
        />
      )}
      {kind.kind === "markdown" && <MarkdownRow row={row} />}
      {kind.kind === "liveMarkdown" && <LiveMarkdownRow row={row} />}
      {kind.kind === "toolGroup" && (
        <ToolGroupRow
          rowId={row.id}
          tools={kind.tools}
          autoOpen={kind.autoOpen}
          chatId={docId}
          motion={toolMotion}
          onOpenSubagent={onOpenSubagent ?? (() => {})}
          onFoldNav={onToolFoldNav}
          client={client}
        />
      )}
      {kind.kind === "inputChip" && <InputChipRow header={kind.header} resolved={kind.resolved} />}
      {kind.kind === "errorChip" && <ErrorChipRow message={kind.message} />}
      {row.timestamp !== null && <RowMeta row={row} visible={hovered} isUserRow={kind.kind === "user"} />}
    </>
  );
}

/** The hover strip under a settled entry's last row: timestamp + copy. */
function RowMeta({ row, visible, isUserRow }: { row: TranscriptRow; visible: boolean; isUserRow: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = (): void => {
    const text = row.copyText;
    const clipboard = globalThis.navigator?.clipboard;
    if (text === null || clipboard === undefined) {
      return;
    }
    void clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), COPIED_CLEAR_MS);
    });
  };
  return (
    // A RESERVED lane (always occupies its 32px) so the virtualizer never
    // shifts; only the contents' visibility flips.
    <div className={`row-meta ${isUserRow ? "row-meta-user" : ""} ${visible ? "row-meta-on" : ""}`}>
      <div className="row-meta-inner">
        <span className="row-meta-time">{formatTimestamp(row.timestamp ?? 0)}</span>
        {row.copyText !== null && (
          <button
            type="button"
            className="row-meta-copy"
            onClick={(event) => {
              event.stopPropagation();
              copy();
            }}
            aria-label="Copy message"
          >
            <Icon name={copied ? "check" : "copy"} size={14} />
          </button>
        )}
      </div>
    </div>
  );
}

// ── User bubble (§2.4-§2.7) ─────────────────────────────────────────────────

function UserRow({
  rowId,
  text,
  mentions,
  pending,
  undelivered = false,
  attachments,
  badges,
  client,
  deviceId,
  fold,
  onToggle,
  onMeasure,
  onHoldTimer,
  reduced,
}: {
  rowId: string;
  text: string;
  mentions: readonly SentMentionSpan[];
  pending: boolean;
  undelivered?: boolean;
  attachments: readonly import("../lib/attachments").UserImageAttachment[];
  badges: readonly MessageBadge[];
  client: EngineClient;
  deviceId: string | null;
  fold: UserFoldState | null;
  onToggle: () => void;
  onMeasure: (height: number) => void;
  onHoldTimer: (rowId: string, timer: number) => void;
  reduced: boolean;
}) {
  const textRef = useRef<HTMLDivElement | null>(null);
  const holdTimerRef = useRef(0);
  const [measured, setMeasured] = useState(0);
  const measuredRef = useRef(0);

  // The bubble's wrapped height, measured from the DOM (the desktop writes
  // it from the paint canvas in `user_bubble_text`); deltas ≤ 0.5px are
  // ignored so idle layout never feeds back.
  useEffect(() => {
    const el = textRef.current;
    if (el === null || typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry === undefined) {
        return;
      }
      const height = entry.borderBoxSize?.[0]?.blockSize ?? el.getBoundingClientRect().height;
      if (Math.abs(measuredRef.current - height) > 0.5) {
        measuredRef.current = height;
        setMeasured(height);
        onMeasure(height);
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [onMeasure]);

  const fullH = Math.max(measured, USER_COLLAPSED_HEIGHT);
  // `collapsible` (:4705): the wrapped-line count supersedes the first-frame
  // proxy once measurement exists.
  const collapsible =
    text.length > 0 &&
    (text.split("\n").length > USER_COLLAPSED_LINES ||
      (measured > 0 && measured > USER_COLLAPSED_TEXT_HEIGHT + 0.5) ||
      (measured === 0 && userMessageNeedsCollapse(text)));
  const expanded = fold?.open ?? false;
  const durationMs = fold?.durationMs ?? userResizeDurationMs(Math.max(0, fullH - USER_COLLAPSED_HEIGHT));
  // The animating window (§2.5): only apply the height transition within
  // `duration + 200ms` of the toggle, keyed by epoch — past it the fold
  // renders statically, so an armed-forever tween never replays on a
  // scroll-back-into-view remount.
  const [animating, setAnimating] = useState(false);
  useEffect(() => {
    if (fold === null || fold.epoch === 0 || reduced) {
      setAnimating(false);
      return;
    }
    setAnimating(true);
    const timer = window.setTimeout(() => setAnimating(false), fold.durationMs + 200);
    return () => window.clearTimeout(timer);
    // A new epoch re-arms; the window's end is the timer's job.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fold?.epoch, reduced]);

  // ── Long-press toggle (USER_HOLD_DELAY, :4451-4484) ──────────────────────
  // Releasing before the threshold preserves a click/selection; ANY move
  // cancels so a drag-select never unexpectedly toggles the fold.
  const cancelHold = useCallback((): void => {
    if (holdTimerRef.current !== 0) {
      onHoldTimer(rowId, 0);
      window.clearTimeout(holdTimerRef.current);
      holdTimerRef.current = 0;
    }
  }, [onHoldTimer, rowId]);
  useEffect(() => cancelHold, [cancelHold]);
  const onHoldPointerDown = (event: React.PointerEvent): void => {
    if (event.button !== 0 || !collapsible) {
      return;
    }
    cancelHold();
    holdTimerRef.current = window.setTimeout(() => {
      holdTimerRef.current = 0;
      onToggle();
    }, USER_HOLD_DELAY_MS);
    onHoldTimer(rowId, holdTimerRef.current);
  };

  // Image-only sends show no bubble (desktop parity); the thumbnail strip
  // above is the whole row.
  if (text.trim().length === 0 && attachments.length === 0) {
    return null;
  }
  // The badges strip above the bubble (badges.rs::render's mount,
  // transcript.rs:5405-5423): right-aligned wrap, one pill per badge. The
  // strip container is ticket 18's; the pills are ticket 20's.
  return (
    <div className="row-user">
      <div className={`user-content ${pending && !undelivered ? "user-bubble-pending" : ""}`}>
        <UserAttachments client={client} deviceId={deviceId} attachments={attachments} />
        <MessageBadges badges={badges} />
        {text.trim().length > 0 && (
          <div className="user-bubble-row">
            <div className={`user-bubble ${pending && !undelivered ? "user-bubble-pending" : ""}`}>
              <div
                className="user-body"
                onPointerDown={onHoldPointerDown}
                onPointerMove={cancelHold}
                onPointerUp={cancelHold}
                onPointerCancel={cancelHold}
                onPointerLeave={cancelHold}
              >
                {collapsible ? (
                  <UserFoldedBody
                    fold={fold}
                    animating={animating}
                    expanded={expanded}
                    fullH={fullH}
                    durationMs={durationMs}
                    reduced={reduced}
                    textRef={textRef}
                  >
                    <UserText text={text} mentions={mentions} />
                  </UserFoldedBody>
                ) : (
                  <div className="user-text" ref={textRef}>
                    <UserText text={text} mentions={mentions} />
                  </div>
                )}
              </div>
              {collapsible && (
                <button
                  type="button"
                  className="user-expand"
                  aria-label={expanded ? "Collapse message" : "Expand message"}
                  aria-expanded={expanded}
                  onClick={onToggle}
                >
                  <span className="user-expand-label">{expanded ? "Show less" : "Show more"}</span>
                  <Icon name={expanded ? "altArrowUp" : "altArrowDown"} size={12} className="user-expand-icon" />
                </button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * The collapsible body: a clip wrapper whose height is `collapsed_text_h`
 * while settled-collapsed, the full text while expanded, and the tweened
 * `from → to` (minus the ellipsis line while collapsed) inside the animating
 * window — the ellipsis row rides outside the clip, inside the tween's
 * endpoints, so removing it never jumps the layout (§2.5).
 */
function UserFoldedBody({
  fold,
  animating,
  expanded,
  fullH,
  durationMs,
  reduced,
  textRef,
  children,
}: {
  fold: UserFoldState | null;
  animating: boolean;
  expanded: boolean;
  fullH: number;
  durationMs: number;
  reduced: boolean;
  textRef: React.RefObject<HTMLDivElement | null>;
  children: ReactNode;
}) {
  const [phase, setPhase] = useState<"from" | "to" | null>(null);
  useEffect(() => {
    if (!animating || fold === null || reduced) {
      setPhase(null);
      return;
    }
    // Phase 1: render at the pre-toggle height with no transition; phase 2
    // (next frame): flip to the target under the resize spec's curve. A new
    // epoch restarts the sequence cleanly.
    setPhase("from");
    let raf = requestAnimationFrame(() => {
      raf = 0;
      setPhase("to");
    });
    return () => {
      cancelAnimationFrame(raf);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fold?.epoch, animating, reduced]);

  const ellipsisH = expanded ? 0 : USER_LINE_HEIGHT;
  let height: number | undefined;
  let transition: string | undefined;
  if (animating && phase !== null) {
    const from = fold?.from ?? USER_COLLAPSED_HEIGHT;
    const to = expanded ? fullH : USER_COLLAPSED_HEIGHT;
    height = Math.max(0, (phase === "from" ? from : to) - ellipsisH);
    if (phase === "to") {
      const curve = userResizeCurve(Math.max(0, fullH - USER_COLLAPSED_HEIGHT));
      const curveVar = curve === "easeInOut" ? "--rb-ease-ease-in-out" : "--rb-ease-ease-out";
      transition = `height ${durationMs}ms var(${curveVar})`;
    } else {
      transition = "none";
    }
  } else if (!expanded) {
    height = USER_COLLAPSED_TEXT_HEIGHT;
  }
  return (
    <div className="user-body-inner">
      <div className="user-text-clip" style={{ height, transition }}>
        <div className="user-text" ref={textRef}>
          {children}
        </div>
      </div>
      {!expanded && <div className="user-ellipsis">...</div>}
    </div>
  );
}

/** The bubble's text runs, mention spans rendered as chips (§2.7). */
function UserText({ text, mentions }: { text: string; mentions: readonly SentMentionSpan[] }) {
  if (mentions.length === 0) {
    return <>{text}</>;
  }
  const out: ReactNode[] = [];
  let at = 0;
  mentions.forEach((mention, ix) => {
    if (mention.start > at) {
      out.push(text.slice(at, mention.start));
    }
    out.push(
      <span key={ix} className="user-mention">
        {text.slice(mention.start, mention.end)}
      </span>,
    );
    at = mention.end;
  });
  if (at < text.length) {
    out.push(text.slice(at));
  }
  return <>{out}</>;
}

// ── Markdown rows ───────────────────────────────────────────────────────────

const MarkdownRow = memo(function MarkdownRow({ row }: { row: TranscriptRow }) {
  const kind = row.rowKind;
  if (kind.kind !== "markdown" && kind.kind !== "liveMarkdown") {
    return null;
  }
  const block = kind.tree.blocks[kind.blockIx]?.block;
  if (block === undefined) {
    return null;
  }
  return (
    <div className="row-md">
      <MarkdownBlockView block={block} />
    </div>
  );
});

/** A streaming markdown row with the fade veil. */
function LiveMarkdownRow({ row }: { row: TranscriptRow }) {
  const kind = row.rowKind;
  const block = kind.kind === "liveMarkdown" ? kind.tree.blocks[kind.blockIx]?.block : undefined;
  const veils =
    block !== undefined && (block.kind === "paragraph" || block.kind === "heading" || block.kind === "codeBlock")
      ? block
      : null;
  const flat = veils === null ? null : blockFlatText(veils);
  const trackerRef = useRef<VeilTracker | null>(null);
  const [chunks, setChunks] = useState<readonly VeilChunk[]>([]);

  useLayoutEffect(() => {
    if (flat === null) {
      return;
    }
    if (trackerRef.current === null) {
      const tracker = new VeilTracker();
      tracker.seed(flat);
      trackerRef.current = tracker;
      return;
    }
    const active = trackerRef.current.advance(flat, performance.now());
    setChunks(active.map((chunk) => ({ key: `${chunk.start}:${chunk.started}`, ...chunk })));
  }, [flat]);

  if (block === undefined) {
    return null;
  }

  const dropChunk = (key: string): void => {
    setChunks((current) => current.filter((chunk) => chunk.key !== key));
  };

  const codeBlock = block.kind === "codeBlock" ? block : null;
  return (
    <div className="row-md row-md-live">
      {flat !== null && chunks.length > 0 && veils !== null && (veils.kind === "paragraph" || veils.kind === "heading") ? (
        <VeiledBlock block={veils} chunks={chunks} onChunkEnd={dropChunk} />
      ) : codeBlock !== null && flat !== null && chunks.length > 0 ? (
        <CodeBlock code={codeBlock.code} language={codeBlock.language} chunks={chunks} onChunkEnd={dropChunk} />
      ) : (
        <MarkdownBlockView block={block} />
      )}
    </div>
  );
}

/** A paragraph/heading whose tail chunks fade in (React-managed splits). */
function VeiledBlock({
  block,
  chunks,
  onChunkEnd,
}: {
  block: Extract<Block, { kind: "paragraph" | "heading" }>;
  chunks: readonly VeilChunk[];
  onChunkEnd: (key: string) => void;
}) {
  const pieces = splitRunsForVeil(block.runs, chunks);
  const content = pieces.map((piece, ix) => {
    const inner = <InlineRunView run={piece.run} />;
    if (piece.chunk === null) {
      return <span key={`p${ix}`} className="veil-plain">{inner}</span>;
    }
    return (
      <span
        key={piece.chunk.key}
        className="veil-fade"
        style={{ animationDuration: `${piece.chunk.durationMs}ms` }}
        onAnimationEnd={() => onChunkEnd(piece.chunk!.key)}
      >
        {inner}
      </span>
    );
  });
  if (block.kind === "heading") {
    const level = Math.min(6, Math.max(1, block.level));
    const Tag = `h${level}` as "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
    return <Tag className={`md-h md-h${level}`}>{content}</Tag>;
  }
  return <p className="md-p">{content}</p>;
}

/** Split inline runs at chunk boundaries (flat-text coordinates). */
function splitRunsForVeil(
  runs: readonly InlineRun[],
  chunks: readonly VeilChunk[],
): Array<{ run: InlineRun; chunk: VeilChunk | null }> {
  const out: Array<{ run: InlineRun; chunk: VeilChunk | null }> = [];
  let offset = 0;
  for (const run of runs) {
    const runStart = offset;
    const runEnd = offset + run.text.length;
    offset = runEnd;
    const covering = chunks.filter((chunk) => chunk.start < runEnd && chunk.end > runStart);
    if (covering.length === 0) {
      out.push({ run, chunk: null });
      continue;
    }
    const cuts = new Set<number>([runStart, runEnd]);
    for (const chunk of covering) {
      cuts.add(Math.max(runStart, chunk.start));
      cuts.add(Math.min(runEnd, chunk.end));
    }
    const sorted = [...cuts].sort((a, b) => a - b);
    for (let ix = 0; ix + 1 < sorted.length; ix++) {
      const piece: InlineRun = {
        text: run.text.slice(sorted[ix]! - runStart, sorted[ix + 1]! - runStart),
        style: run.style,
      };
      const chunk =
        covering.find((candidate) => candidate.start <= sorted[ix]! && sorted[ix + 1]! <= candidate.end) ?? null;
      if (piece.text.length > 0) {
        out.push({ run: piece, chunk });
      }
    }
  }
  return out;
}

/**
 * One styled inline run — `markdown.tsx`'s `InlineRunView` IS the shared
 * renderer now (the ticket collapsed this former `StyledRun` copy into it):
 * veiled rows, thought details and settled blocks all validate links, render
 * media and guard clicks identically.
 */

// ── Input and error chips (§2.9 / §2.10) ────────────────────────────────────

function InputChipRow({ header, resolved }: { header: string; resolved: boolean }) {
  return (
    <div className="input-chip-row">
      <div className="input-chip">
        <span className="input-chip-tile" aria-hidden>
          <Icon name="chatRoundLine" size={12} />
        </span>
        {/* Neutral tones throughout — resolution never recolors the chip. */}
        <span className="input-chip-label">Question</span>
        <span className="input-chip-text">{resolved ? header : "Awaiting your answer…"}</span>
      </div>
    </div>
  );
}

function ErrorChipRow({ message }: { message: string }) {
  // The shared stacked notice chip in its tile treatment (notice.rs). The
  // message WRAPS: a one-line ellipsis made a startup-crash report
  // undiagnosable (zeronsh/comet#95).
  return (
    <div className="error-chip-row">
      <NoticeChip tone="danger" variant="tile" label="Error" message={message} role="alert" />
    </div>
  );
}

// ── Shell chrome published from the transcript surface ──────────────────────

/**
 * The "↓ Scroll to bottom" pill — the desktop's `jump_pill` (`shell.rs:6192`):
 * a 30px rounded-full labeled chip, frosted (blur 16 under the floating-card
 * tint), hairline border, `↓` + label at 13px, paddings 11/13 around a 6px
 * gap. Reusable so ticket 07/19's subagent pane can host its second instance.
 *
 * The entrance (`dialog_in`: 180ms, opacity 0→1, top 2px→0) and the hover
 * wash live in the stylesheet.
 */
export function JumpPill({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" className="jump-pill" onClick={onClick}>
      <span className="jump-pill-inner">
        <span className="jump-pill-glyph" aria-hidden>
          ↓
        </span>
        <span className="jump-pill-label">Scroll to bottom</span>
      </span>
    </button>
  );
}

/**
 * The reserved status strip — the desktop's `render_status_strip`
 * (`shell.rs:6373`): 24px tall, ALWAYS reserved so the composer below never
 * shifts, aligned with the composer column (max-width 768, centered,
 * 24px inner gutters, 11px type). The working loader and the awaiting-input
 * surface live elsewhere now; what is left is the error word and the sending
 * indicator.
 */
export function StatusStrip({ status, sending }: { status: ChatIndicator; sending: boolean }) {
  return (
    <div className="status-strip" role="status">
      {status === "errored" ? (
        <span className="status-strip-error">Run failed</span>
      ) : sending ? (
        <>
          {/*
            The desktop's `gradient_spinner("sending-indicator", cell 2.5)`
            (shell.rs:6427): a 2.5px cell in a 3×3 grid → a 12.5px box
            (ticket 06 deferred the exact geometry here).
          */}
          <MatrixSpinner size={12.5} className="status-strip-spinner" />
          <span className="status-strip-sending">Sending…</span>
        </>
      ) : null}
    </div>
  );
}
