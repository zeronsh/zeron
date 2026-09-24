import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from "react";
import { motion } from "@zeron/theme";
import { Icon } from "@zeron/icons";
import "@xterm/xterm/css/xterm.css";
import type { ChatTerminals, TerminalStore, TerminalTabRecord } from "./store";
import { displayTitle } from "./store";
import {
  TAB_WIDTH,
  TERMINAL_DEFAULT_HEIGHT,
  TERMINAL_MAX_VH,
  TERMINAL_MIN_HEIGHT,
  dropIndex,
  slideOffset,
} from "./tabs";
import { evalWidthTween } from "../state/layout";

/**
 * The terminal panel — the web peer of the desktop's `TerminalPanel`
 * (`crates/ui/src/terminal/panel.rs`), in its two host shapes:
 *
 * - **Drawer** (`docked = false`, the default): the bottom dock under the
 *   conversation column (`render_terminal_container`, shell.rs:6259-6368) —
 *   a 10px height-drag handle floating over the panel's top edge (hover
 *   highlight, constrained-state hiding, double-click reset), the whole
 *   container height-tweened 200ms on toggle while an inner, fixed-height
 *   child rides the clip (the same clip-don't-squeeze trick the side panes
 *   use). Its own tab bar ends in the collapse chevron; closing the last tab
 *   collapses the drawer (an empty dock is dead space).
 * - **Embedded** (`docked = true`): the right pane's terminal surface. The
 *   panel's own bar is hidden entirely — the pane's strip carries one chip
 *   per terminal tab (`Terminal(tab)` surfaces) — and the host selects the
 *   surface's tab by key (`select_tab_by_key`).
 *
 * The tab bar keeps the desktop's fixed-width tabs, pointer drag-reorder
 * with sliding siblings, middle-click close, and a "+" new-tab button.
 */
export function TerminalDock({
  store,
  chatId,
  docked = false,
  /** The pane surface's terminal tab key (embedded host only). */
  tabKey,
}: {
  store: TerminalStore;
  chatId: string;
  docked?: boolean;
  tabKey?: string;
}) {
  useSyncExternalStore(store.subscribe, store.getVersion);
  const chat = store.stateFor(chatId);
  const open = chat !== undefined && chat.open;
  const height = chat?.height ?? TERMINAL_DEFAULT_HEIGHT;

  // `render_right_pane`'s Terminal branch: keep the embedded panel's active
  // tab aligned with the resolved surface (fallbacks can move it). A layout
  // effect so the select lands before the dock's focus pass.
  useLayoutEffect(() => {
    if (docked && tabKey !== undefined) {
      store.selectTabByKey(chatId, tabKey);
    }
  }, [docked, tabKey, store, chatId]);

  // §2.8: toggling the drawer closed hands focus back to the composer —
  // every close path (Mod+J, the collapse chevron, last-tab close) lands
  // here. `shell.rs:3049`.
  const wasOpen = useRef(open);
  useEffect(() => {
    if (wasOpen.current && !open) {
      document.querySelector<HTMLTextAreaElement>(".composer-input")?.focus();
    }
    wasOpen.current = open;
  }, [open]);

  if (docked) {
    if (chat === undefined || chat.tabs.length === 0) {
      return null;
    }
    return (
      <section className="term-dock term-dock-surface" aria-label="Terminal">
        <DockBody store={store} chatId={chatId} chat={chat} docked />
      </section>
    );
  }

  return <DrawerDock store={store} chatId={chatId} chat={chat} open={open} height={height} />;
}

/** `motion::RESIZE` — the drawer's open/close tween, 200ms ease-out. */
const RESIZE_MS = motion.specs.find((spec) => spec.name === "resize")?.durationMs ?? 200;

/**
 * The drawer host: the outer container's height rides the 200ms tween on
 * toggle and the pointer 1:1 while dragging, while the inner child holds the
 * full panel height so nothing reflows mid-transition (`render_terminal_container`).
 */
function DrawerDock({
  store,
  chatId,
  chat,
  open,
  height,
}: {
  store: TerminalStore;
  chatId: string;
  chat: ChatTerminals | undefined;
  open: boolean;
  height: number;
}) {
  const outerRef = useRef<HTMLElement | null>(null);
  const [glide, setGlide] = useState<{ from: number; to: number } | null>(null);
  const [handleState, setHandleState] = useState<"active" | "constrained" | null>(null);
  const previous = useRef({ open, height });
  // The glide state read inside the arm effect below without joining its
  // deps (a dep would re-run the arm on every tween tick).
  const glideRef = useRef(glide);
  glideRef.current = glide;

  // Arm the tween on an open/close flip (`toggle_terminal`'s WidthTween); a
  // height change with no flip is a drag or a reset — live tracking, the
  // desktop clears the tween there. Reduced motion snaps to the target.
  useLayoutEffect(() => {
    const was = previous.current;
    previous.current = { open, height };
    if (was.open === open) {
      if (glideRef.current !== null && was.height !== height) {
        setGlide(null);
      }
      return;
    }
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setGlide(null);
      return;
    }
    setGlide({ from: was.open ? was.height : 0, to: open ? height : 0 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, height]);

  // The per-frame height writer (`eval_tween`, RightPane's takeover-loop
  // pattern): imperative so re-renders never fight the animation, with the
  // steady state (no glide) written directly — 1:1 pointer tracking.
  useLayoutEffect(() => {
    const outer = outerRef.current;
    if (outer === null) {
      return;
    }
    if (glide === null) {
      outer.style.height = `${open ? height : 0}px`;
      return;
    }
    const { from, to } = glide;
    const started = performance.now();
    let raf = 0;
    const write = (elapsed: number): void => {
      outer.style.height = `${evalWidthTween(from, to, elapsed)}px`;
    };
    write(0);
    const tick = (now: number): void => {
      const elapsed = now - started;
      if (elapsed >= RESIZE_MS) {
        outer.style.height = `${to}px`;
        setGlide(null);
        return;
      }
      write(elapsed);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [glide, open, height]);

  // Mounted through the closing glide, not just while open — unmounting on
  // the first frame would empty the dock before it moves. (An armed glide
  // always has its chat record: closing keeps it, and open requires it.)
  if ((!open && glide === null) || chat === undefined) {
    return null;
  }
  const innerHeight = chat.height;

  // `on_terminal_drag` (shell.rs:3064-3084): anchor = (pointerY, height);
  // "active" only while strictly inside (MIN, max) — pinned at a limit the
  // highlight reports "constrained" instead (forced back to opacity 0).
  const startHeightDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startY = event.clientY;
    const startHeight = height;
    const onMove = (move: PointerEvent) => {
      const requested = startHeight + (startY - move.clientY);
      const max = Math.max(window.innerHeight * TERMINAL_MAX_VH, TERMINAL_MIN_HEIGHT);
      store.setHeight(chatId, requested, window.innerHeight);
      setHandleState(
        requested > TERMINAL_MIN_HEIGHT && requested < max ? "active" : "constrained",
      );
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      setHandleState(null);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  return (
    <section ref={outerRef} className="term-dock term-dock-drawer" aria-label="Terminal">
      <div className="term-dock-inner" style={{ height: innerHeight }}>
        {/*
          The handle FLOATS over the panel's top edge (painted after, so it
          wins hit testing) — stacked above the tab bar, its 10px hitbox
          covers the bar's top strip exactly like the desktop's.
        */}
        <div
          className="term-dock-handle"
          data-state={handleState ?? undefined}
          role="separator"
          aria-orientation="horizontal"
          aria-label="Resize terminal"
          onPointerDown={startHeightDrag}
          onDoubleClick={() => store.resetHeight(chatId, window.innerHeight)}
        />
        <DockBody store={store} chatId={chatId} chat={chat} docked={false} />
      </div>
    </section>
  );
}

function DockBody({
  store,
  chatId,
  chat,
  docked,
}: {
  store: TerminalStore;
  chatId: string;
  chat: ChatTerminals;
  docked: boolean;
}) {
  const bodyRef = useRef<HTMLDivElement | null>(null);

  // Keep the active emulator fitted to the body: drag-resizes, window
  // resizes, and tab switches all land here. The emulator resizes
  // immediately; `ResizeTerminal` debounces inside the controller.
  useEffect(() => {
    const body = bodyRef.current;
    if (body === null) {
      return;
    }
    store.fitActive(chatId);
    const observer = new ResizeObserver(() => store.fitActive(chatId));
    observer.observe(body);
    return () => observer.disconnect();
  }, [store, chatId, chat.active, chat.height]);

  // §2.8 "opening terminals focuses the terminal once": focus is claimed on
  // mount (the open/select event) and on a tab SELECT — never on a resize
  // pixel. The desktop consumes a `focus_pending` flag in the panel's render;
  // mounting/active-change is the React shape of that exactly-once claim.
  useEffect(() => {
    store.focusActive(chatId);
  }, [store, chatId, chat.active]);

  // The close half of the handoff: the drawer's terminal unmounts 200ms
  // after the toggle (through the height tween), and xterm's own keyup
  // handler re-grabs focus after any chord that closed it — so the composer
  // handoff lands HERE, on the unmount of a closed drawer's terminal
  // (`toggle_terminal`'s `window.focus(composer)`, shell.rs:3049).
  useEffect(
    () => () => {
      if (!docked && store.stateFor(chatId)?.open !== true) {
        document.querySelector<HTMLTextAreaElement>(".composer-input")?.focus();
      }
    },
    [store, chatId, docked],
  );

  return (
    <>
      {/* Embedded, the pane's own surface chips replace the internal bar. */}
      {!docked && <TabBar store={store} chatId={chatId} chat={chat} />}
      <div className="term-body" ref={bodyRef}>
        {chat.tabs.map((tab, ix) => (
          <div
            key={tab.key}
            className={ix === chat.active ? "term-host" : "term-host term-host-hidden"}
            ref={(host) => {
              if (host !== null) {
                store.attachTab(chatId, tab.key, host);
              }
            }}
          />
        ))}
      </div>
    </>
  );
}

interface DragState {
  readonly from: number;
  readonly over: number;
  /** Pointer travel in px — the dragged tab follows the cursor. */
  readonly dx: number;
}

function TabBar({ store, chatId, chat }: { store: TerminalStore; chatId: string; chat: ChatTerminals }) {
  const [drag, setDrag] = useState<DragState | null>(null);
  const stripRef = useRef<HTMLDivElement | null>(null);
  // A committed drag is followed by a click carrying the pre-drag index —
  // swallow it (the reorder already tracked the active tab).
  const suppressClick = useRef(false);

  const startDrag = (event: ReactPointerEvent, from: number) => {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    const strip = stripRef.current;
    if (strip === null) {
      return;
    }
    let moved = false;
    const startX = event.clientX;
    const onMove = (move: PointerEvent) => {
      const relX = move.clientX - strip.getBoundingClientRect().left;
      const over = dropIndex(relX, TAB_WIDTH, chat.tabs.length);
      moved = moved || Math.abs(move.clientX - startX) > 4;
      // Keep the dragged tab inside the strip.
      const dx = Math.min(Math.max(move.clientX - startX, -from * TAB_WIDTH), (chat.tabs.length - 1 - from) * TAB_WIDTH);
      setDrag((current) =>
        current !== null && current.from === from && current.over === over && current.dx === dx
          ? current
          : { from, over, dx },
      );
    };
    const onUp = (up: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      const relX = up.clientX - strip.getBoundingClientRect().left;
      const over = dropIndex(relX, TAB_WIDTH, chat.tabs.length);
      setDrag(null);
      if (moved) {
        suppressClick.current = true;
        store.reorderTab(chatId, from, over);
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  const select = (ix: number) => {
    if (suppressClick.current) {
      suppressClick.current = false;
      return;
    }
    store.selectTab(chatId, ix);
    store.focusActive(chatId);
  };

  return (
    <div className="term-tabbar">
      <div className="term-tabstrip" ref={stripRef}>
        {chat.tabs.map((tab, ix) => {
          const dragging = drag !== null && drag.from === ix;
          const slide = drag === null || dragging ? 0 : slideOffset(ix, drag.from, drag.over);
          return (
            <TabChip
              key={tab.key}
              tab={tab}
              selected={ix === chat.active}
              transform={dragging ? `translateX(${drag.dx}px)` : slide === 0 ? undefined : `translateX(${slide * TAB_WIDTH}px)`}
              dragging={dragging}
              onSelect={() => select(ix)}
              onClose={() => store.closeTab(chatId, tab.key)}
              onDragStart={(event) => startDrag(event, ix)}
            />
          );
        })}
        <button
          type="button"
          className="term-tab-add"
          aria-label="New terminal"
          title="New terminal"
          onClick={() => {
            store.addTab(chatId);
            store.focusActive(chatId);
          }}
        >
          <Icon name="plus" size={16} />
        </button>
      </div>
      {/*
        The collapse chevron, pinned right of the bar by the strip's own
        flex-1 (desktop `render_tab_bar`'s trailing spacer) — the drawer-only
        "hide terminal" control, dispatching the same toggle Mod+J does.
      */}
      <button
        type="button"
        className="term-tab-add term-tab-collapse"
        aria-label="Close terminal panel"
        title="Close panel"
        onClick={() => store.toggle(chatId)}
      >
        <Icon name="altArrowDown" size={13} />
      </button>
    </div>
  );
}

function TabChip({
  tab,
  selected,
  transform,
  dragging,
  onSelect,
  onClose,
  onDragStart,
}: {
  tab: TerminalTabRecord;
  selected: boolean;
  transform: string | undefined;
  dragging: boolean;
  onSelect: () => void;
  onClose: () => void;
  onDragStart: (event: ReactPointerEvent) => void;
}) {
  return (
    <div
      className={`term-tab${selected ? " term-tab-active" : ""}${dragging ? " term-tab-dragging" : ""}`}
      style={{ transform }}
      role="tab"
      aria-selected={selected}
      tabIndex={0}
      data-exited={tab.exited ? "" : undefined}
      onClick={onSelect}
      onKeyDown={(event: ReactKeyboardEvent) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect();
        }
      }}
      onPointerDown={onDragStart}
      onAuxClick={(event) => {
        if (event.button === 1) {
          event.preventDefault();
          onClose();
        }
      }}
      title={displayTitle(tab)}
    >
      <Icon name="terminal" size={16} className="term-tab-icon" />
      <span className="term-tab-title">{displayTitle(tab)}</span>
      <button
        type="button"
        className="term-tab-close"
        aria-label="Close terminal"
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
        onPointerDown={(event) => event.stopPropagation()}
      >
        <Icon name="close" size={12} />
      </button>
    </div>
  );
}
