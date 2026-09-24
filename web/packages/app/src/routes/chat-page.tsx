import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Link, useNavigate, useRouterState } from "@tanstack/react-router";
import { Icon, harnessBrandIcon } from "@zeron/icons";
import { encodeScopedId, methods, parseScopedId } from "@zeron/engine-client";
import { MESSAGE_QUEUE_ACTIONS_V1 } from "@zeron/proto";
import type { Chat, QueuedMessage } from "@zeron/proto";
import type { ChangeRequestSummary, ContextUsage } from "@zeron/proto";
import { useEngineSession } from "../state/session-provider";
import { engineRegistry, engineStatesOf, useFleetRegistry, useFleetSnapshot } from "../state/fleet";
import { useNow, useWatchSnapshot } from "../state/hooks";
import { useTitlebar } from "../state/chrome";
import { emitShortcut } from "../state/shortcuts";
import { chatPageRow, type ChatRow } from "../lib/view";
import { JumpPill, StatusStrip, TranscriptView, type JumpButtonState } from "../components/transcript";
import { ChatTranscriptOutlet } from "../components/chat-transcript-outlet";
import type { SubagentOpen } from "../components/tool-group";
import { resolvePaneWidth, rightPaneStore, useRightPane } from "../state/right-pane";
import { Composer } from "../components/composer";
import { QueuePanel } from "../components/queue-panel";
import { ComposerFooter } from "../components/composer-footer";
import { useNewThreadTarget } from "../components/composer/new-thread-selectors";
import { NewThreadCanvas } from "./index-page";
import {
  bottomClearance,
  conversationWidth,
  sidebarTarget,
  useSidebarLayout,
  useViewportWidth,
} from "../state/layout";
import {
  SIDEBAR_GLIDE_MS,
  SIDEBAR_SETTLE_CAP_MS,
  sidebarTweenSignal,
  type SidebarTweenSignal,
} from "../lib/sidebar-tween";
import {
  DockMountSequencer,
  clearDockGlideVars,
  dockGlideChannels,
  dockGlideSignal,
  writeDockGlideVars,
} from "../lib/dock-glide";
import type { SurfaceTreatment } from "@zeron/theme";
import { useIsPhone } from "../state/media";
import { navEntryForPath } from "../state/nav-history";
import {
  bottomStackMeasurementMatches,
  dockFrameEquals,
  dockFrameSettled,
  DockState,
  heroLayerMounted,
  type DockFrame,
} from "../lib/composer-dock";
import { COMPOSER_MAX_WIDTH } from "../lib/composer-flip";
import { ChangeRequestStore, type ChangeRequestTarget, changeRequestForChat } from "../state/change-requests-store";
import { QueueStore } from "../state/queue-store";
import { QueueStoreProvider } from "../state/queue-store-context";
import { sidebarNotice } from "../state/notice";
import { markChatSeen } from "../lib/chat-actions";
import { availableQueuePrimaryAction } from "../lib/queue-row-logic";
import { ATTACHMENT_ONLY_TEXT, uploadAttachments, type StagedAttachment } from "../lib/attachments";
import { TerminalDock } from "../terminal/terminal-dock";
import { drawerTerminalStore } from "../terminal/store";
import type { MarkdownSurface } from "../components/markdown";
import { echoStore, TranscriptStore, chatDeliveryDegraded, type TranscriptCache } from "../state/transcript-store";

/**
 * Ticket 81 — WHEN each offline transcript save happened, per
 * `(engineKey, rawChatId)` key, in one small localStorage JSON map. The
 * registry cache itself stays entries-in/entries-out (engine-client shape),
 * so the stamp lives here: `save` records the wall clock alongside the
 * durable write, `load` reads it back. A seconds-old stamp marks a LIVE
 * mid-run snapshot (seeded verbatim — the tail group renders open
 * immediately, like the desktop's state-preserved switch); anything older
 * or absent is a dead session's leftover (downgraded at seed time, ticket
 * 80). Absent or unwritable storage reads as unstamped (0 ⇒ stale).
 */
const TRANSCRIPT_SEED_STAMPS_KEY = "zeron.transcriptSeedStamps.v1";

function readSeedStamps(): Record<string, number> {
  try {
    const raw = window.localStorage.getItem(TRANSCRIPT_SEED_STAMPS_KEY);
    if (raw === null) {
      return {};
    }
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      return {};
    }
    return parsed as Record<string, number>;
  } catch {
    return {};
  }
}

function writeSeedStamp(key: string, savedAtMs: number): void {
  try {
    const stamps = readSeedStamps();
    stamps[key] = savedAtMs;
    window.localStorage.setItem(TRANSCRIPT_SEED_STAMPS_KEY, JSON.stringify(stamps));
  } catch {
    // Unwritable storage: the next load reads the seed as unstamped (stale).
  }
}

/**
 * The chat transcript's offline cache handle: `(engineKey, rawChatId)`
 * resolved off the scoped URL id, backed by the fleet registry's cache.
 */
function transcriptCacheFor(engineKey: string, scopedChatId: string): TranscriptCache | undefined {
  try {
    const raw = parseScopedId(scopedChatId).rawId;
    const cache = engineRegistry.cache;
    const stampKey = `${engineKey}:${raw}`;
    return {
      load: async () => {
        const entries = await cache.loadTranscript(engineKey, raw);
        if (entries === null) {
          return null;
        }
        const stamp = readSeedStamps()[stampKey];
        return { entries, savedAtMs: typeof stamp === "number" ? stamp : 0 };
      },
      save: async (entries) => {
        await cache.saveTranscript(engineKey, raw, entries);
        writeSeedStamp(stampKey, Date.now());
      },
    };
  } catch {
    return undefined;
  }
}

/**
 * The column width's publication policy (ticket 64 §2.4). The column's
 * ResizeObserver fires per geometry tick — per animation FRAME while the
 * sidebar's 200ms CSS width transition runs — and only a fraction of those
 * ticks are React news. Every tick updates the live channels outside React
 * (the mutable measured width below, plus the clamped composer width target
 * the dock pump reads per frame), while the published `columnWidth` state
 * defers exactly one window: the blank canvas under an active sidebar tween
 * (those frames are the shell's own motion, not semantic change). The
 * signal's settle edge — `transitionend`, the settle cap, the drag/reduce
 * disarms; they all funnel through `settle()` — publishes the current final
 * measurement once. Selected-chat publication stays responsive: QueuePanel
 * reads `columnWidth` too, and its layout is not part of this canvas-scoped
 * deferral.
 */
export interface ColumnWidthPublicationOptions {
  /** The shared sidebar tween signal — the ONLY defer window (no second clock). */
  readonly signal: SidebarTweenSignal;
  /** A chat is selected: publication stays responsive (the defer is canvas-scoped). */
  readonly hasSelection: boolean;
  /** The clamped live composer target, written on EVERY tick (the dock pump's per-frame read). */
  readonly liveTarget: { current: number };
  /** The React publication (`setColumnWidth`). */
  readonly publish: (width: number) => void;
}

export class ColumnWidthPublication {
  readonly #signal: SidebarTweenSignal;
  readonly #hasSelection: boolean;
  readonly #liveTarget: { current: number };
  readonly #publish: (width: number) => void;
  readonly #unsubscribe: () => void;
  // §2.4's mutable measured-width ref: every observer tick lands here first.
  #measured: number | null = null;
  #published: number | null = null;
  #deferred = false;

  constructor(options: ColumnWidthPublicationOptions) {
    this.#signal = options.signal;
    this.#hasSelection = options.hasSelection;
    this.#liveTarget = options.liveTarget;
    this.#publish = options.publish;
    this.#unsubscribe = options.signal.subscribe((active) => {
      if (!active) {
        this.#flush();
      }
    });
  }

  /**
   * One column geometry tick: the live channels update unconditionally (the
   * measured width, the clamped composer target — no React commit), then
   * the publication policy decides whether React hears about the width.
   */
  note(width: number): void {
    this.#measured = width;
    this.#liveTarget.current = Math.min(Math.max(width, 0), COMPOSER_MAX_WIDTH);
    if (width === this.#published) {
      // Unchanged (the observer's initial duplicate, a settled re-tick):
      // never a publication — and a deferred window whose measurement has
      // returned to the published width has nothing left to flush.
      this.#deferred = false;
      return;
    }
    if (!this.#hasSelection && this.#signal.isActive()) {
      this.#deferred = true;
      return;
    }
    this.#publishNow(width);
  }

  /** The settle edge: the current final measurement, published once. */
  #flush(): void {
    const measured = this.#measured;
    this.#deferred = false;
    if (measured === null || measured === this.#published) {
      return;
    }
    this.#publishNow(measured);
  }

  #publishNow(width: number): void {
    this.#published = width;
    this.#deferred = false;
    this.#publish(width);
  }

  dispose(): void {
    this.#unsubscribe();
  }
}

/**
 * The conversation page — the desktop's `render_main` (shell.rs:5806-6154).
 *
 * BOTH conversation routes render THIS component (`/` and `/chat/$chatId`,
 * see `router.tsx`): TanStack's `Match` memoizes the component element on
 * `route.options.component`, so the same `ConversationPage` reference on
 * both routes keeps ONE fiber alive across the route boundary — the web
 * peer of the desktop's "one composer entity, re-anchored in prepaint,
 * never remounted". The caret, the selection, the draft and the popup state
 * all travel with the pixels. The page re-renders on navigation through its
 * own router-state subscription; the chat id comes from the pathname
 * (`""` = the new-thread canvas, the boot route).
 *
 * The blank canvas is the hero + the SAME persistent composer vertically
 * re-anchored: the composer's wrapper sits in the bottom chrome stack (its
 * layout slot), and the dock (`lib/composer-dock.ts`, the port of
 * `composer_dock.rs`) glides it to `(viewportHeight − height)·0.5 + 8` via a
 * transform, anchoring by the TOP of the surface. One retargetable clock
 * owns everything: the glide (0.420/470 s critically damped), the pill
 * height (`dockHeight`), the four staged chrome channels, the hero's
 * `dissolve`, the 0.320 s panel handoff, and the width glide that snaps
 * inside the handoff's invisible interval. Reduced motion snaps everything.
 * The phone layer (≤ 768px) mounts the same canvas — hero, selectors, dock
 * re-anchoring — per ticket 53's amendment to spec decision 5.
 */
export function ConversationPage() {
  // `chatId === ""` is the new-thread canvas; anything else names a chat.
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const navEntry = navEntryForPath(pathname);
  const chatId = navEntry !== null && navEntry.kind === "chat" ? navEntry.chatId : "";
  const hasSelection = chatId !== "";
  const session = useEngineSession();
  // The MERGED fleet snapshot: the chat row lookup by its scoped URL id
  // spans every engine; `session` above is the chat's owning engine (the
  // routed session), so calls and watches (transcript/queue/change
  // requests) already target the right connection.
  const snapshot = useFleetSnapshot();
  const registry = useFleetRegistry();
  const status = session === null ? null : session.client.status;
  const now = useNow(10_000);
  const navigate = useNavigate();

  // The routed engine's live `WatchConnectivity` posture (ticket 30's
  // per-engine slot in the session's watch cache) — `chat_delivery_degraded`'s
  // web arm: while the chat's delivery path is degraded, the transcript's
  // pending-send overlay holds "pending" (Queued), never a false "Not
  // delivered" past the 120s grace. No new stream: the page re-renders on
  // this engine's watch changes already (the fleet registry).
  const sessionWatch = useWatchSnapshot(session);
  const deliveryDegraded = chatDeliveryDegraded(sessionWatch?.connectivity.value?.state);

  // Occupancy arrives on the transcript's watch; the composer footer draws it.
  const [contextUsage, setContextUsage] = useState<ContextUsage | null>(null);
  useEffect(() => {
    setContextUsage(null);
  }, [chatId]);

  // Lazily fetch the harness catalog once per chat page open so the
  // composer chips aren't blank behind a stale "Loading." pill.
  useEffect(() => {
    if (session === null) {
      return;
    }
    void session.catalog.loadHarnesses();
  }, [session, chatId]);

  const deviceId = status?.state === "connected" ? status.info.deviceId : null;
  const chat = !snapshot.chats.loaded
    ? undefined
    : snapshot.chats.rows.find((row) => row.id === chatId) ?? undefined;
  const branch = chat?.branch ?? null;
  const checkoutId = chat?.checkoutId ?? null;
  const cwd = chat?.cwd ?? null;

  // ── The canvas target + the stub chat (the new-thread route) ───────────
  // The canvas has no chat row; the composer still needs a `Chat`-shaped
  // target — the remembered device/project picks resolved through
  // `effective_device_id` (state.rs:1314-1320). The stub's id is the DRAFT
  // key `""` (the desktop's `current_key` for the new-thread canvas), so
  // the canvas draft survives every round trip.
  const target = useNewThreadTarget();
  const stubChat = useMemo<Chat>(
    () => ({
      id: chatId,
      deviceId: target.effectiveDeviceId ?? "",
      title: null,
      archived: false,
      cwd: target.space?.path ?? null,
      branch: null,
      checkoutId: null,
      config: null,
      lastMessagePreview: null,
      lastMessageAt: null,
      createdAt: new Date(0).toISOString(),
      spaceId: target.space?.id ?? null,
    }),
    [chatId, target.effectiveDeviceId, target.space?.id, target.space?.path],
  );
  // While a freshly minted chat's row is still landing, the stub stands in
  // (same id, so the composer never re-swaps its draft).
  const effectiveChat = chat ?? stubChat;

  // The markdown host hooks (transcript.rs:5341-5349 `workspace_root` +
  // `LinkOutcome::Internal`): the chat's cwd resolves agent-authored file
  // links, and an internal click opens the file's right-pane tab.
  const markdownSurface = useMemo<MarkdownSurface>(
    () => ({
      workspaceRoot: cwd,
      openWorkspaceFile: (path) => {
        rightPaneStore.addFileSurface(chatId, path);
        if (!rightPaneStore.stateFor(chatId).open) {
          rightPaneStore.toggle(chatId);
        }
      },
    }),
    [chatId, cwd],
  );

  // The chat's ONE transcript store: the transcript view and the composer's
  // question wizard both read it, so an open chat carries a single
  // `WatchDocMessages` stream. The session keeps a bounded set of recent
  // stores live, so revisiting a chat has current rows on its first render.
  const transcriptStore = useMemo(() => {
    if (session === null || chatId === "") {
      return null;
    }
    return session.transcripts.get(chatId, transcriptCacheFor(session.engine.baseUrl, chatId));
  }, [session, chatId]);
  // The LIVE store: the departing transcript (undocking back to the canvas)
  // keeps painting from the source chat's stream until the route finishes
  // its exit (`finish_route_exit`, shell.rs:5901-5904). The session pool
  // owns disposal; switching routes only releases this presentation ref.
  const storeRef = useRef<TranscriptStore | null>(null);
  if (transcriptStore !== null) {
    storeRef.current = transcriptStore;
  }
  // A spawn chip's "Open subagent" registers the right-pane tab under this
  // chat (`add_subagent_surface`, shell.rs:2682) — the pane opens on it.
  const onOpenSubagent = useCallback(
    (payload: SubagentOpen) => {
      rightPaneStore.addSubagentSurface(chatId, payload);
    },
    [chatId],
  );

  const crStore = useMemo(() => {
    if (session === null) {
      return null;
    }
    return new ChangeRequestStore(session.client);
  }, [session]);

  useEffect(() => () => {
    crStore?.dispose();
  }, [crStore]);

  useEffect(() => {
    if (crStore === null || deviceId === null || cwd === null || branch === null) {
      return;
    }
    const trimmed = branch.trim();
    if (trimmed.length === 0) {
      crStore.setTargets([]);
      return;
    }
    const targets: ChangeRequestTarget[] = [{ deviceId, cwd, branch: trimmed, checkoutId }];
    crStore.setTargets(targets);
  }, [crStore, deviceId, cwd, branch, checkoutId]);

  const crSummary: ChangeRequestSummary | null = useMemo(() => {
    if (crStore === null || deviceId === null || cwd === null || branch === null) {
      return null;
    }
    const snap = crStore.getSnapshot();
    return changeRequestForChat(snap.snapshots, { deviceId, cwd, branch: branch.trim(), checkoutId });
  }, [crStore, deviceId, cwd, branch, checkoutId]);

  // Queue store: one per chat. Disposed on chat switch so a fresh
  // subscription lands immediately.
  const queueStore = useMemo(() => {
    if (session === null || deviceId === null || chatId === "") {
      return null;
    }
    return new QueueStore(session.client, chatId, { editorDeviceId: deviceId });
  }, [session, chatId, deviceId]);

  useEffect(() => () => {
    queueStore?.dispose();
  }, [queueStore]);

  // Edit state: the chat page owns which queued row (if any) is feeding
  // text into the composer. Clearing it cancels the lease (the user
  // backed out without saving); committing finishes the lease with the
  // current composer text in place. `editFinishing` is the desktop's
  // `queue_edit_finishing` — true while the lease-closing RPC is in flight
  // (the row shows "Saving…" and its Save/Cancel stand down).
  const [editingRow, setEditingRow] = useState<{
    id: string;
    text: string;
    attachments: readonly string[];
  } | null>(null);
  const [editFinishing, setEditFinishing] = useState(false);

  // The composer's `commit_queue_edit` handle (queue.rs:1430) — assigned by
  // the Composer while a queued-row edit is open, so the queue row's inline
  // Save and the composer's submit share the one commit path.
  const editCommitRef = useRef<(() => void) | null>(null);

  // If the queue store reports the row disappeared while we were editing
  // (another device removed/sent it), drop the edit state so the composer
  // doesn't carry stale text.
  useEffect(() => {
    if (editingRow === null || queueStore === null) {
      return;
    }
    if (queueStore.rowById(editingRow.id) === null) {
      setEditingRow(null);
    }
  }, [editingRow, queueStore, queueStore?.getSnapshot().generation]);

  // ── The 20s edit-lease heartbeat (`start_queue_edit_renewal`,
  // queue.rs:1584-1633) ── renewed for as long as the edit is open; a
  // "lost"/"missing" outcome clears the local edit exactly like the
  // desktop's expiry path (the row then carries a ReviewRequired gate).
  // A transient RPC failure is tolerated — the 60s lease fails closed on
  // the host if every attempt misses.
  useEffect(() => {
    if (editingRow === null || queueStore === null) {
      return;
    }
    const timer = setInterval(() => {
      void queueStore
        .renewEdit()
        .then((result) => {
          if (result.kind === "lost" || result.kind === "missing") {
            setEditingRow(null);
            sidebarNotice.set("Edit protection expired; review this message before sending");
          }
        })
        .catch(() => {});
    }, 20_000);
    return () => clearInterval(timer);
  }, [editingRow, queueStore]);

  const onEditRow = useCallback((row: QueuedMessage) => {
    setEditingRow({ id: row.id, text: row.text, attachments: row.attachments ?? [] });
  }, []);

  const onEditFinish = useCallback(
    (outcome: {
      action: "commit" | "cancel" | "discard" | "releaseUnchanged";
      text: string;
      staged?: readonly StagedAttachment[];
    }) => {
      if (queueStore === null || editingRow === null || session === null) {
        return;
      }
      const store = queueStore;
      const rowId = editingRow.id;
      setEditFinishing(true);
      void (async () => {
        // Non-terminal outcomes keep the edit open with the user's text in
        // the editor (`finish_queue_edit`'s failure arms, queue.rs:1550-1575).
        let keepRow = false;
        try {
          const lease = store.getSnapshot().editLease;
          if (lease === null || lease.messageId !== rowId) {
            return;
          }
          if (outcome.action === "commit") {
            // `finish_queue_edit("commit")`: the staged set uploads first
            // (queue.rs:1522-1539) — the row's own attachments were staged
            // into the composer at edit start, newly added ones are new
            // uploads — then the commit carries text + paths.
            const staged = outcome.staged ?? [];
            const uploaded =
              staged.length > 0
                ? await uploadAttachments(session.client, staged, null)
                : ([] as readonly { path: string }[]);
            const body =
              outcome.text.trim().length > 0 ? outcome.text : ATTACHMENT_ONLY_TEXT;
            const result = await store.finishEdit("commit", {
              text: body,
              attachments: uploaded.map((entry) => entry.path),
            });
            if (result.kind === "conflict") {
              sidebarNotice.set("This message changed on another device; your edit was kept locally");
              keepRow = true;
            } else if (result.kind === "missing") {
              sidebarNotice.set("The queued message was removed; your edit was kept locally");
              keepRow = true;
            } else if (result.kind === "lost") {
              sidebarNotice.set("The edit lease changed; your text is still in the editor");
              keepRow = true;
            }
          } else {
            const result = await store.finishEdit(outcome.action);
            if (result.kind === "conflict" || result.kind === "missing" || result.kind === "lost") {
              sidebarNotice.set("The edit lease changed; your text is still in the editor");
              keepRow = true;
            }
          }
        } catch (error) {
          sidebarNotice.set("Couldn't reach the chat host; your edit is still in the editor");
          keepRow = true;
        } finally {
          setEditFinishing(false);
          if (!keepRow) {
            setEditingRow(null);
          }
        }
      })();
    },
    [queueStore, editingRow, session],
  );

  const onEditCancel = useCallback(() => {
    if (queueStore === null || editingRow === null) {
      return;
    }
    const store = queueStore;
    const rowId = editingRow.id;
    setEditingRow(null);
    setEditFinishing(true);
    void (async () => {
      try {
        const lease = store.getSnapshot().editLease;
        if (lease !== null && lease.messageId === rowId) {
          await store.finishEdit("cancel");
        }
      } catch (error) {
        sidebarNotice.set(`Could not cancel edit: ${error instanceof Error ? error.message : String(error)}`);
      } finally {
        setEditFinishing(false);
      }
    })();
  }, [queueStore, editingRow]);

  const row =
    !snapshot.chats.loaded
      ? undefined
      : chatPageRow(
          chatId,
          snapshot.chats.rows,
          snapshot.spaces.rows,
          snapshot.statuses.rows,
          now,
          snapshot.devices.rows,
          engineStatesOf(registry),
          // The same loaded-only dangling gate as the sidebar (ticket 43).
          snapshot.spaces.loaded,
        );

  // ── The dock: one retargetable clock for the route choreography ────────
  const viewport = useViewportWidth();
  const viewportHeight = useViewportHeight();
  const sidebar = useSidebarLayout();
  // The shared media hook (ticket 49): `(max-width: 768px)` resolved through
  // matchMedia — the same query the stylesheet keys, so JS and CSS flip in
  // the same paint (the old `viewport <= PHONE_MAX_WIDTH` innerWidth compare could
  // disagree with the media query by rounding).
  const phone = useIsPhone();
  // The phone sidebar is a fixed overlay out of flow (the same M3/M2
  // correction `app-shell.tsx` applies to the titlebar's geometry), so the
  // hero's sidebar term reads 0 at ≤768 — `heroWidth` is the full phone
  // canvas width, never `viewport − dragged` (375/304 would paint a 71px
  // hero).
  const sidebarNow = phone ? 0 : sidebarTarget(sidebar);
  const [reducedMotion, setReducedMotion] = useState(prefersReducedMotion);
  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReducedMotion(query.matches);
    onChange();
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);
  // Ticket 53's recorded choice (§2.2, option 1): the dock re-anchors at
  // phone exactly as at ≥769 — the glide, the chrome channels and the
  // dissolve all run, anchored at `(vh − h)·0.5 + 8`; reduced motion still
  // snaps everything through `reducedMotion`.
  const dockReduced = reducedMotion;

  // ── The sidebar slide (tickets 34 → 57a) ─────────────────────────────
  // `toggle_sidebar` (shell.rs:1947-1957) flips the column target; the
  // desktop's `sidebar_now()` (3797-3801) evaluates the 200ms tween IN
  // RENDER (one scalar, GPU-side). The web's peer is now a CSS `width`
  // transition the BROWSER interpolates: `.new-thread-hero[data-sidebar-
  // tween="1"]` (app.css) rides the exact `--rb-motion-resize` spec the
  // column glides on, so the hero and the column share one frame clock with
  // ZERO React frames — no rAF loop, no synchronous re-render per frame
  // (ticket 57a replaces the ticket-34 pump; the visible targets — the
  // 200ms slide, the easeOut curve, the reduced-motion snap — are
  // unchanged). The cutout hole tracks
  // the pill through the raster-window CSS (the readiness layer holds the
  // pre-flip bitmap centered on the gliding hero, `app.css`), and the hero
  // re-rasters ONCE, at settle. A mid-glide reversal needs no capture: a CSS
  // transition retargets from the painted width, which IS the desktop's
  // "restarts from what is painted" semantics.
  const previousSidebarRef = useRef(sidebar);
  // True while the glide runs: flips the hero's `data-sidebar-tween` flag
  // (its width transition + the raster window) from the flip's follow-up
  // commit to the settle commit. Two React commits per toggle, none per
  // frame.
  const [sidebarTweening, setSidebarTweening] = useState(false);
  // The FLIP render's width hold: the flip commit re-renders with the NEW
  // `sidebarNow` before the layout effect below can raise the flag, so the
  // flip render pins the hero's width to the PREVIOUS target — the flip
  // commit is a visual no-op, and the ONE commit that follows (flag on +
  // new width) is the only width change the browser recalcs, which is what
  // arms the transition (a forced layout read inside the flip commit's own
  // layout effects would otherwise recalc the change with the flag still
  // off, and the hero would snap). Mid-glide reversals skip the hold: the
  // running transition retargets from the painted width, so committing the
  // new target directly is correct.
  const sidebarFlip = previousSidebarRef.current.collapsed !== sidebar.collapsed;
  const sidebarTweenArming =
    sidebarFlip &&
    !reducedMotion &&
    !phone &&
    sidebarTarget(previousSidebarRef.current) !== sidebarTarget(sidebar);
  const heroSidebarTerm = sidebarTweenArming && !sidebarTweening
    ? sidebarTarget(previousSidebarRef.current)
    : sidebarNow;

  // Arm on the flip. A LAYOUT effect so the flag's commit lands before
  // paint (the hold above made the flip commit itself a no-op).
  useLayoutEffect(() => {
    const previous = previousSidebarRef.current;
    previousSidebarRef.current = sidebar;
    if (previous.collapsed === sidebar.collapsed) {
      // Not a flip: a seam drag took the clock over mid-tween (the column
      // tracks the pointer exactly under `data-rb-resizing`) — drop the
      // flag so the hero's width transition dies with the column's and the
      // hero follows the drag; NewThreadBackground's settle path snaps the
      // raster to the drag geometry in the same commit.
      if (sidebarTweening) {
        sidebarTweenSignal.settle();
        setSidebarTweening(false);
      }
      return;
    }
    if (reducedMotion || phone || sidebarTarget(previous) === sidebarTarget(sidebar)) {
      // The reduce arm: the endpoint, directly — the reduce block in
      // app.css kills the transition, and the settled formula above IS the
      // endpoint. At phone the sidebar is out of flow, so there is no
      // painted column width to tween — the hero's sidebar term is pinned
      // to 0 by the `sidebarNow` phone arm.
      if (sidebarTweening) {
        sidebarTweenSignal.settle();
        setSidebarTweening(false);
      }
      return;
    }
    // A qualifying flip — or a mid-glide reversal (the retarget restarts
    // from the painted width on its own). The settle listeners below own
    // the fall.
    sidebarTweenSignal.arm();
    setSidebarTweening(true);
    // `sidebarTweening`/`reducedMotion`/`phone` are read from this render's
    // closure on purpose: only a flip re-arms, never their own changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sidebar]);

  // The settle: `transitionend` on the hero's width transition is the glide
  // falling — ONE commit (flag off), and NewThreadBackground's layout effect
  // re-rasters at the settled geometry inside it, before paint. The hero
  // element is NewThreadBackground's (reached below via NewThreadCanvas);
  // the settle cap covers the swallowed cases (the hero unmounts mid-glide,
  // the tab hides the transition, a kill cancels without a successor), and
  // a cancel whose width already matches the inline target settles at once
  // (that is the reduce/resize snap, not a retarget).
  useEffect(() => {
    if (!sidebarTweening) {
      return;
    }
    const settle = (): void => {
      sidebarTweenSignal.settle();
      setSidebarTweening(false);
    };
    const hero = document.querySelector<HTMLElement>(".new-thread-hero");
    const inlineWidth = hero === null ? null : parseFloat(hero.style.width);
    const isHeroWidthEvent = (event: TransitionEvent): boolean =>
      event.propertyName === "width" && event.target === hero;
    const onEnd = (event: TransitionEvent): void => {
      if (isHeroWidthEvent(event)) {
        settle();
      }
    };
    const onCancel = (event: TransitionEvent): void => {
      if (!isHeroWidthEvent(event) || hero === null) {
        return;
      }
      // A retarget fires cancel + restart (the width is mid-flight, away
      // from the inline target — stay armed for the successor); a kill
      // (reduce flip, `data-rb-resizing`) snaps the width TO the inline
      // target — settle now so the settle remask lands with the snap.
      if (inlineWidth !== null && Math.abs(hero.offsetWidth - inlineWidth) < 0.5) {
        settle();
      }
    };
    const cap = window.setTimeout(settle, SIDEBAR_GLIDE_MS + SIDEBAR_SETTLE_CAP_MS);
    hero?.addEventListener("transitionend", onEnd);
    hero?.addEventListener("transitioncancel", onCancel);
    return () => {
      window.clearTimeout(cap);
      hero?.removeEventListener("transitionend", onEnd);
      hero?.removeEventListener("transitioncancel", onCancel);
    };
    // `sidebar` re-runs on a mid-glide reversal so the cap restarts and
    // still bounds the NEW glide, not the old one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sidebarTweening, sidebar]);

  const dockRef = useRef(new DockState());
  const [dockFrame, setDockFrameState] = useState<DockFrame>(() => dockFrameSettled(false));
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const prepaintMovingRef = useRef(false);
  const dockTickedRef = useRef(false);
  // The de-Reacted pump's channels (ticket 57b, §2.3): the live frame the
  // composer's evaluate pass reads per frame (nulled at render — the
  // `dockFrame` prop is authoritative between glides; the pump writes it
  // inside its rAF callback, before the evaluate it invokes), and the
  // composer's evaluate pass itself, parked here so the loop can re-run it
  // per animation frame with zero React state.
  const liveDockFrame = useRef<DockFrame | null>(null);
  liveDockFrame.current = null;
  const dockEvaluateRef = useRef<(() => void) | null>(null);
  // The pump's per-frame inputs, read at rAF time through refs — renders
  // keep them current (the loop never re-arms per frame, so it cannot read
  // them from a stale closure).
  const viewportHeightRef = useRef(viewportHeight);
  viewportHeightRef.current = viewportHeight;
  const dockReducedRef = useRef(dockReduced);
  dockReducedRef.current = dockReduced;

  // ── The pane's synchronous width — the handoff's arming input ───────────
  // `right_now` (shell.rs:3820-3826): the pane's width computed from STATE,
  // never measured — `eval_tween(right_tween, right_target)` is the TARGET
  // at a navigation commit (the chat switch clears the tweens, shell.rs
  // 1837-1862, "snap, no tween — the panels belong to the destination
  // chat"), plus the seam's edge bounce while the pane sits at a drag
  // bound. The pane store subscription matters even though the handoff
  // only arms on a docked flip: `previous` must carry the pane width that
  // was painted while docked, or the undock arm below would read width
  // 0→0 and never fire. The OLD input — the ResizeObserver-measured
  // `columnWidth` — was one commit stale by construction: on the flip
  // commit it still held the canvas width, so the docked flip arrived
  // WITHOUT its width change and the next (fresh) sample saw the width
  // change WITHOUT the flip — the handoff never armed (§2.1).
  const paneState = useRightPane(chatId);
  const paneOpen = hasSelection && paneState.open;
  const paneNowWidth = paneOpen
    ? resolvePaneWidth(paneState, viewport, phone ? 0 : sidebarNow) +
      (paneState.expanded ? 0 : readPaneEdgeBounceOffset())
    : 0;
  const paneNowWidthRef = useRef(paneNowWidth);
  paneNowWidthRef.current = paneNowWidth;

  // ── observePane → transcriptWidth → tick — the desktop's paint order ────
  // KNOWN LIMITATION (sanctioned per tickets 35/36): this render body mutates
  // `dockRef.current` (observePane/transcriptWidth/tick) — React-concurrent-
  // unsafe if this pass were discarded. The tick is guarded to be idempotent
  // (`frame.docked !== hasSelection`), and no concurrent features are enabled,
  // so this is safe under current non-concurrent usage.
  // The shell samples the pane BEFORE `render_main`'s dock tick (observe_pane
  // at shell.rs:7901-7906, transcript_width at :7915-7919, the tick at
  // :5882-5885 inside render_main): the tick reads `pane.progress` to arm
  // the panel_return/panel_departure clocks, so the sample must land first —
  // the ordering ticket 35's merger note flagged (its render-phase tick
  // preceded the layout effect's observePane, leaving #panelDeparture
  // unreachable). One timestamp for all three, the desktop's `frame_time`.
  const frameNowMs = performance.now();
  const paneHandoffLive = dockRef.current.observePane(hasSelection, paneNowWidth, !dockReduced, frameNowMs);
  // `transcript_width` (shell.rs:7915-7919): the retained value feeds the
  // transcript wrapper's width. Called BEFORE the tick so the capture
  // condition (`!docked && frame.docked`) still sees the pre-flip frame —
  // what gets retained is the SOURCE column's width. The target is the
  // conversation's stable content width (`conversation_width(viewport,
  // sidebar_target, right_target_width)`, shell.rs:7910-7914 — the
  // takeover-stable leg never co-occurs with a route flip, which clears
  // the tween).
  const mainContentWidth = conversationWidth(viewport, sidebarTarget(sidebar), paneNowWidth);
  const retainedTranscriptWidth = dockRef.current.transcriptWidth(
    mainContentWidth,
    hasSelection,
    paneHandoffLive,
  );
  const mainContentWidthRef = useRef(mainContentWidth);
  mainContentWidthRef.current = mainContentWidth;

  // ── The same-render dock tick (ticket 35, shell.rs:5882-5885) ────────────
  // The desktop decides the hero layer's mount from the frame ticked in the
  // SAME render — `tick` precedes the layer at shell.rs:5883. The web's
  // route change used to land one render BEFORE the per-commit layout
  // effect's tick below, so the navigation render read the previous
  // render's settled frame: `heroVisible` went false, the hero unmounted
  // for that commit (its artwork state with it), and the tick's re-render
  // remounted it — the route-change double flash. The route-change tick
  // now runs HERE, in the render body, and publishes through a render-phase
  // update (the sanctioned derive-state-during-render shape: React discards
  // this pass and re-renders immediately, so no commit ever paints with the
  // hero unmounted). The mount decision below reads the freshly ticked
  // MUTABLE frame; `dockFrame` state keeps driving the visuals.
  if (dockRef.current.frame.docked !== hasSelection) {
    const ticked = dockRef.current.tick(hasSelection, dockReduced, frameNowMs);
    setDockFrameState((prev) => (dockFrameEquals(prev, ticked) ? prev : ticked));
  }

  // ── Bottom chrome stack bookkeeping ─────────────────────────────────────
  // `bottom_stack` measured live (the desktop's paint-time canvas) PLUS
  // `dock_clearance_correction` — the shell reserves the DESTINATION
  // footprint, never the animated height, so the transcript's bottom fade
  // band and clearance pad never pump mid-route. The same measurement
  // records `bottom_stack_has_composer` for `transcript_geometry_ready`
  // (shell.rs:837): one frame of disagreement hides the transcript so it
  // never flashes under unmeasured chrome.
  const chatColumnRef = useRef<HTMLDivElement | null>(null);
  const bottomStackRef = useRef<HTMLDivElement | null>(null);
  const [columnWidth, setColumnWidth] = useState<number | null>(null);
  // Ticket 64 §2.4: the LIVE clamped composer width target — the column
  // observer feeds it on EVERY tick through `ColumnWidthPublication` (the
  // dock pump reads it per frame), and render must not overwrite it with
  // the published `columnWidth`, which is stale through the defer window.
  // The published value only seeds the pre-measurement initial (null → cap).
  const composerWidthTargetRef = useRef<number>(COMPOSER_MAX_WIDTH);
  const dockCorrectionRef = useRef(0);
  const [measuredHasComposer, setMeasuredHasComposer] = useState(false);
  const expectedHasComposer = session !== null && hasSelection;
  const transcriptGeometryReady = bottomStackMeasurementMatches(measuredHasComposer, expectedHasComposer);

  useEffect(() => {
    const column = chatColumnRef.current;
    if (column === null || typeof ResizeObserver === "undefined") {
      return;
    }
    // Ticket 64 §2.4: every tick updates the live channels (the publisher's
    // measured width and the clamped `composerWidthTargetRef` the dock pump
    // reads per frame) WITHOUT a React commit; the React publication defers
    // only for the blank canvas while the sidebar tween signal is active
    // and re-publishes the current final measurement on its settle edge.
    const publication = new ColumnWidthPublication({
      signal: sidebarTweenSignal,
      hasSelection,
      liveTarget: composerWidthTargetRef,
      publish: (width) => setColumnWidth(width),
    });
    const observer = new ResizeObserver(() => {
      publication.note(column.getBoundingClientRect().width);
    });
    observer.observe(column);
    publication.note(column.getBoundingClientRect().width);
    return () => {
      observer.disconnect();
      publication.dispose();
    };
    // The early returns above gate the refs; re-arm once the tree lands.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatId, row?.chat.id, session]);

  // The dock's per-commit pass: re-anchor the composer's wrapper, publish
  // the stack measurement. Route changes tick in the render body (the
  // same-render tick above, ticket 35 — after the render-body `observePane`
  // sample, the desktop's paint order); the pane sample runs there too, per
  // render, so the handoff's clock advances on the pump's re-renders. The
  // measured `columnWidth` here feeds only the composer's width target.
  // Steady-state frames advance ONLY on the pump's animation frames —
  // ticking per commit would spin the layout-effect/setState pair
  // synchronously and freeze the glide. A layout effect so the transform
  // lands in the same commit as the frame — the web peer of
  // prepaint-before-paint.
  useLayoutEffect(() => {
    const dock = dockRef.current;
    const nowMs = performance.now();
    // The clock initializes on the first pass (the desktop's first tick
    // snaps the settled hero state and stamps `last_frame`); route changes
    // are already ticked by the render above, and the docked check stays as
    // the safety net. Steady frames advance on the pump.
    if (dock.frame.docked !== hasSelection || !dockTickedRef.current) {
      dockTickedRef.current = true;
      const next = dock.tick(hasSelection, dockReduced, nowMs);
      setDockFrameState((prev) => (dockFrameEquals(prev, next) ? prev : next));
    }

    const wrapper = wrapperRef.current;
    const stack = bottomStackRef.current;
    if (wrapper !== null && stack !== null) {
      const stackRect = stack.getBoundingClientRect();
      // The wrapper's NATURAL slot (transform excluded — offsets are layout
      // values): the desktop's prepaint reads the layout bounds.
      const bounds = {
        left: stackRect.left + wrapper.offsetLeft,
        top: stackRect.top + wrapper.offsetTop,
        height: wrapper.offsetHeight,
      };
      const { dx, dy, moving } = dock.prepaint(bounds, viewportHeight, dockReduced, nowMs);
      wrapper.style.transform = `translate(${dx}px, ${dy}px)`;
      prepaintMovingRef.current = moving;
    } else if (wrapper !== null) {
      wrapper.style.transform = "";
      prepaintMovingRef.current = false;
    }

    const column = chatColumnRef.current;
    if (stack !== null && column !== null) {
      const height = stack.getBoundingClientRect().height + dockCorrectionRef.current;
      column.style.setProperty("--rb-bottom-stack", `${height}px`);
      bottomClearance.set(height);
      setMeasuredHasComposer(session !== null && hasSelection);
    }
  });

  // The composer's own width: glides on the dock's clock (snapping inside
  // the handoff's invisible interval), clamped to the 768px cap — the
  // desktop's `layout_width(main_content_width.min(COMPOSER_MAX_WIDTH))`,
  // fed back through `set_available_width` so the pill re-wraps mid-glide.
  // `layoutWidth` with a ~0 dt is a no-op, so render-path calls are safe.
  // The render path reads the PUBLISHED width (the JSX fallback between
  // glides); the pump reads the observer-fed `composerWidthTargetRef` live.
  const composerWidthTarget = Math.min(Math.max(columnWidth ?? COMPOSER_MAX_WIDTH, 0), COMPOSER_MAX_WIDTH);
  const composerWidth = dockRef.current.layoutWidth(composerWidthTarget, dockReduced, performance.now());

  // The frame pump, de-Reacted (ticket 57b, §2.3): one rAF loop while
  // anything is in flight (the desktop's `request_animation_frame` in
  // prepaint/tick), but the frame's channels land as CSS custom properties
  // on the column (`writeDockGlideVars`) and the composer's evaluate pass
  // is invoked imperatively through `dockEvaluateRef` — ZERO React state
  // per frame (the old pump re-rendered the whole page per frame for the
  // 420/470 ms glide + the 0.32 s handoff; the audit's S1(b) #1). The loop
  // self-sustains on the MUTABLE dock state — the effect no longer re-arms
  // per frame — and runs the desktop's per-paint order: the pane sample
  // (the handoff's clock USED to advance on the pump's re-renders, so the
  // loop must sample it itself — observe_pane at shell.rs:7901-7906), the
  // transcript-width retention (:7915-7919), the tick (:5882-5885), the
  // width glide, the composer's evaluate (the child-first layout-effect
  // peer), then prepaint. The pane-progress leg is shell.rs:7907-7909's
  // `motion_active` peer: the handoff's 0.320 s clock can outlive the glide
  // AND the choreography, so `frame.active` alone would stop the loop early
  // and strand the composer mid-fade (the §2.4.2 warning) — the gate
  // re-reads the live progress on every frame.
  useEffect(() => {
    if (!dockFrame.active && !prepaintMovingRef.current && dockRef.current.paneProgress() === null) {
      // The glide is over and the settle commit has landed: the JSX
      // fallbacks are authoritative again. Clear the converged vars (the
      // removal never paints a jump — they equal the settled values within
      // the position epsilon) and null the live frame (the prop is the
      // authority between glides).
      clearDockGlideVars(chatColumnRef.current?.style ?? null);
      liveDockFrame.current = null;
      return;
    }
    dockGlideSignal.arm();
    const mounts = new DockMountSequencer();
    let raf = 0;
    let stopped = false;
    const pumpFrame = (): void => {
      if (stopped) {
        return;
      }
      const dock = dockRef.current;
      const nowMs = performance.now();
      const paneLive = dock.observePane(
        hasSelection,
        paneNowWidthRef.current,
        !dockReducedRef.current,
        nowMs,
      );
      dock.transcriptWidth(mainContentWidthRef.current, hasSelection, paneLive);
      const frame = dock.tick(hasSelection, dockReducedRef.current, nowMs);
      liveDockFrame.current = frame;
      const composerWidth = dock.layoutWidth(composerWidthTargetRef.current, dockReducedRef.current, nowMs);
      dockEvaluateRef.current?.();
      const wrapper = wrapperRef.current;
      const stack = bottomStackRef.current;
      if (wrapper !== null && stack !== null) {
        const stackRect = stack.getBoundingClientRect();
        // The wrapper's NATURAL slot (transform excluded — offsets are
        // layout values): the desktop's prepaint reads the layout bounds.
        const bounds = {
          left: stackRect.left + wrapper.offsetLeft,
          top: stackRect.top + wrapper.offsetTop,
          height: wrapper.offsetHeight,
        };
        const { dx, dy, moving } = dock.prepaint(bounds, viewportHeightRef.current, dockReducedRef.current, nowMs);
        wrapper.style.transform = `translate(${dx}px, ${dy}px)`;
        prepaintMovingRef.current = moving;
      } else if (wrapper !== null) {
        wrapper.style.transform = "";
        prepaintMovingRef.current = false;
      }
      writeDockGlideVars(
        chatColumnRef.current?.style ?? null,
        dockGlideChannels(frame, dock.opacity(), composerWidth, readSurfaceTreatment()),
      );
      // The phase sequencer — the glide's ONLY React state writes: the
      // chrome rows' mount crossings, published as the live frame so the
      // composer's `> 0` mounts flip exactly when the channels cross zero
      // (discrete, per-navigation; every other frame writes none).
      if (mounts.crossing(frame)) {
        setDockFrameState((prev) => (dockFrameEquals(prev, frame) ? prev : frame));
      }
      if (!frame.active && !prepaintMovingRef.current && dock.paneProgress() === null) {
        // Settled: ONE publish lands the settled frame — the settle commit's
        // JSX fallbacks, the composer's final evaluate (its `layout`
        // republish), and the per-commit stack publication carry the rest.
        // The vars stay set (converged = the settled values) until the
        // effect's stop branch above clears them, and the signal falls for
        // 59/63.
        setDockFrameState((prev) => (dockFrameEquals(prev, frame) ? prev : frame));
        dockGlideSignal.settle();
        return;
      }
      raf = requestAnimationFrame(pumpFrame);
    };
    raf = requestAnimationFrame(pumpFrame);
    return () => {
      stopped = true;
      cancelAnimationFrame(raf);
      dockGlideSignal.settle();
    };
    // `hasSelection` re-runs on a mid-glide reversal so the closure (and the
    // fresh `DockMountSequencer`) retarget with the next frame's tick;
    // `dockFrame.active` re-runs at the flip and the settle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dockFrame.active, hasSelection]);

  // The hero: full conversation canvas (`viewport − sidebar_now`, never
  // rescaled by the right pane), mounted while `!has_selection` or the dock
  // is still dissolving one away, outside the transcript's edge fade.
  // During the sidebar slide (ticket 57a) the width term is the PREVIOUS
  // target on the flip render (the hold that lets the flag commit arm the
  // CSS transition) and the settled target otherwise — the ANIMATION
  // between them is the browser's (`data-sidebar-tween`), never React
  // state (0 at phone, where the sidebar is an out-of-flow overlay). The
  // mount decision consumes
  // the MUTABLE frame ticked in THIS render (the same-render tick above,
  // ticket 35 — shell.rs:5883's `(!has_selection || dock_frame.active)`),
  // while `dockFrame` state drives the visuals; the phone layer mounts the
  // hero too (ticket 53 amended decision 5).
  const heroVisible = heroLayerMounted(hasSelection, dockRef.current.frame);
  const heroWidth = Math.max(viewport - heroSidebarTerm, 0);

  // The transcript outlet: selected chat → transcript; nothing selected →
  // the centered new-thread composition. A DEPARTING transcript (undocking)
  // keeps its pixels as visual history, fixed at the source column width
  // (`transcriptWidth`'s retained value, pinned on `.chat-body` below) and
  // occluded so it is not an interaction surface.
  const departing = !hasSelection && dockFrame.visuals.transcript > 0;
  // `finish_route_exit`: once the route is blank and the exit finished,
  // release the retained store.
  useEffect(() => {
    if (!hasSelection && !departing && storeRef.current !== null) {
      storeRef.current = null;
    }
  }, [hasSelection, departing]);
  const liveTranscript = hasSelection ? transcriptStore : departing ? storeRef.current : null;

  // Existing-chat navigation switches the outlet immediately. The outlet
  // keeps the destination's seed hidden until live arrival, with loading
  // feedback instead of retaining an unrelated chat for the roundtrip.
  const transcriptOpacity = departing || transcriptGeometryReady ? dockFrame.visuals.transcript : 0;
  const transcriptRise = 8 * (1 - dockFrame.visuals.transcript);

  // The jump pill's state, published by the transcript surface. The pill
  // itself renders over the composer (see `JumpPillAnchor`).
  const [jumpState, setJumpState] = useState<JumpButtonState | null>(null);
  const onJumpChange = useCallback((state: JumpButtonState) => {
    setJumpState((current) =>
      current?.shown === state.shown && state.shown === false ? current : state,
    );
  }, []);

  // `composer.is_sending()`: a send is still awaiting its confirmation. The
  // pending sends live on the app-wide echo store, so the strip sees them
  // even though the composer owns the sends themselves.
  const sending = useSyncExternalStore(
    useCallback((listener: () => void) => echoStore.subscribe(listener), []),
    useCallback(() => echoStore.forChat(chatId).length > 0, [chatId]),
  );

  // The session row's `started_at` — the working trailer's timer base
  // (`session_for(chat_id).started_at` on the desktop).
  const turnStartedAt = useMemo(() => {
    const started = snapshot.statuses.rows.find((row) => row.chatId === chatId)?.startedAt ?? null;
    if (started === null) {
      return null;
    }
    const parsed = Date.parse(started);
    return Number.isFinite(parsed) ? parsed : null;
  }, [snapshot, chatId]);

  // Opening a chat IS reading it (`mark_chat_seen`): the local stamp lands
  // first and stands whatever the mutation does, so a dropped `Mutate` never
  // makes a chat the user plainly looked at flash unread again.
  useEffect(() => {
    if (session === null || chatId === "") {
      return;
    }
    markChatSeen(session.client, chatId);
  }, [session, chatId]);

  // Mod+Enter on an empty composer activates the most recently queued row
  // (`activate_latest_queued`, queue.rs:1218-1244): Send now, interrupting
  // the current response. An edit/review gate or a host without queue
  // actions makes it a no-op. The web's engine-capability check stands in
  // for the desktop's host registry until ticket 31's fleet (ticket 16
  // defers the host routing itself).
  const hostSupportsActions =
    (session?.client.engineInfo?.capabilities ?? []).includes(MESSAGE_QUEUE_ACTIONS_V1);
  const activateLatestQueued = useCallback(() => {
    if (queueStore === null || editingRow !== null) {
      return;
    }
    const latest = queueStore.getSnapshot().rows.at(-1) ?? null;
    if (latest === null || !availableQueuePrimaryAction(latest.deliveryGate != null, hostSupportsActions)) {
      return;
    }
    void queueStore
      .sendNow(latest.id)
      .then((sent) => {
        if (!sent) {
          sidebarNotice.set("Couldn't send that message");
        }
      })
      .catch((error: unknown) => {
        sidebarNotice.set(`Couldn't send that message: ${error instanceof Error ? error.message : String(error)}`);
      });
  }, [queueStore, editingRow, hostSupportsActions]);

  /**
   * The working trailer's failed-send retry — the desktop's `retry_send`
   * (transcript.rs:5190): skip when the engine is not connected, restart the
   * grace clocks (same message ids, so the overlay returns to Sending), and
   * re-deliver through the engine's `RETRY_DELIVERY` — it re-issues the dead
   * durable commands with their original message ids, and the host's
   * user-entry pre-write dedupes by id, so no bubble doubles.
   */
  const onRetryDelivery = useCallback(() => {
    if (session === null || session.client.state !== "connected" || chatId === "") {
      return;
    }
    echoStore.restartGrace(chatId, Date.now());
    void session.client
      .call(methods.RETRY_DELIVERY, { chatId })
      .catch((error: unknown) => {
        sidebarNotice.set(
          `Could not retry delivery: ${error instanceof Error ? error.message : String(error)}`,
        );
      });
  }, [session, chatId]);

  // `NewThreadTransitionStarted`'s host half: the minted chat exists, the
  // route observation (this navigation) drives the dock —
  // `select_chat`'s commit (shell.rs:1289-1295 just notifies).
  const onNewThreadLaunched = useCallback(
    (mintedId: string) => {
      // §2.3 (composer.rs:6047-6050): the desktop mints the new chat id
      // SCOPED; the web mints raw on the wire (request-routing decodes
      // either form) and scopes HERE, at the navigation — the same call
      // add-space's optimistic space rows make. The merged fleet rows are
      // all scoped, so the URL id then matches `chatPageRow`'s exact
      // compare and the not-found page stays a last resort for genuinely
      // foreign ids.
      const scoped = session === null ? mintedId : encodeScopedId(session.engine.baseUrl, mintedId);
      void navigate({ to: "/chat/$chatId", params: { chatId: scoped } });
    },
    [navigate, session],
  );

  // The titlebar is the shell's; the route fills its identity and the `+`'s
  // handler. The right pane, its toggle, its surface tabs and its expand
  // control are all shell chrome — see `state/chrome.ts` for why they must
  // not travel through here. `onNewSession` is null when the chat is not in
  // the engine's list: `titlebar_plus_alpha` requires a SELECTED chat, and a
  // missing row selects nothing. Note `pane` is deliberately NOT a dep: this
  // effect clears the store on every dep change, and a toggle rebuilding the
  // chrome is what used to tear the pane column down mid-animation.
  useTitlebar(
    () => ({
      identity: row === undefined ? null : <ChatIdentity row={row} />,
      onNewSession: row === undefined ? null : () => emitShortcut("new-chat"),
    }),
    [chatId, row?.chat.id, row?.chat.title, row?.folder, row?.harness],
  );

  if (!snapshot.chats.loaded) {
    return <div className="chat-page" />;
  }
  if (row === undefined && hasSelection) {
    return (
      <div className="empty-state">
        <p>That chat is not in this engine's list.</p>
        <Link to="/" className="btn btn-ghost">
          Back to chats
        </Link>
      </div>
    );
  }
  return (
    <div className="chat-page">
      {/*
        The conversation column: the hero, the transcript underlay, the
        spacer, the bottom chrome stack — `render_main`'s `#chat-dropzone`
        children in order. The hero is FIRST and deliberately outside the
        transcript's edge-fade scope (it paints under the overlaid
        titlebar).
      */}
      <div className="chat-column" ref={chatColumnRef}>
        {heroVisible && (
          <NewThreadCanvas
            viewportHeight={viewportHeight}
            heroWidth={heroWidth}
            dissolve={dockFrame.visuals.dissolve}
            sidebarTween={sidebarTweening}
          />
        )}
        <div
          className="chat-body"
          style={{
            // Ticket 57b: the glide's transcript channels ride the pump's
            // CSS vars (written per frame on the column); the fallback is
            // the last PUBLISHED frame — the flip, a mount crossing, or the
            // settle — so a mid-glide render can never clobber the live
            // values, and the settle commit's fallback takes over the
            // instant the vars clear.
            opacity: `var(--rb-dock-transcript-opacity, ${transcriptOpacity})`,
            transform: `translateY(var(--rb-dock-transcript-rise, ${transcriptRise}px))`,
            // `transcript_width`'s retained value (shell.rs:5917-5938's
            // `.w(px(transcript_width))`): while a departing handoff runs,
            // the wrapper is pinned to the SOURCE column's width so the
            // fading rows never reflow into the canvas-wide layout — the
            // exit flash the retention exists to prevent. `inset: 0` plus
            // a width is over-constrained in LTR: left + width win.
            ...(departing && paneHandoffLive ? { width: `${retainedTranscriptWidth}px` } : {}),
          }}
        >
          {liveTranscript !== null && session !== null ? (
            <ChatTranscriptOutlet store={liveTranscript} departing={departing}>
              {(activeStore) => (
                <TranscriptView
                  client={session.client}
                  docId={chatId}
                  deviceId={deviceId}
                  store={activeStore}
                  markdownSurface={markdownSurface}
                  onContextUsage={setContextUsage}
                  onRetryDelivery={onRetryDelivery}
                  onJumpChange={onJumpChange}
                  indicator={row?.status ?? "idle"}
                  turnStartedAt={turnStartedAt}
                  onOpenSubagent={onOpenSubagent}
                  deliveryDegraded={deliveryDegraded}
                />
              )}
            </ChatTranscriptOutlet>
          ) : null}
          {/*
            A departing transcript is visual history, not an active
            interaction surface bound to the newly blank route
            (shell.rs:5940-5944).
          */}
          {departing && <div className="departing-veil" aria-hidden="true" />}
        </div>
        {/*
          The bottom chrome stack (`render_main`'s flex-none bottom section):
          the reserved status strip (both routes — the canvas shows the idle
          indicator), the persistent composer — one entity, its wrapper in
          this slot, re-anchored by the dock — with the jump pill floating
          over it, the queue edit toolbar, and the terminal drawer LAST
          (`render_terminal_container`, shell.rs:6124 — the dock sits below
          the composer at the column's bottom). The drawer measures into the
          stack like every sibling, so the transcript's bottom clearance and
          fade band track it; its own store is the DRAWER's — the pane's
          Terminal surfaces ride a separate, independent host.
        */}
        <div className="bottom-stack" ref={bottomStackRef}>
          <StatusStrip status={row?.status ?? "idle"} sending={sending} />
          {session !== null && (
            <div
              className="persistent-composer"
              id="persistent-composer"
              ref={wrapperRef}
              style={{
                // Ticket 57b: the glide's width and the handoff's
                // fade-through ride the pump's CSS vars — the fallbacks are
                // the last render's values, clobber-proof mid-glide.
                width: `var(--rb-dock-composer-width, ${composerWidth}px)`,
                opacity: `var(--rb-dock-pane-opacity, ${dockRef.current.opacity()})`,
              }}
            >
              <Composer
                session={session}
                chat={effectiveChat}
                catalog={session.catalog}
                // The LIVE store, never the swap-retained one: the
                // composer's target is the destination chat — the wizard's
                // rows must not offer the previous chat's entries.
                transcript={liveTranscript}
                availableWidth={composerWidth}
                // Ticket 64 §2.4: the observer-fed live target — the
                // evaluate pass's strip budget reads it even while the
                // published prop defers through the sidebar glide.
                liveAvailableWidth={composerWidthTargetRef}
                editingMessage={editingRow}
                onEditFinish={onEditFinish}
                onEditCancel={onEditCancel}
                editCommitRef={editCommitRef}
                activateLatestQueued={activateLatestQueued}
                dockFrame={dockFrame}
                liveDockFrame={liveDockFrame}
                dockEvaluateRef={dockEvaluateRef}
                onNewThreadLaunched={onNewThreadLaunched}
                dockCorrectionRef={dockCorrectionRef}
                queueSlot={
                  queueStore !== null && deviceId !== null ? (
                    <QueueStoreProvider value={queueStore}>
                      <QueuePanel
                        client={session.client}
                        editorDeviceId={deviceId}
                        onEditRow={onEditRow}
                        editingRowId={editingRow?.id ?? null}
                        editFinishing={editFinishing}
                        composerWidth={columnWidth}
                        hostSupportsActions={hostSupportsActions}
                        onSaveEdit={() => editCommitRef.current?.()}
                        onEditCancel={onEditCancel}
                      />
                    </QueueStoreProvider>
                  ) : null
                }
                footerSlot={
                  <ComposerFooter chat={effectiveChat} crSummary={crSummary} contextUsage={contextUsage} />
                }
              />
              {hasSelection && <JumpPillAnchor state={jumpState} />}
            </div>
          )}
          <TerminalDock store={drawerTerminalStore} chatId={chatId} />
        </div>
      </div>
    </div>
  );
}

/** The window's inner height — the dock anchors the hero in viewport space. */
function useViewportHeight(): number {
  const [height, setHeight] = useState(() =>
    typeof window === "undefined" ? 800 : window.innerHeight,
  );
  useEffect(() => {
    const onResize = () => setHeight(window.innerHeight);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return height;
}

/** `prefers-reduced-motion` at first paint. */
function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * The live edge-bounce offset of a seam's 5px pulse (the `--rb-*-edge-offset`
 * variables `pane-seam.tsx` writes on the document element): the desktop's
 * `sidebar_now()`/`right_now()` include them, so a width sampled mid-bounce
 * carries the painted value.
 */
function readEdgeOffset(varName: string): number {
  if (typeof document === "undefined") {
    return 0;
  }
  const raw = window.getComputedStyle(document.documentElement).getPropertyValue(varName);
  const parsed = Number.parseFloat(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}

/** `right_now()`'s bounce leg (shell.rs:3822-3825) — `--rb-pane-edge-offset`. */
function readPaneEdgeBounceOffset(): number {
  return readEdgeOffset("--rb-pane-edge-offset");
}

/**
 * The hero's dissolve multiplier's surface leg, read live per pump frame —
 * the same resolution `NewThreadBackground`'s reactive hook performs (the
 * `data-surface` attribute on the document root, installed by `theme.ts`).
 */
function readSurfaceTreatment(): SurfaceTreatment {
  if (typeof document === "undefined") {
    return "opaque";
  }
  return document.documentElement.dataset.surface === "frosted" ? "frosted" : "opaque";
}

/**
 * `render_jump_to_bottom`'s positioner: the pill floats 36px ABOVE the
 * composer, horizontally centered across the composer's own width less its
 * 10px right inset. It paints outside the transcript's fade, over whatever
 * sits above the composer.
 */
function JumpPillAnchor({ state }: { state: JumpButtonState | null }) {
  if (state === null || !state.shown) {
    return null;
  }
  return (
    <div className="jump-pill-anchor">
      <JumpPill onClick={state.jump} />
    </div>
  );
}

/**
 * The titlebar's identity group — the desktop's (`tabs.rs:318-350`): the
 * harness's 14px brand mark, the 12px/500 title at `text @ 85%`, and the
 * 12px `folder @ device` tag at `text_muted @ 50%`. Nothing else — no badge,
 * no archived marker; the group's own gap (6px) and truncation live in the
 * stylesheet.
 */
function ChatIdentity({ row }: { row: ChatRow }) {
  const brand = row.harness === null ? null : harnessBrandIcon(row.harness);
  return (
    <>
      {brand !== null && (
        <Icon
          name={brand.name}
          size={14}
          className="identity-brand"
          style={brand.tint === null ? undefined : { color: brand.tint }}
        />
      )}
      <span className="identity-title">{row.chat.title ?? "New session"}</span>
      <span className="identity-folder">{row.folder}</span>
    </>
  );
}
