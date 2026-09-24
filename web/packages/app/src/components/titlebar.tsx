import { useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { motion } from "@zeron/theme";
import { Icon, type IconName } from "@zeron/icons";
import { installIdentityFreeze } from "../lib/identity-freeze";
import { sidebarTweenSignal } from "../lib/sidebar-tween";
import {
  CLUSTER_BUTTONS_WIDTH,
  evalWidthTween,
  TITLEBAR_ACTION_SLOT_WIDTH,
  TITLEBAR_CLUSTER_PAD,
  TITLEBAR_HEIGHT,
  TITLEBAR_ISLAND_INSET,
  TITLEBAR_TOP_PAD,
} from "../state/layout";
import { usePrefersReducedMotion } from "../state/media";

/**
 * The unified titlebar — the desktop's `render_titlebar_cluster` (left) and
 * `render_session_title_bar` (identity + trailing).
 *
 * Its shape, from `tabs.rs`:
 *
 *     [sidebar toggle] [back forward] [+] … [harness icon + title + target] … [toggle-files][toggle-changes]
 *
 * The trailing section is the desktop's two fixed right-edge anchors
 * (`PANEL_TOGGLE_SLOTS`, tabs.rs:48): the Files (explorer) toggle and the
 * right pane's open/close toggle — two 28px `header_icon_button`s. With the
 * pane open, its surface tabs and the expand button reveal to their left
 * inside a band as wide as the pane. Panel surfaces are tabs in that pane,
 * never buttons up here; the Files toggle is the one exception the desktop
 * ships — it drives the docked explorer portion, which is not a surface tab
 * (tickets 22/23).
 *
 * Geometry is the desktop's to the pixel: a 38px bar
 * (`layout::TITLEBAR_HEIGHT`) whose content rides 4px lower than centre
 * (`TITLEBAR_TOP_PAD`); a 10px cluster inset (`TITLEBAR_CLUSTER_PAD`); 24px
 * cluster controls and 28px trailing controls on a 2px within-group rhythm
 * (`TITLEBAR_CONTROL_GAP`) with 8px between groups (`TITLEBAR_GROUP_GAP`) and
 * a 6px trailing inset (`TITLEBAR_ACTION_EDGE_INSET`). Icons are 16px in every
 * control (`window_control_button` / `nav_history_button` /
 * `header_icon_button` all render at size 16).
 *
 * The `+` is driven by `titlebar_new_session_alpha`: 1 only on the chat route
 * with a chat selected. It is conditionally rendered — the desktop's
 * `show_plus.then(...)` leaves no phantom slot at alpha 0 (ticket 60), so the
 * cluster's shrink-to-fit width ends at the last visible control — and
 * APPEARS with a mount fade on the same 200ms resize curve
 * `--rb-titlebar-row-left` transitions on; the desktop tweens the two as one.
 *
 * The island (ticket 34) is the desktop's `render_titlebar_cluster` panel:
 * a frosted 12/20 island behind the controls, shown exactly when the canvas
 * route, the sidebar collapsed, and a resolved background coincide — the
 * cluster's FIRST child, tweened by `useIslandTween` over the resize curve.
 *
 * The browser owns the window, so the desktop's minimize/maximize/close
 * cluster has no counterpart here, and the drag region is layout-only.
 *
 * The bar overlays the full window width: the sidebar and the main column
 * both start beneath it, which is why each pads itself down by
 * `--rb-titlebar-height`.
 */

export interface TitlebarProps {
  readonly onToggleSidebar: () => void;
  readonly onBack: () => void;
  readonly onForward: () => void;
  readonly canBack: boolean;
  readonly canForward: boolean;
  /**
   * `titlebar_new_session_alpha` — 1 while an existing chat is selected on
   * the chat route, else 0. The `+` renders only above the desktop's 0.01
   * gate and appears with a mount fade.
   */
  readonly newSessionAlpha: number;
  /**
   * `island_target` (ticket 34) — 1 exactly on the canvas route with the
   * sidebar collapsed and a resolved background beneath; the shell wires
   * the inputs and this component tweens 0 ↔ 1 over the resize curve.
   */
  readonly islandTarget: number;
  /** Null hides the `+` entirely (no handler — the blank canvas, Settings). */
  readonly onNewSession?: (() => void) | null;
  /** The identity group: harness mark, title, and the `space @ device` tag. */
  readonly identity?: ReactNode;
  /**
   * The project-Actions control — the desktop's `render_project_actions_control`
   * (tabs.rs:316-320): mounted after the `flex_1` spacer (052b4b9a's
   * right alignment) so it rides the right edge beside the trailing group.
   */
  readonly actions?: ReactNode;
  /** The right pane's toggle. Absent when no chat owns a pane. */
  readonly onTogglePane?: (() => void) | null;
  readonly paneOpen?: boolean;
  /**
   * The docked Files (explorer) toggle — the desktop's `toggle-files-panel`
   * (tabs.rs:366-384), LEFT of the pane toggle. Absent when no chat owns a
   * pane (the desktop hides the whole trailing group on the canvas).
   */
  readonly onToggleFiles?: (() => void) | null;
  /** The docked explorer portion's flag — drives the button's active wash. */
  readonly filesOpen?: boolean;
  /** The pane's surface tabs — revealed to the toggle's left while open. */
  readonly paneTabs?: ReactNode;
  readonly paneExpanded?: boolean;
  readonly onToggleExpand?: (() => void) | null;
}

// ---------------------------------------------------------------------------
// The island's pure logic — `island_target` + `titlebar_island_vertical_geometry`
// ---------------------------------------------------------------------------

/**
 * `island_target` (shell.rs:3990-4001, as amended by ticket 48's
 * resolved-artwork predicate): 1 exactly on the chat route with nothing
 * selected — the new-thread canvas — the sidebar collapsed, and a
 * background RESOLVING beneath: the resolved artwork (installed, or the
 * bundled default once the stored entry no longer decodes), never the raw
 * setting. A resolution failure hides the island; the default fallback
 * still shows it.
 */
export function islandTarget(options: {
  readonly isChatRoute: boolean;
  readonly hasSelectedChat: boolean;
  readonly sidebarCollapsed: boolean;
  readonly backgroundResolves: boolean;
}): number {
  return options.isChatRoute &&
    !options.hasSelectedChat &&
    options.sidebarCollapsed &&
    options.backgroundResolves
    ? 1
    : 0;
}

/**
 * `titlebar_island_vertical_geometry` (shell.rs:829-835): the island grows
 * 28→32 with progress, centered on the padded row's center
 * `(TITLEBAR_HEIGHT + TITLEBAR_TOP_PAD) · 0.5` — 4px of air around the 24px
 * controls at full expansion. `top` is in the 38px row's space.
 */
export function titlebarIslandVerticalGeometry(progress: number): {
  readonly top: number;
  readonly height: number;
} {
  const height = 28 + 4 * Math.min(Math.max(progress, 0), 1);
  const center = (TITLEBAR_HEIGHT + TITLEBAR_TOP_PAD) * 0.5;
  return { top: center - height * 0.5, height };
}

/**
 * The island's horizontal span in window space (tickets 60/66): the wrapper
 * is `left(6).right_0()` over the cluster (shell.rs:4027-4028), and an
 * absolute child's insets resolve against the parent's PADDING box — border
 * removed, padding kept (taffy 0.12.2 flexbox.rs:2164-2167, 2336-2340; CSS
 * absolute positioning follows the same rule). The desktop's cluster is
 * `left_0()` with `.px(TITLEBAR_CLUSTER_PAD)` (shell.rs:4025-4034) and the
 * web's is that same shape since ticket 66 (`.titlebar-cluster`: left 0,
 * padding-inline 10), so the span is [6, 10 + controls + 10]: [6, 102],
 * width 96 with the `+` hidden, [6, 134] while shown. The `+`'s 32px slot
 * exists ONLY while it is in the tree — `show_plus.then(...)`
 * (shell.rs:4093-4104) reserves no phantom slot at alpha 0 — and the two
 * states never coexist, so the residual island during a route change still
 * covers the control row it faded from. The prior research's [16, 92] read
 * the desktop's CONTENT box [10, 92] as the padding box; the buttons keep
 * that exact [10, 92] span, now covered 10px past each edge like the
 * desktop. Ticket 66 also wires this helper to the shipped geometry: the
 * tests derive the same bounds from app.css itself, so it can no longer
 * drift from the rendered island.
 */
export function titlebarIslandHorizontalGeometry(showsNewSession: boolean): {
  readonly left: number;
  readonly right: number;
} {
  const containerWidth =
    2 * TITLEBAR_CLUSTER_PAD +
    CLUSTER_BUTTONS_WIDTH +
    (showsNewSession ? TITLEBAR_ACTION_SLOT_WIDTH : 0);
  return {
    left: TITLEBAR_ISLAND_INSET,
    right: containerWidth,
  };
}

/** `motion::RESIZE` — the 200ms ease-out the island's opacity/height tween rides. */
export const ISLAND_TWEEN_MS =
  motion.specs.find((spec) => spec.name === "resize")?.durationMs ?? 200;

/**
 * One island scalar's geometry/opacity write (ticket 64 §2.4): for
 * p = clamp(progress, 0, 1) the height runs 28→32 centered on the padded
 * row's center 21 (`titlebar_island_vertical_geometry`, shell.rs:829-835),
 * the cluster-local top is the row's top less the cluster's own 4px pad,
 * and the opacity is p. Written straight to the element — from the driver's
 * rAF tick and snap paths, always before paint — never through React state.
 */
function paintIslandGeometry(el: HTMLElement, p: number): void {
  const progress = Math.min(Math.max(p, 0), 1);
  const geometry = titlebarIslandVerticalGeometry(progress);
  el.style.top = `${geometry.top - TITLEBAR_TOP_PAD}px`;
  el.style.height = `${geometry.height}px`;
  el.style.opacity = `${progress}`;
}

export interface IslandTweenDriverOptions {
  /** One frame's scalar write (the island element's geometry/opacity). */
  readonly paint: (p: number) => void;
  /**
   * The mount gate (the desktop's `(island > 0.001)` content gate): fires on
   * CROSSINGS only, plus the settled initial presentation — never per frame.
   */
  readonly publishMounted: (mounted: boolean) => void;
  readonly now?: () => number;
  readonly requestFrame?: (callback: () => void) => number;
  readonly cancelFrame?: (handle: number) => void;
}

/**
 * The island's scalar tween, DOM-owned (ticket 64 §2.4 — the desktop's
 * persistent `island` tween, shell.rs:3995-4024): ONE imperative rAF loop
 * over a `{from, to, startedAt}` tween ref evaluating the existing
 * `evalWidthTween` (the 200ms RESIZE ease-out), writing geometry/opacity to
 * the element per frame — the `setPainted`/`setPump` per-frame React loop
 * this replaces. A retarget reads the running tween at that instant and
 * starts from the PAINTED progress (a reversal never jumps back to an
 * endpoint); the initial presentation is settled (a fresh mount at target 1
 * starts expanded with its panel; target 0 has no panel — no entrance); a
 * reduced-motion activation writes the endpoint immediately, synchronizes
 * the mount gate, and cancels the frame — even when the target is
 * unchanged. React hears from the scalar only through the gate crossings.
 */
export class IslandTweenDriver {
  #tween: { from: number; to: number; startedAt: number } | null = null;
  #painted: number;
  #mounted: boolean;
  #raf: number | null = null;
  readonly #paint: (p: number) => void;
  readonly #publishMounted: (mounted: boolean) => void;
  readonly #now: () => number;
  readonly #requestFrame: (callback: () => void) => number;
  readonly #cancelFrame: (handle: number) => void;

  constructor(target: number, options: IslandTweenDriverOptions) {
    this.#paint = options.paint;
    this.#publishMounted = options.publishMounted;
    this.#now = options.now ?? (() => performance.now());
    this.#requestFrame = options.requestFrame ?? ((callback) => requestAnimationFrame(callback));
    this.#cancelFrame = options.cancelFrame ?? ((handle) => cancelAnimationFrame(handle));
    this.#painted = target;
    this.#mounted = target > 0.001;
    // The settled initial presentation, written before first paint: target
    // 1 starts with p 1 and a mounted panel; target 0 has no panel.
    this.#paint(target);
    options.publishMounted(this.#mounted);
  }

  /** The painted scalar — the reversal's read-back (shell.rs:4013-4024). */
  get painted(): number {
    return this.#painted;
  }

  /**
   * Retarget (or re-snap) the tween. A flip with a running tween starts
   * from the painted value; reduced motion — or already sitting on the
   * target — writes the endpoint immediately and clears the frame, whatever
   * the target's change state (a preference flip with an unchanged target
   * still lands the endpoint).
   */
  setTarget(target: number, reduced: boolean): void {
    if (reduced || (this.#tween === null && this.#painted === target)) {
      this.#cancelPendingFrame();
      this.#tween = null;
      this.#paintTo(target);
      return;
    }
    if (this.#tween !== null && this.#tween.to === target) {
      // Already gliding there: the running tween continues — re-arming
      // would restart the ease curve from the painted value.
      return;
    }
    const nowMs = this.#now();
    const tween = this.#tween;
    const from =
      tween === null ? this.#painted : evalWidthTween(tween.from, tween.to, nowMs - tween.startedAt);
    this.#tween = { from, to: target, startedAt: nowMs };
    // The pending frame (if any) reads `#tween` at fire time, so a retarget
    // never leaves an obsolete callback evaluating the OLD tween, and the
    // loop stays armed exactly once.
    if (this.#raf === null) {
      this.#raf = this.#requestFrame(this.#frame);
    }
  }

  /** Teardown: cancel any pending callback and drop the tween. */
  dispose(): void {
    this.#cancelPendingFrame();
    this.#tween = null;
  }

  #cancelPendingFrame(): void {
    if (this.#raf !== null) {
      this.#cancelFrame(this.#raf);
      this.#raf = null;
    }
  }

  readonly #frame = (): void => {
    this.#raf = null;
    const tween = this.#tween;
    if (tween === null) {
      return;
    }
    const elapsed = this.#now() - tween.startedAt;
    if (elapsed >= ISLAND_TWEEN_MS) {
      // Settle on the EXACT endpoint — no residue past the duration.
      this.#tween = null;
      this.#paintTo(tween.to);
      return;
    }
    this.#paintTo(evalWidthTween(tween.from, tween.to, elapsed));
    this.#raf = this.#requestFrame(this.#frame);
  };

  #paintTo(p: number): void {
    this.#painted = p;
    this.#paint(p);
    const mounted = p > 0.001;
    if (mounted !== this.#mounted) {
      this.#mounted = mounted;
      this.#publishMounted(mounted);
    }
  }
}

/**
 * The island's tween, wired to the element (shell.rs:3995-4012): the driver
 * owns the scalar and its per-frame DOM writes; React holds ONLY the
 * mount-gate boolean (the frosted panel mounts iff the painted value clears
 * 0.001), published on crossings and the discrete target/preference
 * lifecycle — the frame scalar never becomes React state.
 */
function useIslandTween(
  target: number,
  reduced: boolean,
  islandRef: { readonly current: HTMLElement | null },
): boolean {
  const [panelMounted, setPanelMounted] = useState(target > 0.001);
  const driverRef = useRef<IslandTweenDriver | null>(null);
  useLayoutEffect(() => {
    const driver = new IslandTweenDriver(target, {
      paint: (p) => {
        const el = islandRef.current;
        if (el !== null) {
          paintIslandGeometry(el, p);
        }
      },
      publishMounted: setPanelMounted,
    });
    driverRef.current = driver;
    return () => {
      driverRef.current = null;
      driver.dispose();
    };
    // One driver per mount; the target/preference lifecycle rides below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useLayoutEffect(() => {
    driverRef.current?.setTarget(target, reduced);
  }, [target, reduced]);
  return panelMounted;
}

export function Titlebar({
  onToggleSidebar,
  onBack,
  onForward,
  canBack,
  canForward,
  newSessionAlpha,
  islandTarget,
  onNewSession,
  identity,
  actions,
  onTogglePane,
  paneOpen = false,
  onToggleFiles,
  filesOpen = false,
  paneTabs,
  paneExpanded = false,
  onToggleExpand,
}: TitlebarProps) {
  const takeover = paneOpen && paneExpanded;
  const reduced = usePrefersReducedMotion();
  const islandRef = useRef<HTMLDivElement | null>(null);
  const islandPanelMounted = useIslandTween(islandTarget, reduced, islandRef);
  // The identity freeze (ticket 63): while the sidebar tween runs the row's
  // free space slides (its two inputs animate on one curve but from
  // endpoint deltas that do not cancel when the pane is width-clamped),
  // and the identity is the row's only shrinkable child. The subscription
  // is the freeze's ONLY integration point — every settle path funnels
  // through `sidebarTweenSignal.settle()` — so the box pins at the flip
  // (the capture reads the layout while the row's transitions still sit at
  // their pre-tween values) and the truncation re-evaluates exactly once
  // at settle. The dock glide never moves the row, so the sidebar signal
  // is the only window. A layout effect so the subscription exists before
  // any flip could arm it.
  const identityRef = useRef<HTMLDivElement | null>(null);
  useLayoutEffect(
    () => installIdentityFreeze(sidebarTweenSignal, () => identityRef.current),
    [],
  );
  return (
    <div className={`titlebar ${takeover ? "titlebar-takeover" : ""}`}>
      <div className="titlebar-cluster">
        {/*
          The island (ticket 34) — `render_titlebar_cluster`'s frosted panel
          (shell.rs:3990-4042), visible exactly in the reported state: the
          canvas route, the sidebar collapsed, a background resolving
          beneath. The wrapper is the cluster row's FIRST child, left 6 /
          right 0 like the desktop's, painted BEHIND the controls
          (`z-index: -1`) — pure chrome behind existing controls, no hit
          area. `top`/`height`/`opacity` are the tween driver's imperative
          writes (ticket 64 — before paint, never React state); the top is
          row-space less the cluster's own top pad, since this row's
          containing block starts `TITLEBAR_TOP_PAD` down. The frosted panel
          itself mounts only while the painted scalar clears 0.001, exactly
          the desktop's `(island > 0.001)` content gate.
        */}
        <div className="titlebar-island" aria-hidden="true" ref={islandRef}>
          {islandPanelMounted && <div className="titlebar-island-panel" />}
        </div>
        <WindowControl icon="sidebarMinimalisticLeft" label="Toggle sidebar" onClick={onToggleSidebar} />
        <div className="titlebar-group titlebar-nav">
          <NavHistoryButton icon="arrowLeft" label="Back" onClick={onBack} enabled={canBack} />
          <NavHistoryButton icon="arrowRight" label="Forward" onClick={onForward} enabled={canForward} />
        </div>
        {/*
          The `+` renders only while shown — the desktop's
          `show_plus.then(...)` (shell.rs:4093-4104), gate `plus_alpha >
          0.01` — so at alpha 0 it contributes NO geometry and the island's
          `right: 0` anchors just past the last visible control (ticket 60;
          ticket 66 restored the desktop's padded containing block, so the
          island covers the [10, 92] controls as [6, 102]). The appear fade is a
          MOUNT animation on the same 200ms resize curve the row's left
          padding rides; the disappear is the unmount itself — the island
          requires "no selected chat" and the `+` requires one, so the two
          are never visible together and nothing is mid-fade when it goes.
        */}
        {onNewSession != null && newSessionAlpha > 0.01 && (
          <div className="titlebar-new-session">
            <WindowControl icon="plus" label="New session" onClick={onNewSession} />
          </div>
        )}
      </div>
      {/*
        In panel takeover the header strip spans the whole band, so the
        identity hides for the duration rather than sitting under it.
      */}
      {identity !== undefined && !takeover && (
        <div className="titlebar-identity" ref={identityRef}>
          {identity}
        </div>
      )}
      {/*
        The desktop's `flex_1` spacer (tabs.rs:353) — kept even when the
        identity is empty so the trailing group stays right-anchored.
      */}
      <div className="titlebar-fill" />
      {/*
        The project-Actions control (tabs.rs:316-320, after the spacer —
        052b4b9a): right-aligned beside the trailing group, hidden in
        takeover and on the canvas exactly like the desktop's gate.
      */}
      {actions !== undefined && !takeover && actions}
      {onTogglePane != null && (
        <div className="titlebar-trailing">
          {/*
            The pane's header. Its width is NOT a prop: the stylesheet derives
            it from `--rb-pane-now`, the same variable the pane column itself
            lays out on, so the two are one number and cannot drift — the
            desktop reads one `right_now` for both. Routing it through the
            chrome store instead put an effect-published copy a frame behind
            the column, which is what made the strip lurch.

            The band element stays mounted at width 0 while shut (a width
            transition needs something to animate from) but renders NO
            children then — the desktop unmounts the whole trailing band's
            content, and a hidden expand button must not stay tabbable
            inside a zero-width box.
          */}
          <div className="titlebar-pane-band">
            {paneOpen && (
              <div className="titlebar-pane-band-inner">
                <div className="titlebar-pane-tabs">{paneTabs}</div>
                {onToggleExpand != null && (
                  <HeaderIconButton
                    icon={paneExpanded ? "collapseArrows" : "expandArrows"}
                    label={paneExpanded ? "Collapse panel" : "Expand panel"}
                    onClick={onToggleExpand}
                  />
                )}
              </div>
            )}
          </div>
          {/*
            The Files toggle (`toggle-files-panel`, tabs.rs:366-384): the
            desktop's custom FILE_TREE icon, aria-label "Hide/Show files
            panel", and `bg(wash(0.09))` while the docked explorer portion is
            open — the first of the two fixed right-edge anchors, LEFT of the
            pane toggle. It is not the pane band's business: its width budget
            already reserves both anchor slots (PANEL_TOGGLE_SLOTS).
          */}
          {onToggleFiles != null && (
            <HeaderIconButton
              icon="fileTree"
              label={filesOpen ? "Hide files panel" : "Show files panel"}
              onClick={onToggleFiles}
              active={filesOpen}
            />
          )}
          <HeaderIconButton icon="sidebarMinimalistic" label="Toggle panel" onClick={onTogglePane} />
        </div>
      )}
    </div>
  );
}

/**
 * The desktop's `window_control_button` (`shell.rs:7369`): a 24px square
 * control with a 6px radius and a 16px glyph, quiet until hovered.
 *
 * There is no pressed/active variant: `window_control_button` has one resting
 * look and one hover, whatever the control it drives is currently doing.
 */
function WindowControl({
  icon,
  label,
  onClick,
  disabled = false,
  tabIndex,
}: {
  icon: IconName;
  label: string;
  onClick: () => void;
  disabled?: boolean;
  tabIndex?: number;
}) {
  return (
    <button
      type="button"
      className="window-control"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      tabIndex={tabIndex}
    >
      <Icon name={icon} size={16} />
    </button>
  );
}

/**
 * The desktop's `nav_history_button` (`shell.rs:7523`): enabled it is exactly
 * a `window_control_button`; disabled it is a NON-interactive 24px box — no
 * background at all, only the icon dimmed to `text_muted @ 35%` — that still
 * holds its place so the cluster never reflows.
 */
function NavHistoryButton({
  icon,
  label,
  onClick,
  enabled,
}: {
  icon: IconName;
  label: string;
  onClick: () => void;
  enabled: boolean;
}) {
  return (
    <button
      type="button"
      className="window-control"
      onClick={onClick}
      disabled={!enabled}
      aria-label={label}
      title={label}
    >
      <Icon name={icon} size={16} />
    </button>
  );
}

/**
 * The desktop's `header_icon_button` (`shell.rs:7552`) — the trailing
 * controls: a 28px square with a 6px radius and a 16px glyph, transparent at
 * rest and `wash(0.11)` on hover. One active variant exists, the Files
 * toggle's (`tabs.rs:381-383`): `bg(wash(0.09))` while the docked explorer
 * portion is open, rendered through the `data-active` attribute; the pane
 * toggle stays a plain button whatever the pane is doing.
 */
function HeaderIconButton({
  icon,
  label,
  onClick,
  active = false,
}: {
  icon: IconName;
  label: string;
  onClick: () => void;
  /** The active wash — `bg(wash(0.09))` while the driven panel is open. */
  active?: boolean;
}) {
  return (
    <button
      type="button"
      className="header-icon-button"
      onClick={onClick}
      aria-label={label}
      title={label}
      data-active={active ? "1" : undefined}
    >
      <Icon name={icon} size={16} />
    </button>
  );
}
