import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { Icon, harnessBrandIcon } from "@zeron/icons";
import { parseScopedId } from "@zeron/engine-client";
import { useEngineSession, useEngineSessions } from "../state/session-provider";
import type { EngineSession } from "../state/engine-session";
import { engineStatesOf, fleetLocalDeviceId, useFleet, useFleetRegistry, useFleetSnapshot } from "../state/fleet";
import { useNow } from "../state/hooks";
import { useSidebar } from "../state/sidebar";
import { sidebarNotice } from "../state/notice";
import { cycleTarget, onShortcut } from "../state/shortcuts";
import { useJumpHints, visibleJumpOrder } from "../state/jump-hints";
import { describeMutateError, setChatArchived } from "../lib/chat-actions";
import {
  chatListRows,
  chatRowHeight,
  healedSpaceFilter,
  resortOffsets,
  sidebarGroups,
  sidebarKeyOrderChanged,
  sidebarRowHeight,
  sidebarVisibleOrder,
  statusWord,
  type ChatRow,
  type SidebarKeyed,
} from "../lib/view";
import { useFleetChatChangeRequests } from "../state/change-requests-store";
import { useChatMenu } from "./chat-menu";
import { PinnedSection } from "./pinned-section";
import {
  commitSessionDrop,
  commitVisiblePinReorder,
  pinOrderedRows,
  pinnedHeaderKeyedHeight,
  pinnedSessionClampedIndex,
  pinnedSessionDropIndex,
  sidebarPinProfileKey,
  SIDEBAR_PINNED_DIVIDER_HEIGHT,
  SIDEBAR_PINNED_DIVIDER_KEY,
  SIDEBAR_PINNED_HEADER_KEY,
} from "../lib/sidebar-pins";
import { preventNativeSidebarRowDrag } from "../lib/sidebar-drag-events";
import { sidebarStore } from "../state/sidebar";
import { activeSidebarSections, sectionMembership, sectionRows } from "../lib/sidebar-sections";
import { CreateSectionDialog, CustomSection, customSectionKeyedHeight } from "./sidebar-sections";
import { GlyphSpinner } from "./glyph-spinner";
import { SidebarFadedLabel } from "./sidebar-faded-label";
import { ProjectIconMark } from "./project-monogram";
import { projectIconsStore } from "../lib/project-icons";
import { Tooltip } from "./ui/Tooltip";
import { TOOLTIP_VIEW_OPTIONS_MS } from "./ui/Tooltip";
import {
  SidebarDisclosureBody,
  SidebarDisclosureHeader,
  useSidebarDisclosure,
} from "./sidebar-disclosure";
import { ChangeRequestBadge } from "./change-request-badge";

/** `shell.rs::SIDEBAR_LIST_GAP` — the flex gap between sidebar rows. */
export const SIDEBAR_LIST_GAP = 2;
/**
 * `shell.rs::SIDEBAR_ACTIVE_HARNESS_*` — the active card keeps its harness
 * identity close on the standard 8px rhythm (SPACE_SM).
 */
export const SIDEBAR_ACTIVE_HARNESS_TITLE_GAP = 8;
export const SIDEBAR_ACTIVE_HARNESS_ICON_SIZE = 13;
/** `spaces.rs::SIDEBAR_SECTION_GAP` — every disclosure section's top band. */
const SIDEBAR_SECTION_GAP = 12;
/** `spaces.rs::SIDEBAR_DISCLOSURE_BODY_INSET` — the body's handoff padding. */
const SIDEBAR_DISCLOSURE_BODY_INSET = 4;
/**
 * `spaces.rs::SIDEBAR_DISCLOSURE_SECTION_HEIGHT` = SECTION_GAP + HEADER (28):
 * a collapsed section's keyed height for the FLIP diff.
 */
const SIDEBAR_DISCLOSURE_SECTION_HEIGHT = SIDEBAR_SECTION_GAP + 28;

/**
 * `shell.rs::RESORT` — 260ms `cubic-bezier(0.22, 1, 0.36, 1)`. A UI-level
 * spec the proto motion catalog does not carry (it lives beside the FLIP
 * code in shell.rs, not in motion.rs), so the web declares it here and as
 * `--rb-motion-resort` in app.css rather than through @zeron/theme.
 */
export const SIDEBAR_RESORT_MS = 260;
export const SIDEBAR_RESORT_CURVE: readonly [number, number, number, number] = [0.22, 1, 0.36, 1];
export const SIDEBAR_RESORT_EASING = `cubic-bezier(${SIDEBAR_RESORT_CURVE.join(", ")})`;

function prefersReducedMotion(): boolean {
  const query = (globalThis as { matchMedia?: (query: string) => { matches: boolean } }).matchMedia;
  if (query === undefined) {
    return false;
  }
  return query("(prefers-reduced-motion: reduce)").matches;
}

interface ResortState {
  readonly epoch: number;
  readonly offsets: ReadonlyMap<string, number>;
  readonly newKeys: ReadonlySet<string>;
}

const RESORT_NONE: ResortState = { epoch: 0, offsets: new Map(), newKeys: new Set() };

/**
 * The §2.7 FLIP diff, in render order: `useLayoutEffect` compares this
 * render's keyed list against the previous one AFTER the DOM is laid out at
 * its new positions but BEFORE paint — a reorder computes each surviving
 * key's paint-only start offset and bumps the epoch, so the moved elements
 * animate from the offset down to zero over `RESORT`. First fill never
 * animates; a height-only change (a disclosure opening) is not a reorder;
 * removals just go (their survivors glide up to close the gap).
 */
function useSidebarResort(keyed: readonly SidebarKeyed[], resetEpoch = 0): ResortState {
  const prev = useRef<readonly SidebarKeyed[]>([]);
  const prevReset = useRef(resetEpoch);
  const [state, setState] = useState<ResortState>(RESORT_NONE);
  useLayoutEffect(() => {
    const old = prev.current;
    prev.current = keyed;
    if (prevReset.current !== resetEpoch) {
      // A pin-drag commit placed the rows visually already — adopt the new
      // order without a glide (`commit_pinned_session_drag` clears the
      // desktop's resort bookkeeping the same way).
      prevReset.current = resetEpoch;
      return;
    }
    if (old.length === 0 || !sidebarKeyOrderChanged(old, keyed)) {
      return;
    }
    const offsets = resortOffsets(old, keyed, SIDEBAR_LIST_GAP);
    const oldKeys = new Set(old.map((entry) => entry.key));
    const newKeys = new Set(
      keyed.filter((entry) => !oldKeys.has(entry.key)).map((entry) => entry.key),
    );
    if (offsets.size === 0 && newKeys.size === 0) {
      return;
    }
    setState((current) => ({ epoch: current.epoch + 1, offsets, newKeys }));
  }, [keyed, resetEpoch]);
  return state;
}

/**
 * One keyed element's paint-only glide: Web Animations API from
 * `translateY(dy)` to none, so layout stays at the final position and only
 * the paint offset tweens — the desktop's `with_animation` relative-inset
 * equivalent. New keys fade in via the `chat-row-in` CSS class instead
 * (fresh mount, the animation runs once). Reduced motion skips both.
 */
function useResortGlide(ref: React.RefObject<HTMLElement | null>, dy: number | undefined, epoch: number): void {
  useLayoutEffect(() => {
    const el = ref.current;
    if (el === null || dy === undefined || dy === 0 || prefersReducedMotion()) {
      return;
    }
    const animation = el.animate(
      [{ transform: `translateY(${dy}px)` }, { transform: "translateY(0)" }],
      {
        duration: SIDEBAR_RESORT_MS,
        easing: SIDEBAR_RESORT_EASING,
        fill: "none",
      },
    );
    return () => {
      animation.cancel();
    };
    // The epoch pins the effect to one resort: a later resort with the same
    // dy re-runs, a re-render with the same state does not.
  }, [ref, dy, epoch]);
}

export function ChatList() {
  // The MERGED fleet snapshot: every paired engine's chats under scoped ids,
  // one flat list — the sidebar never knows which engine owns which row.
  const snapshot = useFleetSnapshot();
  const registry = useFleetRegistry();
  const fleet = useFleet();
  const sessions = useEngineSessions();
  const sidebar = useSidebar();
  const now = useNow(10_000);
  const navigate = useNavigate();
  // The ACTIVE engine's own device is the fleet's "local device" — the
  // group its chats land in under ByDevice, promoted to the top (scoped,
  // to match the projected rows' device ids).
  const localDeviceId = fleetLocalDeviceId(registry, fleet.active);
  const engineStates = engineStatesOf(registry);

  const chats = snapshot.chats;
  const filter = snapshot === null ? null : healedSpaceFilter(sidebar.spaceFilter, snapshot.spaces.rows);
  const visible =
    chats.error !== null
      ? []
      : filter === null
        ? chats.rows
        : chats.rows.filter((chat) => chat.spaceId !== undefined && chat.spaceId === filter);
  const changeRequests = useFleetChatChangeRequests(sessions, visible);

  // The device-group collapse keys — in-memory only, exactly like the
  // desktop's `sidebar_collapsed_groups` (a reload re-expands every group).
  const [collapsedGroups, setCollapsedGroups] = useState<ReadonlySet<string>>(() => new Set());
  // Bumped by a pin-drag commit: the drop leaves every row at its final slot,
  // so the FLIP diff adopts the new order without gliding it.
  const [pinResetEpoch, setPinResetEpoch] = useState(0);

  const rows =
    chats.error === null && chats.loaded
      ? chatListRows(visible, snapshot.spaces.rows, snapshot.statuses.rows, now, snapshot.devices.rows, {
          sort: sidebar.sort,
          showHarness: sidebar.showHarness,
          showBranch: sidebar.showBranch,
          showPullRequest: sidebar.showPullRequest,
          changeRequests,
          engineStates,
          // A dangling spaceId hides its chat only once the spaces frame
          // is in; until then the row renders with the "?" label (ticket 43).
          spacesLoaded: snapshot.spaces.loaded,
        })
      : [];

  // Pins are bucketed per workspace profile (settings.rs's
  // `sidebar_pinned_session_ids_by_profile`) — one bucket per registry engine
  // in pairing order; zeron engines are all local-scoped in practice, so
  // this is usually the one shared "local" bucket.
  const pinProfileKeys: string[] = [];
  for (const engine of registry.engines) {
    const key = sidebarPinProfileKey(engine.info?.workspaceScope ?? null, engine.info?.deviceId ?? null);
    if (key !== null && !pinProfileKeys.includes(key)) {
      pinProfileKeys.push(key);
    }
  }
  const pinnedIds = pinProfileKeys.flatMap((key) => sidebar.pinnedByProfile[key] ?? []);

  // Custom sections read the ACTIVE workspace profile — the desktop's
  // `active_sidebar_pin_profile_key` (sections and pins share the profile
  // bucket model; membership is exclusive between them). Web pins land
  // synchronously in the same settings write, so no optimistic mask is
  // needed (the desktop's mask covers its queued pin burst seam).
  const activeRegistryEngine =
    registry.engines.find((engine) => engine.key === fleet.active) ?? null;
  const activeProfileKey =
    activeRegistryEngine === null
      ? null
      : sidebarPinProfileKey(
          activeRegistryEngine.info?.workspaceScope ?? null,
          activeRegistryEngine.info?.deviceId ?? null,
        );
  const sections = activeSidebarSections(sidebar.sectionsByProfile, activeProfileKey);
  // `active_sidebar_pins`: a section's member hides its pin (the bucket
  // keeps the id; leaving the section restores the pin to view).
  const sectionClaimed = new Set(sections.flatMap((section) => section.sessionIds));
  const displayedPins = pinnedIds.filter((id) => !sectionClaimed.has(id));

  // `retain_known_pins` on the desktop's synced-chats tick, per ACTIVE
  // profile: another profile's absent chats are not deletions, and an engine
  // that has not loaded yet never judges its own pins.
  useEffect(() => {
    if (!chats.loaded || chats.error !== null) {
      return;
    }
    const keys = registry.engines
      .filter((engine) => engine.chats.loaded)
      .map((engine) => sidebarPinProfileKey(engine.info?.workspaceScope ?? null, engine.info?.deviceId ?? null))
      .filter((key): key is string => key !== null);
    sidebarStore.pruneUnknownPins(keys, new Set(chats.rows.map((chat) => chat.id)));
  }, [chats.loaded, chats.error, chats.rows, registry]);

  // Project icons (ticket 05): reconcile the per-space artwork cache with
  // the fleet's space list — new spaces probe `ICON_PATHS` through their
  // owning engine's files RPC (the desktop's `render_project_icon` cache
  // fill), vanished spaces drop their entry, and the view toggle hides the
  // surface outright. Rows read the settled entries through
  // `useProjectIcon`; while a probe is in flight the monogram stays.
  useEffect(() => {
    if (!sidebar.showProjectIcon) {
      projectIconsStore.dropAll();
      return;
    }
    projectIconsStore.ensure(snapshot.spaces.rows, sessions, localDeviceId);
  }, [sidebar.showProjectIcon, snapshot.spaces.rows, sessions, localDeviceId]);

  // The pinned section leads; custom sections follow (claimed rows render
  // inside their section, never in the regular groups — `render_active_rows`
  // splits claimed/unclaimed first, upstream 86249cf0); regular rows keep
  // the existing grouping over the unclaimed remainder.
  const { groups: sectionGroups, remaining } = sectionRows(sections, rows);
  const { pinned: pinnedRows, regular: regularRows } = pinOrderedRows(remaining, displayedPins);
  const hasPinnedDivider =
    pinnedRows.length > 0 && (regularRows.length > 0 || sectionGroups.length > 0);
  const pinnedOpen = sidebar.pinnedOpen;
  const compact = sidebar.compact;
  const showLabel = sidebar.showProjectLabel;
  const showProjectIcon = sidebar.showProjectIcon;
  const groups = sidebarGroups(regularRows, sidebar.organization, localDeviceId);

  // ── Drag transfers between Pinned and regular sessions (6851fc34) ───────
  // A regular row's press arms a transfer: a release inside the pinned
  // section pins the chat at the drop index
  // (`finish_sidebar_session_transfer`'s `SidebarSessionDrop::Pinned`; a
  // closed section opens on success). Releasing anywhere else is a no-op —
  // regular rows never acquire a manual order — and the FLIP resort glide
  // carries the row into the section on commit. The pinned-section side of
  // the gesture (dragging OUT) lives in PinnedSection's `onTransferOut`.
  // The state is the dragging chat alone (0f152647 removed the highlights);
  // the drop index reads the live DOM at release, like the desktop's
  // prepaint row centers.
  const [transferIn, setTransferIn] = useState<string | null>(null);
  const pinnedSectionRef = useRef<HTMLElement | null>(null);
  // A completed transfer drag suppresses the click its pointerup would fire.
  const suppressRowClickRef = useRef(false);
  const sidebarRef = useRef<HTMLDivElement | null>(null);
  const bucketsRef = useRef<{ keys: string[]; byProfile: Readonly<Record<string, readonly string[]>>; open: boolean }>({ keys: [], byProfile: {}, open: true });
  bucketsRef.current = {
    keys: pinProfileKeys,
    byProfile: sidebar.pinnedByProfile,
    open: pinnedOpen,
  };
  const visiblePinsRef = useRef<string[]>([]);
  visiblePinsRef.current = pinnedRows.map((row) => row.chat.id);
  // The live section list + profile key for the drop commit (the drag's
  // window outlives the render that armed it, like the buckets ref).
  const sectionsRef = useRef<{ sections: typeof sections; profileKey: string | null }>({
    sections: [],
    profileKey: null,
  });
  sectionsRef.current = { sections, profileKey: activeProfileKey };
  // The pinned section's root bounds + the rows group's bounds (the drop
  // index math reads them off the live DOM, like the desktop's prepaint
  // row_centers).
  const pinnedDropIndex = (pointer: { clientX: number; clientY: number }): number | null => {
    const section = pinnedSectionRef.current;
    if (section === null) {
      return null;
    }
    const sectionBounds = section.getBoundingClientRect();
    if (
      pointer.clientX < sectionBounds.left ||
      pointer.clientX > sectionBounds.right ||
      pointer.clientY < sectionBounds.top ||
      pointer.clientY > sectionBounds.bottom
    ) {
      return null;
    }
    const group = section.querySelector<HTMLElement>('[data-testid="sidebar-pinned-sessions"]');
    if (group !== null) {
      const groupBounds = group.getBoundingClientRect();
      if (pointer.clientY >= groupBounds.top && pointer.clientY <= groupBounds.bottom) {
        const count = visiblePinsRef.current.length;
        if (count === 0) {
          return 0;
        }
        const relY = pointer.clientY - groupBounds.top;
        return pinnedSessionDropIndex(relY, count) ?? pinnedSessionClampedIndex(relY, count) ?? 0;
      }
    }
    // The header (or a collapsed body) pins at the top.
    return 0;
  };
  const finishTransferIn = (chatId: string, pointer: { clientX: number; clientY: number }): void => {
    // `finish_sidebar_session_transfer`'s target lattice, hit-tested at the
    // release point: a custom section element claims it first (header, body,
    // or the "Drop sessions here" strip — `SidebarSessionDrop::Section`);
    // then the pinned section (`Pinned(index)`); then a regular row
    // (`Regular` — membership clears, a pinned chat unpins). Anything else
    // (empty space, Archived) is a cancel: regular rows never acquire a
    // manual order.
    const sectionHit = dropSectionAt(pointer);
    if (sectionHit !== null) {
      const { sections: live, profileKey } = sectionsRef.current;
      // The section write validates the target still exists (a vanished id
      // refuses the drop, like the desktop's early return).
      if (
        profileKey !== null &&
        live.some((section) => section.id === sectionHit) &&
        (sectionMembership(live, chatId) !== sectionHit || pinnedInBuckets(chatId))
      ) {
        unpinEverywhere(chatId);
        sidebarStore.assignSidebarSection(profileKey, chatId, sectionHit);
      }
      return;
    }
    const index = pinnedDropIndex(pointer);
    if (index !== null) {
      const { keys, byProfile, open } = bucketsRef.current;
      const buckets: Record<string, readonly string[]> = {};
      for (const key of keys) {
        buckets[key] = byProfile[key] ?? [];
      }
      sidebarStore.replacePinsByProfile(
        commitSessionDrop(buckets, visiblePinsRef.current, chatId, { kind: "pinned", index }),
      );
      // The Pinned arm's target section is None: pinning clears membership
      // at the same commit.
      sidebarStore.assignSidebarSection(sectionsRef.current.profileKey, chatId, null);
      if (!open) {
        sidebarStore.setPinnedOpen(true);
      }
      return;
    }
    const regularHit = dropRegularRowAt(pointer);
    if (regularHit) {
      const { sections: live, profileKey } = sectionsRef.current;
      // The Regular arm only writes when something changes: a member leaves
      // its section, a pinned chat unpins — an unclaimed row's drop is a
      // no-op that leaves recency order alone.
      if (sectionMembership(live, chatId) !== null || pinnedInBuckets(chatId)) {
        unpinEverywhere(chatId);
        sidebarStore.assignSidebarSection(profileKey, chatId, null);
      }
    }
  };
  /** The chat's id in ANY profile bucket — the raw ledger, mask-free. */
  const pinnedInBuckets = (chatId: string): boolean => {
    const { byProfile } = bucketsRef.current;
    return Object.values(byProfile).some((list) => list.includes(chatId));
  };
  /** `sidebar_session_drop_pins`' Regular/Section arm: the chat leaves every bucket. */
  const unpinEverywhere = (chatId: string): void => {
    if (!pinnedInBuckets(chatId)) {
      return;
    }
    const { keys, byProfile } = bucketsRef.current;
    const buckets: Record<string, readonly string[]> = {};
    for (const key of keys) {
      buckets[key] = byProfile[key] ?? [];
    }
    sidebarStore.replacePinsByProfile(
      commitSessionDrop(buckets, visiblePinsRef.current, chatId, { kind: "regular" }),
    );
  };
  /**
   * The custom section under a release point, if any — the live DOM at
   * pointerup, like the desktop's `on_drop` on the section element (a
   * dropped chat resolves its section by hit-testing, not bookkeeping).
   */
  const dropSectionAt = (pointer: { clientX: number; clientY: number }): string | null => {
    const hit = document.elementFromPoint?.(pointer.clientX, pointer.clientY);
    const section = hit instanceof Element ? hit.closest("[data-sidebar-section-id]") : null;
    if (section instanceof HTMLElement) {
      return section.dataset.sidebarSectionId ?? null;
    }
    return null;
  };
  /**
   * A release over a regular row OUTSIDE the custom sections (their rows
   * resolve to their section first) — the desktop's per-row `on_drop`
   * carrying `SidebarSessionDrop::Regular`.
   */
  const dropRegularRowAt = (pointer: { clientX: number; clientY: number }): boolean => {
    const hit = document.elementFromPoint?.(pointer.clientX, pointer.clientY);
    if (!(hit instanceof Element)) {
      return false;
    }
    const row = hit.closest(".regular-row");
    return row !== null && row.closest("[data-sidebar-section-id]") === null;
  };
  /** A pointer must travel this far before the press reads as a drag. */
  const DRAG_ARM_PX = 4;
  const armTransferIn = (event: React.PointerEvent, chatId: string): void => {
    if (event.button !== 0) {
      return;
    }
    // Interactive corners (the Archive pill) own their press.
    if ((event.target as HTMLElement).closest("button") !== null) {
      return;
    }
    // Pinned rows carry their own gesture (PinnedSection's reorder/transfer).
    if ((event.target as HTMLElement).closest('[data-testid="sidebar-pinned-sessions"]') !== null) {
      return;
    }
    const startX = event.clientX;
    const startY = event.clientY;
    let moved = false;
    const onMove = (move: PointerEvent): void => {
      // `contain_pinned_session_drag`: leaving the sidebar's column cancels.
      const sidebar = sidebarRef.current;
      if (sidebar !== null) {
        const bounds = sidebar.getBoundingClientRect();
        if (move.clientX < bounds.left || move.clientX > bounds.right) {
          cancel();
          return;
        }
      }
      if (!moved) {
        if (Math.abs(move.clientX - startX) <= DRAG_ARM_PX && Math.abs(move.clientY - startY) <= DRAG_ARM_PX) {
          return;
        }
        moved = true;
        setTransferIn(chatId);
      }
    };
    const teardown = (): void => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", onKey, true);
    };
    const finish = (up: PointerEvent): void => {
      teardown();
      setTransferIn(null);
      if (moved) {
        finishTransferIn(chatId, up);
        // The pointerup lands as a click on the row's link — swallow it.
        suppressRowClickRef.current = true;
        window.setTimeout(() => {
          suppressRowClickRef.current = false;
        }, 0);
      }
    };
    const cancel = (): void => {
      teardown();
      setTransferIn(null);
    };
    const onKey = (key: KeyboardEvent): void => {
      if (key.key === "Escape") {
        key.preventDefault();
        key.stopPropagation();
        cancel();
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", onKey, true);
  };
  const suppressTransferClick = (): boolean => suppressRowClickRef.current;

  // ── The keyboard's sidebar half (ticket 12) ─────────────────────────────
  // The DISPLAYED order — `sidebar_visible_order`: what cycle, jump, and the
  // jump-hint chips all read, so keyboard order never drifts from the screen.
  // A collapsed pinned section hides its rows, so they hold no slot here;
  // open custom sections' members slot between the pins and the unclaimed
  // rows, a collapsed section's members hold no slot either.
  const order = sidebarVisibleOrder(
    rows,
    sidebar.organization,
    localDeviceId,
    displayedPins,
    pinnedOpen,
    sections,
  );

  // The chips: while the hints are visible, the first nine rows carry the
  // slot's `badgeCombo` text in the corner — ticket 08's `.chat-row-jump`
  // class renders it, this module supplies the label from the same order the
  // jump shortcut targets.
  const hints = useJumpHints();
  const jumpSlotById: Map<string, number> | null = hints.visible
    ? new Map(visibleJumpOrder(order).map((id, slot) => [id, slot] as const))
    : null;
  const jumpLabelFor = (chatId: string): string | null => {
    const slot = jumpSlotById?.get(chatId);
    return slot === undefined ? null : (hints.combos[slot] ?? null);
  };

  // The session-nav shortcuts' execution half. The dispatcher in AppShell
  // holds the route and overlay guards; these handlers act on the live
  // order through a ref, so a snapshot tick never re-subscribes them.
  const routed = useEngineSession();
  const navRef = useRef({ order, session: routed });
  navRef.current = { order, session: routed };
  useEffect(() => {
    const selectedChatId = (): string | null => {
      const match = /^\/chat\/([^/]+)\/?$/.exec(window.location.pathname);
      if (match === null || match[1] === undefined) {
        return null;
      }
      try {
        return decodeURIComponent(match[1]);
      } catch {
        return match[1];
      }
    };
    const cycleTo = (forward: boolean): void => {
      const target = cycleTarget(navRef.current.order, selectedChatId(), forward);
      if (target !== null) {
        void navigate({ to: "/chat/$chatId", params: { chatId: target } });
      }
    };
    const offs = [
      onShortcut("next-session", () => cycleTo(true)),
      onShortcut("prev-session", () => cycleTo(false)),
      // `jump_to_session`: a slot past the end does nothing; the target takes
      // the same path a click on that row takes.
      onShortcut("jump-session", (detail) => {
        const id = navRef.current.order[detail.slot ?? -1];
        if (id !== undefined) {
          void navigate({ to: "/chat/$chatId", params: { chatId: id } });
        }
      }),
      // `archive_selected_chat` — the open chat moves to the archived shelf;
      // archiving never closes an open chat.
      onShortcut("archive-session", () => {
        const chatId = selectedChatId();
        const liveSession = navRef.current.session;
        if (chatId === null || liveSession === null) {
          return;
        }
        setChatArchived(liveSession.client, chatId, true).catch((error: unknown) => {
          sidebarNotice.set(describeMutateError(error));
        });
      }),
    ];
    return () => {
      for (const off of offs) {
        off();
      }
    };
  }, [navigate]);

  const keyed: SidebarKeyed[] = [];
  const entries: { key: string; element: React.ReactNode }[] = [];
  // The pinned section leads (`render_active_rows`'s pin split). The FLIP
  // diff's order vec carries the section's PHANTOM entries — the disclosure
  // header (28px + the open body's inset) and the divider — which hold no
  // element of their own; only the pinned ROWS carry elements so the resort
  // glide still reaches them. Collapsed, the pinned rows hold no slot at
  // all (the desktop's `if ix < pinned_count && !self.pinned_open` skip).
  if (pinnedRows.length > 0) {
    keyed.push({ key: SIDEBAR_PINNED_HEADER_KEY, height: pinnedHeaderKeyedHeight(pinnedOpen) });
  }
  for (const row of pinnedRows) {
    const key = `c:${row.chat.id}`;
    keyed.push({
      key,
      height: sidebarRowHeight(compact, showLabel, row.branch !== null, row.changeRequest !== null),
    });
    entries.push({
      key,
      element: (
        <ChatListRow
          key={row.chat.id}
          row={row}
          jumpLabel={jumpLabelFor(row.chat.id)}
          compact={compact}
          showLabel={showLabel}
          showProjectIcon={showProjectIcon}
          localDeviceId={localDeviceId}
        />
      ),
    });
  }
  if (hasPinnedDivider && pinnedOpen) {
    keyed.push({ key: SIDEBAR_PINNED_DIVIDER_KEY, height: SIDEBAR_PINNED_DIVIDER_HEIGHT });
  }
  // Custom sections sit between the pinned divider and the regular groups
  // (`render_active_rows`'s `section:` split, upstream 86249cf0): one keyed
  // entry per section (the 12px band + header + open body height), its
  // element the whole disclosure.
  for (const group of sectionGroups) {
    const key = `custom:${group.section.id}`;
    keyed.push({
      key,
      height: customSectionKeyedHeight(group.section, group.rows, compact, showLabel),
    });
    entries.push({
      key,
      element: (
        <CustomSection
          key={group.section.id}
          entry={{ profileKey: activeProfileKey ?? "", section: group.section }}
          rows={group.rows}
          renderRow={(row) => (
            <ChatListRow
              row={row}
              jumpLabel={jumpLabelFor(row.chat.id)}
              compact={compact}
              showLabel={showLabel}
              showProjectIcon={showProjectIcon}
              localDeviceId={localDeviceId}
            />
          )}
          onRowPointerDown={armTransferIn}
          draggingChatId={transferIn}
          shouldSuppressClick={suppressTransferClick}
          sessions={sessions}
        />
      ),
    });
  }
  for (const bucket of groups) {
    if (bucket.group === null) {
      for (const row of bucket.rows) {
        const key = `c:${row.chat.id}`;
        keyed.push({
          key,
          height: sidebarRowHeight(compact, showLabel, row.branch !== null, row.changeRequest !== null),
        });
        entries.push({
          key,
          element: (
            <RegularRowDragArm
              key={row.chat.id}
              chatId={row.chat.id}
              onArm={armTransferIn}
              dragged={transferIn === row.chat.id}
              shouldSuppressClick={suppressTransferClick}
            >
              <ChatListRow
                row={row}
                jumpLabel={jumpLabelFor(row.chat.id)}
                compact={compact}
                showLabel={showLabel}
                showProjectIcon={showProjectIcon}
                localDeviceId={localDeviceId}
              />
            </RegularRowDragArm>
          ),
        });
      }
      continue;
    }
    const collapseKey = `${bucket.group.kind}:${bucket.group.key}`;
    const collapsed = collapsedGroups.has(collapseKey);
    keyed.push({
      key: `g:${collapseKey}`,
      height:
        SIDEBAR_DISCLOSURE_SECTION_HEIGHT + (collapsed ? 0 : sidebarGroupBodyHeight(bucket.rows, compact, showLabel)),
    });
    entries.push({
      key: `g:${collapseKey}`,
      element: (
        <DeviceGroupSection
          key={collapseKey}
          collapseKey={collapseKey}
          label={bucket.group.label}
          rows={bucket.rows}
          collapsed={collapsed}
          compact={compact}
          showLabel={showLabel}
          showProjectIcon={showProjectIcon}
          localDeviceId={localDeviceId}
          jumpLabelFor={jumpLabelFor}
          onRowPointerDown={armTransferIn}
          draggingChatId={transferIn}
          shouldSuppressClick={suppressTransferClick}
          onToggle={() => {
            setCollapsedGroups((current) => {
              const next = new Set(current);
              if (next.has(collapseKey)) {
                next.delete(collapseKey);
              } else {
                next.add(collapseKey);
              }
              return next;
            });
          }}
        />
      ),
    });
  }

  // Hooks stay unconditional across the early returns below: an unconnected
  // first render must not register fewer hooks than the connected ones.
  const resort = useSidebarResort(keyed, pinResetEpoch);

  if (fleet.engines.length === 0) {
    return <p className="sidebar-note">Pair an engine to see its chats.</p>;
  }
  if (chats.error !== null) {
    return <p className="sidebar-note sidebar-note-error">{chats.error.message}</p>;
  }
  if (!chats.loaded) {
    return <p className="sidebar-note">Loading chats…</p>;
  }
  if (rows.length === 0) {
    return (
      <p className="sidebar-empty">
        {filter === null ? "No chats yet." : "No chats in this space."}
      </p>
    );
  }

  // The offsets/newKeys arrive one commit after the new order — re-wrap the
  // already-keyed children with their glide/fade state before paint. Only
  // element-bearing entries decorate (the FLIP order vec's phantom entries
  // — the pinned header and divider — contribute heights, not elements).
  const decorated = entries.map(({ key, element }) => {
    if (resort.newKeys.has(key)) {
      return (
        <div className="chat-row-in" key={key}>
          {element}
        </div>
      );
    }
    const dy = resort.offsets.get(key);
    if (dy !== undefined) {
      return (
        <ResortGlideBox key={key} dy={dy} epoch={resort.epoch}>
          {element}
        </ResortGlideBox>
      );
    }
    return <Fragment key={key}>{element}</Fragment>;
  });
  // The decorated list splits back into the pinned section (its own drag
  // container and disclosure), the custom sections, and the regular groups.
  const pinnedItems = decorated.slice(0, pinnedRows.length);
  const sectionItems = decorated.slice(pinnedRows.length, pinnedRows.length + sectionGroups.length);
  const regularItems = decorated.slice(pinnedRows.length + sectionGroups.length);
  const pinBuckets = (): Record<string, readonly string[]> => {
    const buckets: Record<string, readonly string[]> = {};
    for (const key of pinProfileKeys) {
      buckets[key] = sidebar.pinnedByProfile[key] ?? [];
    }
    return buckets;
  };
  const visiblePinIds = pinnedRows.map((row) => row.chat.id);
  return (
    <div className="chat-list" ref={sidebarRef}>
      {pinnedRows.length > 0 && (
        <PinnedSection
          rows={pinnedRows}
          items={pinnedItems}
          open={pinnedOpen}
          hasDivider={hasPinnedDivider}
          sectionRef={pinnedSectionRef}
          onToggle={() => {
            // The disclosure owns this movement: adopt the new order without
            // a second (FLIP) glide of it — the desktop's header click
            // clears its resort bookkeeping the same way.
            setPinResetEpoch((epoch) => epoch + 1);
            sidebarStore.setPinnedOpen(!pinnedOpen);
          }}
          onCommit={(from, to) => {
            // `commit_pinned_session_drag` over the profile buckets: reorder
            // the visible projection, settle every id back into its own
            // bucket (`commitVisiblePinReorder`). The rows are already
            // visually in place, so the FLIP diff adopts without gliding.
            sidebarStore.replacePinsByProfile(
              commitVisiblePinReorder(pinBuckets(), visiblePinIds, from, to),
            );
            setPinResetEpoch((epoch) => epoch + 1);
          }}
          onTransferOut={(chatId, pointer) => {
            // `finish_sidebar_session_transfer` from the pinned section's
            // gesture: a custom section under the release claims the drop
            // (unpin + assign in the same commit); anywhere else is the
            // Regular arm — only the pin membership changes, and section
            // membership clears with it, so the FLIP resort glide carries
            // the row to its live activity position.
            const sectionHit = dropSectionAt(pointer);
            const { sections: live, profileKey } = sectionsRef.current;
            if (
              sectionHit !== null &&
              profileKey !== null &&
              live.some((section) => section.id === sectionHit)
            ) {
              sidebarStore.replacePinsByProfile(
                commitSessionDrop(pinBuckets(), visiblePinIds, chatId, { kind: "regular" }),
              );
              sidebarStore.assignSidebarSection(profileKey, chatId, sectionHit);
              return;
            }
            sidebarStore.replacePinsByProfile(
              commitSessionDrop(pinBuckets(), visiblePinIds, chatId, { kind: "regular" }),
            );
            sidebarStore.assignSidebarSection(sectionsRef.current.profileKey, chatId, null);
          }}
        />
      )}
      {sectionItems}
      {regularItems}
      {pinnedRows.length > 0 &&
        regularRows.length === 0 &&
        sectionGroups.length === 0 &&
        transferIn !== null && <div className="sidebar-drop-unpin">Drop here to unpin</div>}
      <CreateSectionDialog profileKey={activeProfileKey} />
    </div>
  );
}

/**
 * A regular row's drag arm: the pointer-press starts a sidebar session
 * transfer (6851fc34). The wrapper stays layout-neutral (a plain div) — the
 * parent's gesture runs at the window level, and the row's click is
 * suppressed after a completed drag.
 */
function RegularRowDragArm({
  chatId,
  onArm,
  dragged,
  shouldSuppressClick,
  children,
}: {
  chatId: string;
  onArm: (event: React.PointerEvent, chatId: string) => void;
  dragged: boolean;
  shouldSuppressClick: () => boolean;
  children: React.ReactNode;
}) {
  return (
    <div
      className="regular-row"
      data-sidebar-dragging={dragged ? "1" : undefined}
      onPointerDown={(event) => onArm(event, chatId)}
      // The row's anchor is natively draggable; a press that moves must
      // stay OUR transfer gesture (see sidebar-drag-events.ts).
      onDragStart={preventNativeSidebarRowDrag}
      onClickCapture={(event) => {
        if (shouldSuppressClick()) {
          event.preventDefault();
          event.stopPropagation();
        }
      }}
    >
      {children}
    </div>
  );
}

/** A keyed wrapper that glides a displaced child (row or whole section). */
function ResortGlideBox({
  dy,
  epoch,
  children,
}: {
  dy: number;
  epoch: number;
  children: React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  useResortGlide(ref, dy, epoch);
  return (
    <div ref={ref} className="chat-row-resort">
      {children}
    </div>
  );
}

/** `spaces.rs::render_active_rows`' body height: inset + rows + gaps. */
function sidebarGroupBodyHeight(
  rows: readonly ChatRow[],
  compact: boolean,
  showLabel: boolean,
): number {
  let total = SIDEBAR_DISCLOSURE_BODY_INSET;
  for (const row of rows) {
    total += sidebarRowHeight(compact, showLabel, row.branch !== null, row.changeRequest !== null);
  }
  total += SIDEBAR_LIST_GAP * Math.max(rows.length - 1, 0);
  return total;
}

/**
 * One ByDevice/ByProject disclosure section (`spaces.rs` group arm): the
 * shared header (label + hairline + chevron), the tweened body, and the 12px
 * section band above it. Its keyed height for the FLIP diff is the
 * collapsed/open pair the parent computed.
 */
function DeviceGroupSection({
  collapseKey,
  label,
  rows,
  collapsed,
  compact,
  showLabel,
  showProjectIcon,
  localDeviceId,
  jumpLabelFor,
  onRowPointerDown,
  draggingChatId,
  shouldSuppressClick,
  onToggle,
}: {
  collapseKey: string;
  label: string;
  rows: readonly ChatRow[];
  collapsed: boolean;
  compact: boolean;
  showLabel: boolean;
  showProjectIcon: boolean;
  localDeviceId: string | null;
  jumpLabelFor: (chatId: string) => string | null;
  /** The parent's transfer-in gesture arm (one per regular row). */
  onRowPointerDown: (event: React.PointerEvent, chatId: string) => void;
  draggingChatId: string | null;
  shouldSuppressClick: () => boolean;
  onToggle: () => void;
}) {
  const bodyHeight = sidebarGroupBodyHeight(rows, compact, showLabel);
  const { bodyRef, chevronRef, toggle } = useSidebarDisclosure(
    `group:${collapseKey}`,
    !collapsed,
    bodyHeight,
  );
  return (
    <section className="sidebar-group" id={`sidebar-group-${collapseKey}`}>
      <SidebarDisclosureHeader
        label={collapsed ? `${label} (${rows.length})` : label}
        open={!collapsed}
        chevronRef={chevronRef}
        onToggle={() => {
          // The motion begins on the CURRENT height before the flip — a
          // rapid double-click reverses from mid-flight, not from rest.
          toggle();
          onToggle();
        }}
      />
      <SidebarDisclosureBody bodyRef={bodyRef}>
        <div className="sidebar-group-rows">
          {rows.map((row) => (
            <RegularRowDragArm
              key={row.chat.id}
              chatId={row.chat.id}
              onArm={onRowPointerDown}
              dragged={draggingChatId === row.chat.id}
              shouldSuppressClick={shouldSuppressClick}
            >
              <ChatListRow
                row={row}
                jumpLabel={jumpLabelFor(row.chat.id)}
                compact={compact}
                showLabel={showLabel}
                showProjectIcon={showProjectIcon}
                localDeviceId={localDeviceId}
              />
            </RegularRowDragArm>
          ))}
        </div>
      </SidebarDisclosureBody>
    </section>
  );
}

/**
 * One sidebar chat card — the desktop's `shell.rs::render_chat_row`, line for
 * line, plus upstream 78e9e6ae→378a1945's compact mode:
 *
 * 1. `project @ device` at 11px/14px in the muted subline tone, with the
 *    status corner right-aligned. The corner is activity, not position: a
 *    small colored word beside a glyph — Working animates the pixel spinner,
 *    Done wears a check, the rest use a 6px dot — and Idle rows show the
 *    relative time instead. A jump hint (the slot's `badgeCombo`, ticket 12)
 *    takes the corner outright above both.
 * 2. The harness brand mark (13px) beside the title at 13px/17px, with the
 *    project mark leading (78e9e6ae's project icons): the space's probed
 *    repository artwork when it resolves, else the curated-palette
 *    monogram — see `project-monogram.tsx` and `lib/project-icons.ts`.
 * 3. Structural, not reserved: branch and change-request badge, omitted
 *    entirely when the chat has neither — the invisible spring keeps the
 *    badge pinned right without moving anything when absent.
 *
 * Compact rows (78e9e6ae + deaf2c4e) drop lines 1 and 3: the status glyph
 * leads the single line, the monogram and harness mark follow, the PR badge
 * rides beside the title, elapsed time sits in a fixed 30px right slot, and
 * the corner keeps the remote glyph (Earth, with the owning device's
 * tooltip — 4f9a5fd5/378a1945) that the Archive pill replaces on hover.
 *
 * Hovering the ROW (not the corner — corner-only tested as undiscoverable)
 * swaps the corner for the Archive pill, whose padding bleeds into the row's
 * so its text right-aligns exactly where the status word sat: the swap moves
 * pixels around the label, not the label itself. Right mouse-down opens the
 * chat context menu at the pointer, exactly as the desktop does.
 */
function ChatListRow({
  row,
  jumpLabel = null,
  compact = false,
  showLabel = true,
  showProjectIcon = true,
  localDeviceId = null,
}: {
  row: ChatRow;
  jumpLabel?: string | null;
  compact?: boolean;
  showLabel?: boolean;
  showProjectIcon?: boolean;
  localDeviceId?: string | null;
}) {
  // A row can live on ANY paired engine — resolve its owning session off
  // the scoped chat id so archive/menu mutations route to the right one.
  const sessions = useEngineSessions();
  const owning = owningSession(sessions, row.chat.id);
  const [hovered, setHovered] = useState(false);
  const word = statusWord(row.status);
  const archived = row.chat.archived;
  const brand = row.harness === null ? null : harnessBrandIcon(row.harness);
  const { menu, element } = useChatMenu(row.chat);
  const remote = localDeviceId !== null && row.deviceId !== localDeviceId;
  const device = row.deviceName ?? "Unknown device";
  const projectName = row.projectPath === null ? "Home" : row.project;
  const projectSeed = row.projectPath ?? "home";

  function toggleArchive(event: React.MouseEvent): void {
    // The row's own click is the selector; only the corner archives.
    event.preventDefault();
    event.stopPropagation();
    if (owning === null) {
      sidebarNotice.set("Engine not connected");
      return;
    }
    setChatArchived(owning.client, row.chat.id, !archived).catch((error: unknown) => {
      sidebarNotice.set(describeMutateError(error));
    });
  }

  const monogram =
    showProjectIcon ? (
      <ProjectIconMark
        name={projectName}
        seed={projectSeed}
        device={device}
        spaceId={row.chat.spaceId ?? null}
      />
    ) : null;

  // The corner's compact body: the remote glyph at rest (the Archive pill
  // takes the slot on hover — deaf2c4e's compact corner); nothing for a
  // local row until hover.
  const compactCornerBody = hovered ? (
    <button
      type="button"
      className="chat-row-archive"
      aria-label={archived ? "Unarchive chat" : "Archive chat"}
      onClick={toggleArchive}
    >
      <Icon name={archived ? "archiveUpMinimalistic" : "archiveMinimalistic"} size={11} />
      {archived ? "Unarchive" : "Archive"}
    </button>
  ) : remote ? (
    <Tooltip label={device} delay={TOOLTIP_VIEW_OPTIONS_MS} trigger={<Icon name="global" size={13} className="chat-row-remote" />} />
  ) : null;

  // `menu` wraps the row so a right-click opens the chat context menu at
  // the pointer (`useChatMenu`'s ContextMenu.Trigger adopts this div). The
  // dialogs live outside it — they portal anyway, and their state must
  // outlive the menu's unmount.
  const rowElement = menu(
    <div
      className={compact ? "chat-row-item chat-row-compact" : "chat-row-item"}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <Link
        to="/chat/$chatId"
        params={{ chatId: row.chat.id }}
        className="chat-row"
        data-status={row.status}
        data-compact={compact ? "1" : undefined}
        activeProps={{ className: "chat-row chat-row-active" }}
      >
        {!compact && (
          <div className="chat-row-line">
            <SidebarFadedLabel className="chat-row-folder" fill>
              {row.folder}
            </SidebarFadedLabel>
            <span className="chat-row-corner">
              {jumpLabel !== null ? (
                <span className="chat-row-jump mono">{jumpLabel}</span>
              ) : hovered ? (
                <button
                  type="button"
                  className="chat-row-archive"
                  aria-label={archived ? "Unarchive chat" : "Archive chat"}
                  onClick={toggleArchive}
                >
                  <Icon name={archived ? "archiveUpMinimalistic" : "archiveMinimalistic"} size={11} />
                  {archived ? "Unarchive" : "Archive"}
                </button>
              ) : word === null ? (
                <span className="chat-row-time">{row.timeAgo}</span>
              ) : (
                <span className={`chat-row-status status-${row.status}`}>
                  <StatusGlyph status={row.status} />
                  {word}
                </span>
              )}
            </span>
          </div>
        )}
        <div className="chat-row-title-line">
          {compact && (
            <span className={`chat-row-status-compact status-${row.status}`} aria-label={word ?? "Idle"}>
              <StatusGlyph status={row.status} />
            </span>
          )}
          {monogram}
          {brand !== null && (
            <Icon
              name={brand.name}
              size={SIDEBAR_ACTIVE_HARNESS_ICON_SIZE}
              className="chat-row-brand"
              style={brand.tint === null ? undefined : { color: brand.tint }}
            />
          )}
          <SidebarFadedLabel className="chat-row-title" fill>
            {row.chat.title ?? "New session"}
          </SidebarFadedLabel>
          {/* Detailed rows carry the remote Earth glyph only while the
              project label is hidden (78e9e6ae's slot rule). */}
          {!compact && !showLabel && remote && (
            <Tooltip label={device} delay={TOOLTIP_VIEW_OPTIONS_MS} trigger={<Icon name="global" size={13} className="chat-row-remote" />} />
          )}
          {compact && compactCornerBody}
          {compact && row.changeRequest !== null && (
            <span
              className="chat-row-pr"
              onClick={(event) => {
                // The badge's own anchor owns the click; the row's Link
                // must not also navigate.
                event.stopPropagation();
              }}
            >
              <ChangeRequestBadge summary={row.changeRequest} size="sidebar" />
            </span>
          )}
          {compact && (
            <span className="chat-row-time chat-row-time-compact">
              {jumpLabel !== null ? jumpLabel : row.timeAgo}
            </span>
          )}
        </div>
        {!compact && (row.branch !== null || row.changeRequest !== null) && (
          <div className="chat-row-meta">
            {row.branch !== null && (
              <>
                <Icon name="gitBranch" size={11} />
                <SidebarFadedLabel className="chat-row-branch">{row.branch}</SidebarFadedLabel>
              </>
            )}
            <span className="chat-row-meta-spring" />
            {row.changeRequest !== null && (
              <span
                className="chat-row-pr"
                onClick={(event) => {
                  // The badge's own anchor owns the click; the row's Link
                  // must not also navigate.
                  event.stopPropagation();
                }}
              >
                <ChangeRequestBadge summary={row.changeRequest} size="sidebar" />
              </span>
            )}
          </div>
        )}
      </Link>
    </div>,
  );
  return (
    <>
      {rowElement}
      {element}
    </>
  );
}

/**
 * The session that owns a (scoped) chat id — the row-level router for
 * sidebar mutations. Unscoped ids resolve to null (nothing to route to).
 */
function owningSession(
  sessions: ReadonlyMap<string, EngineSession>,
  chatId: string,
): EngineSession | null {
  try {
    const engine = parseScopedId(chatId).engine;
    return engine === null ? null : sessions.get(engine) ?? null;
  } catch {
    return null;
  }
}

/**
 * The corner's glyph slot (`render_chat_row`): Done wears the check, Working
 * the animated pixel glyph, everything else a compact 6px dot.
 */
function StatusGlyph({ status }: { status: ChatRow["status"] }) {
  if (status === "completed") {
    return <Icon name="check" size={11} />;
  }
  if (status === "working") {
    return <GlyphSpinner size={11} />;
  }
  return <span className={`dot dot-${status}`} />;
}
