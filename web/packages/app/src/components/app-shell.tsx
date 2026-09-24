import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, DragEvent } from "react";
import { motion } from "@zeron/theme";
import { Link, Outlet, useNavigate, useRouter, useRouterState } from "@tanstack/react-router";
import { Icon } from "@zeron/icons";
import { useFleet, useFleetRegistry } from "../state/fleet";
// Ticket 11's engine-side sidebar state bridge: importing the module wires
// the registry-driven sync (pins + custom sections mirror engine-side;
// `localStorage` stays the offline cache).
import "../state/sidebar-state-sync";
import { useEngineSession } from "../state/session-provider";
import { useEngineStatus } from "../state/hooks";
import {
  applyKeymap,
  emitShortcut,
  isMacPlatform,
  matchKeybinding,
  onShortcut,
} from "../state/shortcuts";
import { overlayOwnsKeyboard, useKeymap, keystrokesIntercepted } from "../state/keymap";
import { toggleAddSpace } from "../state/add-space";
import { toggleCommandPalette } from "../state/command-palette";
import { installJumpHintModifierListeners } from "../state/jump-hints";
import { useChrome } from "../state/chrome";
import {
  ESCAPE_PRIORITY,
  installEscapeLadder,
  registerEscapeSurface,
  resolveShellEscape,
} from "../state/escape";
import {
  navEntryForPath,
  navEntryPath,
  navHistory,
  sameNavEntry,
  useNavHistory,
  type NavEntry,
} from "../state/nav-history";
import {
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  TITLEBAR_CONTENT_START,
  PANEL_TOGGLE_SLOTS,
  conversationWidth,
  rightPaneMaxWidth,
  sidebarLayout,
  sidebarTarget,
  titlebarAvailableTitlebarWidth,
  titlebarNewSessionAlpha,
  titlebarPaneBandWidth,
  titlebarRowLeft,
  useSidebarLayout,
  useViewportWidth,
  PHONE_MAX_WIDTH,
} from "../state/layout";
import { useIsPhone } from "../state/media";
import { effectiveIndicator } from "../lib/view";
import { sendInterrupt } from "../lib/composer-actions";
import { sidebarNotice } from "../state/notice";
import { uiSettings, FILES_PANEL_MAX, FILES_PANEL_MIN, FILES_PANEL_DEFAULT } from "../state/ui-settings";
import {
  RIGHT_PANE_MIN,
  panelKey,
  resolvePaneWidth,
  resolvedActive,
  rightPaneStore,
  useRightPane,
} from "../state/right-pane";
import { useSidebar } from "../state/sidebar";
import { useNewThreadBackground } from "../state/appearance";
import { SidebarBody } from "./sidebar-body";
import { SettingsNavBody } from "./settings-nav";
import { PaneSeam } from "./pane-seam";
import { RightPane, usePaneGlide } from "./right-pane";
import { FilesPaneColumn } from "./files/files-pane-column";
import { RightTabStrip } from "./right-tab-strip";
import { useConnectionState } from "./connection-state";
import { Titlebar, islandTarget } from "./titlebar";
import { ProjectActionsControl } from "./project-actions-control";
import { TerminalProvider, drawerTerminalStore } from "../terminal/store";

/**
 * The app shell — the desktop's `shell.rs` chrome.
 *
 * Structure follows the desktop's root row (`shell.rs:8048`): one flex row
 * spanning the FULL window height, holding three columns —
 *
 *     [sidebar] [conversation] [right pane]
 *
 * — with the titlebar as an absolute glass overlay ON TOP of that row rather
 * than a band above it. That is why each column pads itself down by the
 * titlebar height instead of the bar pushing them: surfaces and seam hairlines
 * run the whole height, behind the bar, and the transcript can scroll under it.
 *
 * Both outer columns are resizable and both collapse. Their widths come from
 * `../state/layout`, which ports the desktop's three width functions verbatim
 * so a window divides the same way in both clients — including the rule that a
 * manual drag may never take the conversation below 300px while takeover may
 * consume it entirely.
 *
 * Collapse CLIPS rather than squeezes: the sidebar's outer column animates its
 * width while the content inside stays pinned at the dragged width, so rows
 * slide out of a shrinking window instead of reflowing through it
 * (`render_sidebar` caches its inner pane at a fixed width for exactly this).
 *
 * At phone widths the sidebar leaves the flow and becomes a drawer over the
 * content, the titlebar keeps its cluster, and the content owns the viewport.
 *
 * The global keyboard is the desktop's whole keymap (ticket 12): one
 * capture-phase `window` listener consults the resolved keystroke → action
 * table (capture, because a matched binding must outrank any raw key
 * handler — a focused xterm otherwise eats Mod+J as a linefeed), applies the
 * desktop's per-route guards, and fans the action out over the shortcut bus.
 * Escape stays ticket 06's two-phase ladder; jump-hint modifiers are tracked
 * by `state/jump-hints.ts`.
 */
/** `motion::RESIZE` — the curve the columns glide on. */
const TAKEOVER_GLIDE_MS =
  motion.specs.find((spec) => spec.name === "resize")?.durationMs ?? 200;

export function AppShell() {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const fleet = useFleet();
  const session = useEngineSession();
  const status = useEngineStatus(session);
  const state = useConnectionState(status);
  const registry = useFleetRegistry();
  const navigate = useNavigate();
  const router = useRouter();
  const chrome = useChrome();
  const sidebar = useSidebarLayout();
  const viewport = useViewportWidth();
  const sidebarWidth = sidebarTarget(sidebar);
  // The phone sidebar is a fixed overlay out of flow (app.css's phone block),
  // so the desktop width functions must not see its dragged width: with 0
  // the title row starts at 136/104 instead of the dragged 320 (M3 — the
  // identity lands next to the cluster), and the pane keeps its 75px floor
  // instead of collapsing to 0 (M2's geometry inputs; its phone FORM is
  // ticket 52's). One branch, shared by `rowLeft`, `paneOpenWidth` and the
  // band the two feed.
  const phone = useIsPhone();
  const sidebarForGeometry = phone ? 0 : sidebarWidth;
  // The pane's owning chat, straight off the router. Deliberately NOT via the
  // chrome store: that is published from an effect and cleared on every dep
  // change, so the shell saw "no pane" for one commit on each toggle and tore
  // the column down mid-glide. The router's state is synchronous with the
  // navigation that actually changes which chat is on screen.
  const paneChatId = useRouterState({ select: (s) => chatIdOf(s.location.pathname) });
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const nav = useNavHistory();
  // `apply_nav`'s gate: the navigation a Back/Forward click performs must not
  // itself push, or the cursor would be dragged straight back to where it
  // started. We record the entry we asked for and let the route effect below
  // recognise — and swallow — exactly that one arrival.
  const appliedNavRef = useRef<NavEntry | null>(null);

  // The route-driven half of the nav model: every navigation the USER makes
  // (a chat row, a settings item, a link) is a visit. Paths outside the model
  // (`/pair`, `/files`) leave the stack alone.
  useEffect(() => {
    const entry = navEntryForPath(pathname);
    const applied = appliedNavRef.current;
    appliedNavRef.current = null;
    if (entry === null) {
      return;
    }
    if (applied !== null && sameNavEntry(applied, entry)) {
      return;
    }
    navHistory.visit(entry);
  }, [pathname]);

  // ── M7(b): the phone drawer closes on navigate (ticket 55) ─────────────
  // Every entry surface into the drawer's content navigates by changing the
  // pathname — chat rows and archived rows are router Links, settings rows
  // are Links, the account menu's Settings item calls `navigate` — so ONE
  // effect keyed on the pathname covers them all, future surfaces included.
  // At phone width the drawer is a transient overlay over the destination,
  // so it closes; at desktop width the sidebar is a persistent column that
  // never closes on selection, and the matchMedia guard — the toggle's own
  // breakpoint (`onToggleSidebar` above), read at navigation time inside the
  // body, not at render time — makes this a no-op there. Re-notifying the
  // SAME pathname does not refire the effect (deps compare equal), and the
  // ref-recorded `pathChanged` keeps even a hypothetical refire from reading
  // as a navigation: the drawer stays open on a same-path re-tap (the
  // recorded known limitation — the backdrop, the cluster toggle, and Escape
  // still close it). `sidebarOpen` is deliberately NOT a dep: a dep would
  // fire this on drawer-open changes alone and slam the drawer shut the
  // frame the toggle opens it.
  const previousPathname = useRef(pathname);
  useEffect(() => {
    const pathChanged = previousPathname.current !== pathname;
    previousPathname.current = pathname;
    if (
      shouldCloseDrawer(
        pathChanged,
        sidebarOpen,
        window.matchMedia(`(max-width: ${PHONE_MAX_WIDTH}px)`).matches,
      )
    ) {
      setSidebarOpen(false);
    }
  }, [pathname]);

  const onNavWalk = useCallback(
    (entry: NavEntry | null) => {
      if (entry === null) {
        return;
      }
      appliedNavRef.current = entry;
      // Raw path rather than `navigate({to})`: a `NavEntry` names a route
      // computed at runtime (any chat id, any settings section), which the
      // typed router's literal `to` union cannot express.
      router.history.push(navEntryPath(entry));
    },
    [router],
  );
  // The pane's own per-chat state, keyed the desktop's way (`panel_key`):
  // the chat id on a chat route, `space-canvas:{space}` on the blank canvas
  // so a canvas toggle can never read as global state across unrelated
  // spaces. The pane never mounts without a chat, so the canvas key only
  // isolates the stored flags.
  const sidebarState = useSidebar();
  const canvasSpace = sidebarState.spaceFilter ?? sidebarState.lastSpaceId ?? "";
  const pane = useRightPane(panelKey(paneChatId, canvasSpace));
  // What the pane resolves to WHEN OPEN, and what it lays out at right now.
  // Keeping the two apart is what lets the column animate between them: the
  // content keeps the open width while the column itself glides to zero.
  const paneOpenWidth = resolvePaneWidth({ ...pane, open: true }, viewport, sidebarForGeometry);
  const paneWidth = paneChatId !== null && pane.open ? paneOpenWidth : 0;

  const onNewChat = useCallback(() => {
    if (fleet.engines.length === 0) {
      // No engine paired: keyboard shortcut is the equivalent of the welcome
      // "Pair an engine" button.
      void navigate({ to: "/pair" });
      return;
    }
    emitShortcut("new-chat");
  }, [fleet.engines.length, navigate]);

  // One control, two meanings — the desktop's `toggle_sidebar` collapses the
  // column; at phone widths the same button opens the drawer over the content.
  // The breakpoint is the shared media hook's (`state/media.ts`, ticket 49):
  // the same `(max-width: 768px)` query the stylesheet keys, so the click can
  // never land in the dead band the old one-shot matchMedia-per-click risked —
  // asking at a wider query than CSS drew left a window where the click
  // flipped the drawer flag while CSS still drew the column. The value is
  // read at render and captured in the callback, so a resize re-renders and
  // the captured branch follows.
  const isPhone = useIsPhone();
  const onToggleSidebar = useCallback(() => {
    if (isPhone) {
      setSidebarOpen((current) => !current);
      return;
    }
    sidebarLayout.toggleCollapsed();
  }, [isPhone]);

  const onCloseDrawer = useCallback(() => {
    setSidebarOpen(false);
  }, []);

  // The global keymap dispatch (`apply_keymap` + the action guards,
  // shell.rs:7768-7839): one capture-phase window listener consulting the
  // resolved keystroke → action table. Capture, not bubble: gpui runs a
  // matched binding BEFORE any raw `on_key_down` listener, and the one web
  // surface where that ordering is load-bearing is a focused terminal —
  // xterm stops propagation on the keys it handles, so a bubble-phase
  // listener would never see Mod+J (the old chat-page listener documented
  // the same trap).
  const keymap = useKeymap();
  const isMac = isMacPlatform();
  const table = useMemo(() => applyKeymap(keymap, isMac), [keymap, isMac]);
  const route = pathname.startsWith("/settings") ? "settings" : "chat";
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      // The shortcuts recorder's `cx.intercept_keystrokes` equivalent: while
      // it owns the keyboard, a matched binding is recorded/refused by the
      // recorder instead of running its action (shortcuts.rs:127-140).
      if (keystrokesIntercepted()) {
        return;
      }
      const binding = matchKeybinding(event, table);
      if (binding === null) {
        return;
      }
      // The desktop's inputs let unbound keys through and a matched binding
      // outranks them; the distinction the web can draw is modifiers — a
      // bare-key binding would swallow typing, so it alone is guarded while
      // an editable element holds focus.
      if (binding.bare && isEditableTarget(event.target)) {
        return;
      }
      // A matched binding is consumed even when its guards no-op the action:
      // letting the browser default run (Mod+S's save dialog, Mod+R's
      // reload) is not the desktop's "nothing".
      event.preventDefault();
      switch (binding.event) {
        case "new-chat":
          // Always works — `open_new_session` routes back to chat itself,
          // so Settings is not a dead spot.
          onNewChat();
          return;
        case "toggle-sidebar":
          emitShortcut("toggle-sidebar");
          return;
        case "save-file":
          // `SaveFile`'s scope: a chat route with the pane open on a
          // Files/File surface (shell.rs:7781-7784's surface guard). Read
          // the live pane state so a tab switch mid-listener still guards.
          if (route === "chat" && paneChatId !== null && pane.open) {
            const active = resolvedActive(rightPaneStore.stateFor(paneChatId));
            if (active.kind === "file") {
              emitShortcut("save-file");
            }
          }
          return;
        case "toggle-changes":
          if (route === "chat" && paneChatId !== null) {
            emitShortcut("toggle-changes");
          }
          return;
        case "toggle-terminal":
          if (route === "chat") {
            emitShortcut("toggle-terminal");
          }
          return;
        case "next-session":
        case "prev-session":
        case "archive-session":
          // Chat-scoped, and quiet under an overlay that owns the keyboard
          // (the add-space palette or a composer picker): an unguarded jump
          // would switch sessions UNDER the open popover.
          if (route === "chat" && !overlayOwnsKeyboard()) {
            emitShortcut(binding.event);
          }
          return;
        case "open-model-picker":
          // `OpenModelPicker` (upstream faac7432): only on the chat route,
          // and quiet under an overlay that owns the keyboard.
          if (route === "chat" && !overlayOwnsKeyboard()) {
            emitShortcut(binding.event);
          }
          return;
        case "jump-session":
          // Ticket 10 gives the composer's model picker first refusal on
          // the slot; until then the jump routes straight to the row, from
          // any route.
          if (!overlayOwnsKeyboard()) {
            emitShortcut("jump-session", { slot: binding.slot });
          }
          return;
        case "add-space-palette":
        case "command-palette":
        case "open-settings":
          emitShortcut(binding.event);
          return;
        default:
          return;
      }
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [table, route, pathname, paneChatId, pane.open, onNewChat]);

  // The shell-owned actions subscribe to the bus the same way the
  // scattered widget listeners do — the keyboard layer stays free of
  // component imports.
  useEffect(() => onShortcut("toggle-sidebar", () => onToggleSidebar()), [onToggleSidebar]);
  useEffect(
    () =>
      onShortcut("toggle-changes", () => {
        if (paneChatId === null) {
          return;
        }
        rightPaneStore.toggle(paneChatId);
        // `toggle_right_pane`'s focus return: closing hands focus back to a
        // mounted target so the next shortcut can reopen it
        // (shell.rs:7798-7807). Ticket 06/07's column has no focus handle of
        // its own, so the composer's textarea is the mounted target.
        if (!rightPaneStore.stateFor(paneChatId).open) {
          document.querySelector<HTMLTextAreaElement>(".composer-input")?.focus();
        }
      }),
    [paneChatId],
  );
  useEffect(
    () =>
      onShortcut("open-settings", () => {
        void navigate({ to: "/settings" });
      }),
    [navigate],
  );
  // Mod+K toggles the add-space palette (the New project binding,
  // `ShortcutId::NewProject` — mod-shift-n — resolves through the keymap
  // table to the same event; the fixed binding was mod-k until ticket 16
  // moved that chord to the command palette).
  useEffect(() => onShortcut("add-space-palette", toggleAddSpace), []);
  // Mod+K toggles the command palette (the fixed `ToggleCommandPalette`
  // binding, shell.rs — the desktop's mod-k rebind, ticket 16).
  useEffect(() => onShortcut("command-palette", toggleCommandPalette), []);

  // The modifier-hold lifecycle for the sidebar's jump chips (§2.4) — one
  // install, capture-phase observers that never preventDefault.
  useEffect(() => installJumpHintModifierListeners(), []);

  // The shell's Escape model — one capture-phase ladder (installed once) plus
  // the bubble-phase interrupt (`on_key_down` → `resolve_shell_escape`). The
  // phone sidebar registers as a ladder surface; the popovers and dialogs
  // that still close on their own Escape listeners are theirs to migrate
  // onto the ladder in their tickets.
  useEffect(() => installEscapeLadder(), []);
  useEffect(() => {
    if (!sidebarOpen) {
      return;
    }
    return registerEscapeSurface(ESCAPE_PRIORITY.webDrawer, () => {
      onCloseDrawer();
      return true;
    });
  }, [sidebarOpen, onCloseDrawer]);

  // Interrupts already in flight for a chat — the desktop's
  // `composer.is_interrupting(chat_id)`. A second Escape while the Stop
  // request is still on the wire resolves to Ignored instead of stacking.
  const interruptingRef = useRef<ReadonlySet<string>>(new Set());
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      // Case-insensitive: the DOM spells the key `"Escape"`; a strict
      // lowercase compare let real keystrokes slip past the interrupt.
      if (event.key.toLowerCase() !== "escape") {
        return;
      }
      const route = pathname.startsWith("/settings") ? "settings" : "chat";
      const selectedChatId = route === "chat" ? chatIdOf(pathname) : null;
      const indicator =
        selectedChatId === null || session === null
          ? null
          : effectiveIndicator(
              session.cache.getSnapshot().statuses.rows.find((row) => row.chatId === selectedChatId),
              Date.now(),
            );
      const outcome = resolveShellEscape({
        key: event.key,
        blockingOverlay: false,
        escapeStopsActiveAgent: uiSettings.getSnapshot().escapeStopsActiveAgent,
        route,
        interrupting: selectedChatId !== null && interruptingRef.current.has(selectedChatId),
        indicator: indicator === "none" ? null : indicator,
        selectedChatId,
      });
      if (outcome.kind !== "interruptChat" || session === null) {
        return;
      }
      event.preventDefault();
      const chatId = outcome.chatId;
      const inFlight = new Set(interruptingRef.current);
      inFlight.add(chatId);
      interruptingRef.current = inFlight;
      void sendInterrupt(session.client, chatId)
        .catch((error) => {
          sidebarNotice.set(
            `Could not interrupt: ${error instanceof Error ? error.message : String(error)}`,
          );
        })
        .finally(() => {
          const next = new Set(interruptingRef.current);
          next.delete(chatId);
          interruptingRef.current = next;
        });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [pathname, session]);

  const paired = fleet.engines.length > 0;
  const hasPane = paired && paneChatId !== null;
  const takeover = hasPane && pane.open && pane.expanded;
  // The phone pane drawer rides the Escape ladder's same rung as the phone
  // sidebar drawer (12, `webDrawer` — the web-only phone chrome): Escape
  // closes it exactly as it closes the sidebar drawer, and two drawers open
  // at once peel one per press in registration order. Desktop-pane-open
  // Escape is untouched — the rung is phone-gated.
  useEffect(() => {
    if (!phone || !hasPane || !pane.open) {
      return;
    }
    return registerEscapeSurface(ESCAPE_PRIORITY.webDrawer, () => {
      if (paneChatId !== null) {
        rightPaneStore.close(paneChatId);
      }
      return true;
    });
  }, [phone, hasPane, pane.open, paneChatId]);
  // One glide, shared: `toggle_right_pane_expand` tweens the pane AND the
  // conversation together, so both columns have to read the same clock.
  const glide = usePaneGlide(hasPane && pane.open, takeover, hasPane ? paneOpenWidth : 0);
  // `main_takeover_tween` + `stable_panel_content_width`: across a takeover the
  // conversation is laid out at the LARGER of its two widths and clipped, so
  // the transcript is revealed or covered rather than re-wrapping through
  // every intermediate width.
  const conversationNow = conversationWidth(viewport, sidebarWidth, paneWidth);
  const conversationStable = useTakeoverStableWidth(takeover, conversationNow);
  // `titlebar_plus_alpha`: the `+` (and its 32px row-left slot) exists only
  // on the chat route with a chat selected — never on the blank canvas,
  // never in Settings.
  const isChatRoute = navEntryForPath(pathname)?.kind === "chat";
  const plusAlpha = titlebarNewSessionAlpha(isChatRoute, paired && paneChatId !== null);
  // ── The titlebar island's gate (ticket 34) ─────────────────────────────
  // The desktop's `island_target` (shell.rs:3990-4001) keyed off the
  // RESOLVED background — ticket 48's semantics (installed-else-default),
  // never the raw setting: `useNewThreadBackground` resolves through
  // `resolveNewThreadBackground`, so a stored entry that no longer decodes
  // falls back to the bundled default and the island still shows; only a
  // resolution failure (null url) hides it. Deliberately not through
  // `state/chrome.ts` — route-published effect state is what tore columns
  // down before (ticket 06's lesson). The resolution lands a frame after
  // mount, so the island's first appearance on a reload rides its 200ms
  // tween; the tween itself mounts settled.
  const newThreadBackground = useNewThreadBackground();
  const islandTargetValue = islandTarget({
    isChatRoute,
    hasSelectedChat: paneChatId !== null,
    sidebarCollapsed: sidebar.collapsed,
    backgroundResolves: newThreadBackground.url !== null,
  });
  // The settings route's bar is a BARE strip (`render_title_bar`,
  // shell.rs:3898-3909): no identity, no `+`, no trailing group, and its
  // left inset is flat `title_bar_content_start()` — it does not track the
  // sidebar, because the settings column is not the conversation column.
  const rowLeft = isChatRoute
    ? titlebarRowLeft({
        sidebar: sidebarForGeometry,
        showsNewSession: plusAlpha > 0,
        takeover,
      })
    : TITLEBAR_CONTENT_START;
  // ── The project-Actions control (tabs.rs:316-320) ─────────────────────
  // The desktop's `!takeover && !on_canvas` gate: the chat route with a
  // selected chat. `available_titlebar_width` (b1484015) measures the room
  // the control may claim after the trailing strip — the trailing group is
  // the pane's band plus the two fixed 28px toggle anchors
  // (PANEL_TOGGLE_SLOTS), or just the pair while the pane is shut.
  const paneBandWidth = titlebarPaneBandWidth({ viewport, paneWidth, rowLeft, takeover });
  const showActionsControl = isChatRoute && paneChatId !== null && !takeover;
  const actionsTitlebarWidth = titlebarAvailableTitlebarWidth({
    viewport,
    rowLeft,
    trailingWidth: paneWidth > 0 ? paneBandWidth + PANEL_TOGGLE_SLOTS : PANEL_TOGGLE_SLOTS,
  });
  const shellClass = [
    "shell",
    sidebar.collapsed ? "shell-sidebar-collapsed" : "",
    sidebarOpen ? "shell-sidebar-open" : "",
    takeover ? "shell-pane-takeover" : "",
    // Clip the conversation for the whole glide, not just while takeover is
    // on: dropping the clip on the first frame of a collapse spills a
    // full-width transcript over the pane for 200ms.
    conversationStable !== null ? "shell-pane-gliding" : "",
  ]
    .filter((part) => part.length > 0)
    .join(" ");

  // ── The drop veil's drag bookkeeping ───────────────────────────────────
  // dragenter/dragleave fire per ELEMENT boundary, so a counter (not a
  // boolean) tracks whether the pointer is still inside the column; leaving
  // the window entirely (relatedTarget null) resets it in one step.
  const [dropDepth, setDropDepth] = useState(0);
  const fileDrag = (event: DragEvent): boolean => event.dataTransfer.types.includes("Files");
  const onDragEnter = (event: DragEvent): void => {
    if (!fileDrag(event)) {
      return;
    }
    event.preventDefault();
    setDropDepth((depth) => depth + 1);
  };
  const onDragOver = (event: DragEvent): void => {
    if (!fileDrag(event)) {
      return;
    }
    // Needed for the drag to stay alive over this element.
    event.preventDefault();
  };
  const onDragLeave = (event: DragEvent): void => {
    if (!fileDrag(event)) {
      return;
    }
    if (event.relatedTarget === null) {
      setDropDepth(0);
      return;
    }
    setDropDepth((depth) => Math.max(0, depth - 1));
  };
  const onDrop = (event: DragEvent): void => {
    if (!fileDrag(event)) {
      return;
    }
    // Ticket 17 owns what happens to the dropped files; here the drop is
    // only swallowed so the browser does not navigate to the file.
    event.preventDefault();
    setDropDepth(0);
  };

  return (
    <div
      className={shellClass}
      style={
        {
          // Each outer column's laid-out width (zero when shut) and the width
          // its content keeps throughout the glide — see the clip note above.
          // The seams read these to stay parked on the moving edges.
          "--rb-sidebar-now": `${sidebarWidth}px`,
          "--rb-sidebar-content": `${sidebar.width}px`,
          "--rb-pane-now": `${paneWidth}px`,
          "--rb-pane-open": `${hasPane ? paneOpenWidth : 0}px`,
          // The docked explorer column's laid-out width (zero when shut).
          "--rb-files-now": `${hasPane && pane.filesOpen ? uiSettings.getSnapshot().filesPanelWidth : 0}px`,
          // The title row's left inset, which tracks the sidebar so the
          // identity sits on the conversation's own edge and glides with a
          // collapse — `render_session_title_bar`'s `row_left`.
          "--rb-titlebar-row-left": `${rowLeft}px`,
          // The header strip rides the pane's animated width, capped to the
          // room the row has left — `animated_width`. Never `auto`: that made
          // it snap to full width in takeover while the column glided.
          "--rb-pane-band": `${paneBandWidth}px`,
          // Auto outside a takeover glide, so the column is plain flex again.
          "--rb-main-stable": conversationStable === null ? "auto" : `${conversationStable}px`,
        } as CSSProperties
      }
    >
      <Titlebar
        onToggleSidebar={onToggleSidebar}
        onBack={() => onNavWalk(navHistory.back())}
        onForward={() => onNavWalk(navHistory.forward())}
        canBack={nav.canBack}
        canForward={nav.canForward}
        newSessionAlpha={plusAlpha}
        islandTarget={islandTargetValue}
        // No fallback handler: the `+` exists only where the route published
        // one (a selected chat), exactly `titlebar_plus_alpha`'s gate.
        onNewSession={chrome.onNewSession}
        identity={chrome.identity}
        // The project-Actions control (tabs.rs:316-320): the desktop's
        // `!takeover && !on_canvas` gate, sized to the titlebar room left.
        actions={
          showActionsControl && paneChatId !== null ? (
            <ProjectActionsControl chatId={paneChatId} availableTitlebarWidth={actionsTitlebarWidth} />
          ) : undefined
        }
        // Every pane control is shell-owned and synchronous with the store, so
        // the toggle, the strip and the column all move on the same frame.
        onTogglePane={hasPane ? () => rightPaneStore.toggle(paneChatId) : null}
        paneOpen={hasPane && pane.open}
        // The docked explorer portion's toggle (`toggle_files_panel`,
        // files_panel.rs:234-245): opens the pane with just the explorer
        // portion when closed, closes that portion when docked — never the
        // surface host.
        onToggleFiles={hasPane ? () => rightPaneStore.toggleFilesPanel(paneChatId) : null}
        filesOpen={hasPane && pane.filesOpen}
        paneExpanded={hasPane && pane.expanded}
        // Mounted whether or not the pane is open: the band clips it to zero
        // when shut, so it can glide away with the column instead of blinking
        // out on the first frame of the close. At phone the band renders NO
        // tabs — the strip lives in the drawer's header (RightPane mounts it
        // there, §2.2) — so the prop is gated here at its mount site and the
        // band carries only the expand control next to the toggle.
        paneTabs={
          hasPane && !phone ? <RightTabStrip chatId={paneChatId} pane={pane} /> : undefined
        }
        onToggleExpand={hasPane ? () => rightPaneStore.toggleExpanded(paneChatId) : null}
      />
      {/*
        `SidebarPane::render`'s route match (shell.rs:993-1008): the sidebar
        COLUMN persists — width, seam, collapse, titlebar pad all stay — and
        only its CONTENT swaps, the settings nav replacing the chat sidebar
        on `/settings/*`. Never keyed by engine: the desktop's tree never
        is, the sidebar reads the fleet-merged snapshot, and a remount
        would force-close an open add-space palette and reset the
        group-collapse state (ticket 43).
      */}
      <aside className="sidebar">
        <div className="sidebar-inner">
          {route === "settings" ? (
            <SettingsNavBody />
          ) : (
            <SidebarBody />
          )}
        </div>
      </aside>
      {/*
        The seam floats over the sidebar/conversation seam with zero layout
        width, so the sidebar's right gutter stays exactly as wide as its left
        one — a real flex child read as lopsided spacing (`shell.rs:7997`). It
        sits outside the sidebar because that column clips its overflow.
      */}
      {!sidebar.collapsed && (
        <PaneSeam
          label="Resize sidebar"
          widthAt={(clientX) => clientX}
          onWidth={(width) => sidebarLayout.setWidth(width)}
          onReset={() => sidebarLayout.reset()}
          className="pane-seam-sidebar"
          // `on_sidebar_drag`: clamp into [SIDEBAR_MIN, SIDEBAR_MAX] and
          // bounce once per held pointer at either bound.
          bounds={{ min: SIDEBAR_MIN, max: SIDEBAR_MAX }}
          bounceVar="--rb-sidebar-edge-offset"
        />
      )}
      <div
        className="sidebar-backdrop"
        role="presentation"
        onClick={() => setSidebarOpen(false)}
      />
      <TerminalProvider>
        {/*
          Mod+J's execution half: the bottom dock, not the right pane (S23).
          Inside the provider because the store is context-scoped.
        */}
        <TerminalShortcutBridge />
        {/*
          `card` + `main` from `shell.rs`: the outer column is the flex
          remainder and the clip; the inner one carries the width the content
          is laid out at, pinned to the wider endpoint across a takeover so the
          transcript is revealed or covered rather than re-wrapping through
          every intermediate width.
        */}
        <main
          className="main panel"
          onDragEnter={onDragEnter}
          onDragOver={onDragOver}
          onDragLeave={onDragLeave}
          onDrop={onDrop}
        >
          <div className="main-inner">
            {!paired ? (
              <Welcome />
            ) : (
              <>
                {/* The routed engine's non-fatal transport states (and a
                    PARKED routed engine while other engines are live — the
                    all-parked case belongs to the gate card in root-layout). */}
                {session !== null && state.className !== "conn-connected" && (!state.parked || !registry.engines.every((engine) => engine.state === "off")) ? (
                  <div className={`banner ${state.parked ? "banner-alert" : ""}`} role="status">
                    <span className={`conn ${state.className}`}>
                      <span className={`dot ${state.dot}`} />
                      {state.label}
                    </span>
                    {state.detail !== null && <span className="banner-detail">{state.detail}</span>}
                    {state.pairable && (
                      <button type="button" className="btn btn-solid" onClick={() => void navigate({ to: "/pair" })}>
                        Pair again
                      </button>
                    )}
                  </div>
                ) : null}
                {/*
                  A parked session is FATAL (revoked credential / engine
                  changed): the gate card owns that case now (see
                  root-layout), so the banner here covers the non-fatal
                  transport states only.
                */}
                <Outlet />
              </>
            )}
          </div>
          {/*
            `#attachment-drop-overlay` — the conversation column's drop veil.
            Revealed only by a drag whose payload is real files (GPUI matches
            the payload's concrete TypeId; `types.includes("Files")` is the
            web's equivalent, so resize markers and text drags can never
            reveal it). Purely visual — pointer-events none — so the drop
            itself keeps bubbling to this column (ticket 17 owns what happens
            to the files).
          */}
          {isChatRoute && paired && (
            <div
              id="attachment-drop-overlay"
              className="attachment-drop-overlay"
              data-on={dropDepth > 0 ? "1" : "0"}
              aria-hidden
            >
              Drop to attach
            </div>
          )}
        </main>
        {/*
          The third column. It is chat-scoped chrome, so routes without one
          (Settings, the blank canvas) simply publish no owner and it is absent
          — their per-chat open flags survive the round trip untouched.
        */}
        {hasPane && (
          <>
            <RightPane chatId={paneChatId} pane={pane} openWidth={paneOpenWidth} glide={glide} />
            {/*
              The docked explorer portion of the one right pane
              (`render_files_panel`): independent of the surface host, sharing
              its height with a left hairline, resized through its own seam.
            */}
            <FilesPaneColumn chatId={paneChatId} pane={pane} />
          </>
        )}
      </TerminalProvider>
      {/*
        The pane's seam, parked on its left edge. Like the sidebar's it lives
        out here: the pane clips, and the target has to straddle both columns.
        Takeover derives its width from the viewport, so it carries no handle,
        and neither does a glide in flight (`shell.rs:7943-7947` — no
        `tween_active`; the desktop's `panel_handoff` guard has no web
        equivalent, there is no handoff to another panel owner).
      */}
      {hasPane && pane.open && !pane.expanded && !glide.gliding && (
        <PaneSeam
          label="Resize panel"
          // Right-anchored (`on_right_pane_drag`): the seam parks on the
          // pane's LEFT edge, which sits left of the docked files column
          // whenever that column is open — so the room from the pointer to
          // the window's edge includes the files column, and the pane's own
          // committed width subtracts it back out.
          widthAt={(clientX) => viewport - clientX - (hasPane && pane.filesOpen ? uiSettings.getSnapshot().filesPanelWidth : 0)}
          onWidth={(width) =>
            rightPaneStore.setWidth(
              paneChatId,
              width,
              rightPaneMaxWidth(viewport, sidebarWidth),
            )
          }
          onReset={() => rightPaneStore.resetWidth(paneChatId)}
          className="pane-seam-right"
          // The shared clamp: [RIGHT_PANE_MIN, rightPaneMaxWidth]. When the
          // max falls below the min the seam pins to max — the chat floor
          // wins and the pane yields.
          bounds={{ min: RIGHT_PANE_MIN, max: rightPaneMaxWidth(viewport, sidebarWidth) }}
          bounceVar="--rb-pane-edge-offset"
        />
      )}
      {hasPane && pane.filesOpen && (
        <PaneSeam
          label="Resize files"
          widthAt={(clientX) => viewport - clientX}
          onWidth={(width) => rightPaneStore.setFilesPanelWidth(width)}
          onReset={() => uiSettings.updateImmediate({ filesPanelWidth: FILES_PANEL_DEFAULT })}
          className="pane-seam-files"
          bounds={{ min: FILES_PANEL_MIN, max: FILES_PANEL_MAX }}
        />
      )}
      {/*
        The phone pane drawer's backdrop — the mirror of `.sidebar-backdrop`
        above: fixed, z 20 under the drawer's 30, and shown by the phone CSS
        only while the pane is open (the drawer's own `aria-hidden` drives the
        sibling rule). A tap closes the pane, the phone counterpart of the
        sidebar backdrop's tap. Mounted unconditionally like the sidebar's —
        at ≥769px the stylesheet hides it, and the pane is the in-flow column
        there.
      */}
      <div
        className="pane-backdrop"
        role="presentation"
        onClick={() => {
          if (paneChatId !== null) {
            rightPaneStore.close(paneChatId);
          }
        }}
      />
    </div>
  );
}

/**
 * Ticket 55 / research M7(b) — the phone drawer's close-on-navigate rule:
 * close iff the navigation actually changed the pathname, the drawer is
 * open (the already-closed state is the effect's early return, encoded
 * here as the short-circuit), and the viewport is at phone width — the
 * toggle's own matchMedia breakpoint, so the desktop column never closes
 * on selection ("there is nothing to port", M7(b)). Extracted pure so the
 * drawer suite can drive the rule's full truth table, the same-path branch
 * included: the effect's `[pathname]` deps never refire on a same-path
 * re-notify, and the previous-pathname ref keeps even a refire from
 * reading as a navigation.
 */
export function shouldCloseDrawer(
  pathChanged: boolean,
  sidebarOpen: boolean,
  isPhone: boolean,
): boolean {
  return pathChanged && sidebarOpen && isPhone;
}

/**
 * `main_takeover_tween` + `stable_panel_content_width`, as a hook: across a
 * takeover toggle the conversation lays out at the LARGER of its two widths
 * for the glide, and is clipped to the animating column. `null` once settled,
 * which hands the column back to plain flex.
 *
 * Only `expanded` triggers it — the desktop arms this tween in
 * `toggle_right_pane_expand` (both directions) and, for a plain open/close,
 * only when leaving takeover.
 *
 * A layout effect, not a passive one: the class it flips
 * (`shell-pane-gliding`) must ride the SAME commit as the takeover flip, or
 * the titlebar's row-left jump would spend a frame inside its translateX
 * transition before the suppression below lands.
 */
function useTakeoverStableWidth(takeover: boolean, conversation: number): number | null {
  const [stable, setStable] = useState<number | null>(null);
  const previous = useRef({ takeover, conversation });

  useLayoutEffect(() => {
    const was = previous.current;
    previous.current = { takeover, conversation };
    if (was.takeover === takeover) {
      return;
    }
    setStable(Math.max(was.conversation, conversation));
    const timer = window.setTimeout(() => setStable(null), TAKEOVER_GLIDE_MS);
    return () => window.clearTimeout(timer);
  }, [takeover, conversation]);

  return stable;
}

/**
 * The chat a `/chat/$chatId` path names, or `null` anywhere else — the pane is
 * chat-scoped chrome, so Settings, the blank canvas and the changes page get
 * none. Matching the path rather than reading a route-published value keeps
 * this synchronous with navigation.
 */
function chatIdOf(pathname: string): string | null {
  const match = /^\/chat\/([^/]+)\/?$/.exec(pathname);
  if (match === null || match[1] === undefined) {
    return null;
  }
  try {
    return decodeURIComponent(match[1]);
  } catch {
    return match[1];
  }
}

/** True when a keyboard event would land inside a typed-into element. */
function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target.isContentEditable) {
    return true;
  }
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

/**
 * `ToggleTerminal`'s execution half — Mod+J toggles the BOTTOM drawer on the
 * conversation column (`toggle_terminal`, shell.rs:3026-3062), never the
 * right-pane terminal surface (gap S23; that surface is reached from the
 * pane's `+` menu). Opening claims focus exactly once — the dock's own
 * mount effect (§2.8's frame-deferred `focusActive`); closing hands focus
 * back to the composer, which the dock's open-watch also does (the bridge
 * keeps it for the case the drawer was already unmounted).
 */
function TerminalShortcutBridge() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  useEffect(
    () =>
      onShortcut("toggle-terminal", () => {
        const chatId = chatIdOf(pathname);
        if (chatId === null) {
          return;
        }
        drawerTerminalStore.toggle(chatId);
        if (drawerTerminalStore.stateFor(chatId)?.open !== true) {
          document.querySelector<HTMLTextAreaElement>(".composer-input")?.focus();
        }
      }),
    [pathname],
  );
  return null;
}

function Welcome() {
  return (
    <div className="empty-state">
      <Icon name="zeronLogo" size={44} className="empty-state-mark" />
      <h1>Zeron</h1>
      <p>This browser has no paired engine yet.</p>
      <Link className="btn btn-solid" to="/pair">
        Pair an engine
      </Link>
    </div>
  );
}
