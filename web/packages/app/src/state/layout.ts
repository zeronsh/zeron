import { useSyncExternalStore } from "react";
import { motion } from "@zeron/theme";
import {
  CHAT_PANEL_MIN,
  SIDEBAR_DEFAULT,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  clampOr,
  uiSettings,
} from "./ui-settings";

/**
 * The shell's column geometry — the desktop's `shell.rs` layout state.
 *
 * Three columns share the window width: the sidebar (S), the conversation (C)
 * and the right pane (R). The desktop resolves them with three pure functions
 * (`shell.rs:189`, `:444`, `:450`) and this module is their port, so both
 * clients divide a window the same way:
 *
 *     C = max(0, V - S - R)
 *     R_max      = max(0, V - S - CHAT_PANEL_MIN)   // manual drag
 *     R_takeover = max(0, V - S)                    // expand
 *
 * The asymmetry is deliberate and load-bearing: a manual drag may never take
 * the conversation below [`CHAT_PANEL_MIN`], but takeover is allowed to consume
 * it completely. On a window too narrow to give both their minimums the *pane*
 * yields — it goes below its own floor so the chat keeps its 300px.
 *
 * The sidebar width lives here too, but its storage does not: the desktop
 * persists it in `ui-settings.json` and the browser's peer of that file is
 * `./ui-settings.ts`, so this module owns the geometry and delegates the
 * bytes. The right pane's width is per-chat and lives in `./right-pane.ts`,
 * matching the desktop's split between global widths and per-session flags.
 */

/**
 * `settings.rs`'s column bounds, re-exported from the settings store that owns
 * them so a clamp here and a heal on load can never drift apart.
 */
export { CHAT_PANEL_MIN, SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN };

/** Below this the sidebar stops being a column and becomes a drawer. */
export const PHONE_MAX_WIDTH = 768;

export function conversationWidth(viewport: number, sidebar: number, right: number): number {
  return Math.max(0, viewport - sidebar - right);
}

/**
 * The widest the right pane may be dragged while the conversation retains its
 * floor. On unusually small windows this deliberately falls below the pane's
 * own minimum — see the module note.
 */
export function rightPaneMaxWidth(viewport: number, sidebar: number): number {
  return Math.max(0, viewport - sidebar - CHAT_PANEL_MIN);
}

/** Takeover width: unlike a drag, it may consume the conversation entirely. */
export function rightPaneTakeoverWidth(viewport: number, sidebar: number): number {
  return Math.max(0, viewport - sidebar);
}

/**
 * `stable_panel_content_width` (`shell.rs:177-184`): across an open/close the
 * content column is laid out at the LARGER endpoint, so the surface is
 * revealed or clipped away rather than reflowing through every intermediate
 * width. `null` transition = steady state.
 */
export function stablePanelContentWidth(
  target: number,
  transition: readonly [from: number, to: number] | null,
): number {
  return transition === null ? target : Math.max(transition[0], transition[1]);
}

/**
 * `right_panel_content_width` (`shell.rs:186-191`): the pane's inner width.
 * A takeover transition (`right_takeover_content_tween`) overrides — in
 * takeover the contents TRACK the animating frame instead of holding one end.
 */
export function rightPanelContentWidth(
  target: number,
  transition: readonly [from: number, to: number] | null,
  takeover: number | null,
): number {
  return takeover ?? stablePanelContentWidth(target, transition);
}

// ---------------------------------------------------------------------------
// Width tweens — `shell.rs::eval_tween` (3753-3767) over `motion.rs`'s curves
// ---------------------------------------------------------------------------

/** The resize spec off the motion catalog: 200ms on the `easeOut` curve. */
const RESIZE_SPEC = motion.specs.find((spec) => spec.name === "resize") ?? {
  name: "resize",
  durationMs: 200,
  delayMs: 0,
  curve: "easeOut",
};
const RESIZE_CURVE = motion.curves[RESIZE_SPEC.curve] ?? ([0, 0, 0.58, 1] as const);

/**
 * `motion.rs::CubicBezier::eval` (125-205), ported: a CSS
 * `cubic-bezier(x1, y1, x2, y2)` timing function — Newton-Raphson on x(t)
 * with a bisection fallback, output clamped hard (f32 rounding can push
 * `sample_y` a hair past 1).
 */
export function cubicBezierEval(
  curve: readonly [number, number, number, number],
  x: number,
): number {
  const [x1, y1, x2, y2] = curve;
  const coefficients = (a: number, b: number): readonly [number, number, number] => {
    const c = 3 * a;
    const bb = 3 * (b - a) - c;
    const aa = 1 - c - bb;
    return [aa, bb, c];
  };
  const sampleX = (t: number): number => {
    const [a, b, c] = coefficients(x1, x2);
    return ((a * t + b) * t + c) * t;
  };
  const sampleY = (t: number): number => {
    const [a, b, c] = coefficients(y1, y2);
    return ((a * t + b) * t + c) * t;
  };
  const sampleXDerivative = (t: number): number => {
    const [a, b, c] = coefficients(x1, x2);
    return (3 * a * t + 2 * b) * t + c;
  };
  const solveTForX = (x: number): number => {
    let t = x;
    for (let step = 0; step < 8; step += 1) {
      const err = sampleX(t) - x;
      if (Math.abs(err) < 1e-6) {
        return t;
      }
      const d = sampleXDerivative(t);
      if (Math.abs(d) < 1e-6) {
        break;
      }
      t -= err / d;
    }
    let lo = 0;
    let hi = 1;
    for (let step = 0; step < 32; step += 1) {
      const mid = (lo + hi) / 2;
      if (sampleX(mid) < x) {
        lo = mid;
      } else {
        hi = mid;
      }
    }
    return (lo + hi) / 2;
  };
  if (x <= 0) {
    return 0;
  }
  if (x >= 1) {
    return 1;
  }
  return Math.min(1, Math.max(0, sampleY(solveTForX(x))));
}

/**
 * `shell.rs::eval_tween` (3753-3767) for widths: the eased 200ms lerp from
 * `from` to `to` on the resize curve. Absent, stale, or past the duration:
 * exactly `to`. Reduced motion is the caller's branch — the CSS has already
 * snapped, so the JS writes the endpoint directly.
 */
export function evalWidthTween(from: number, to: number, elapsedMs: number): number {
  const total = RESIZE_SPEC.durationMs;
  const raw = elapsedMs / total;
  if (raw >= 1) {
    return to;
  }
  const eased = cubicBezierEval(RESIZE_CURVE, Math.min(Math.max(raw, 0), 1));
  return from + (to - from) * eased;
}

// ---------------------------------------------------------------------------
// Resize drag sampling + edge bounce — `motion.rs:266-323`, `shell.rs:3769`
// ---------------------------------------------------------------------------

/**
 * `motion.rs::ResizeEdge`: which clamp bound (if any) a drag sample sits on.
 * `null` is mid-range.
 */
export type ResizeEdge = "min" | "max" | null;

export interface ResizeSample {
  readonly width: number;
  readonly edge: ResizeEdge;
  /**
   * A NEW edge was hit under a pointer that had not latched it — the one
   * event per held pointer that arms the 5px bounce.
   */
  readonly startsBounce: boolean;
}

/**
 * `motion.rs::resize_drag_sample`: clamp the requested width and report the
 * edge plus whether this sample ARMS a bounce. The `latched` edge is the one
 * the pointer already bounced at; a held pointer at the same edge produces
 * exactly one nudge because the latch is only rearmed by leaving the edge.
 */
export function resizeDragSample(
  requested: number,
  min: number,
  max: number,
  latched: ResizeEdge,
  reducedMotion: boolean,
): ResizeSample {
  const edge = requested <= min ? "min" : requested >= max ? "max" : null;
  return {
    width: Math.min(max, Math.max(min, requested)),
    edge,
    startsBounce: !reducedMotion && edge !== null && edge !== latched,
  };
}

/** `motion.rs::RESIZE_EDGE_NUDGE` — how far past the limit the bounce goes. */
export const RESIZE_EDGE_NUDGE = 5;
/** `motion.rs::RESIZE_EDGE_BOUNCE_MS` — the whole out-and-back pulse. */
export const RESIZE_EDGE_BOUNCE_MS = 220;
/** `motion.rs::RESIZE_EDGE_BOUNCE_OUT_FRACTION` — the outbound share of it. */
export const RESIZE_EDGE_BOUNCE_OUT_FRACTION = 0.32;

/** `t²(3 − 2t)` — zero velocity at both joins of the pulse. */
function smoothstep(t: number): number {
  return t * t * (3 - 2 * t);
}

/**
 * `motion.rs::resize_bounce_offset`: a rounded two-phase pulse over 220 ms —
 * out for the first 32 %, back over the rest, ±5 px depending on the edge.
 * `null` edge or an elapsed outside the window yields 0; reduced motion is
 * enforced by the caller (`eval_resize_edge_bounce` returns 0 there).
 */
export function resizeBounceOffset(edge: ResizeEdge, elapsedMs: number): number {
  if (edge === null) {
    return 0;
  }
  const raw = Math.min(Math.max(elapsedMs / RESIZE_EDGE_BOUNCE_MS, 0), 1);
  const outbound = raw < RESIZE_EDGE_BOUNCE_OUT_FRACTION;
  const phase = outbound
    ? smoothstep(raw / RESIZE_EDGE_BOUNCE_OUT_FRACTION)
    : 1 - smoothstep((raw - RESIZE_EDGE_BOUNCE_OUT_FRACTION) / (1 - RESIZE_EDGE_BOUNCE_OUT_FRACTION));
  const magnitude = phase * RESIZE_EDGE_NUDGE;
  return edge === "min" ? -magnitude : magnitude;
}

/**
 * `shell.rs:173-175` — the vertical seams' hit geometry. The 20 px target is
 * centred on the seam and starts `TITLEBAR_HEIGHT` down, so the titlebar
 * chrome above it stays clickable across an animated pane boundary
 * (`pane_resize_hitboxes_yield_the_titlebar_chrome`).
 */
export const PANE_RESIZE_HITBOX_HALF_WIDTH = 10;
/** `Theme::TITLEBAR_HEIGHT` (`proto/layout.rs:45`), same value as `--rb-titlebar-height`. */
export const TITLEBAR_HEIGHT = 38;
/** `Theme::TITLEBAR_TOP_PAD` (`proto/layout.rs:48`), same value as `--rb-titlebar-top-pad`. */
export const TITLEBAR_TOP_PAD = 4;

// ---------------------------------------------------------------------------
// Titlebar row inset — `tabs.rs::render_session_title_bar`
// ---------------------------------------------------------------------------

/**
 * `shell.rs`'s titlebar constants. The window-control cluster is an OVERLAY on
 * the desktop, not a member of the title row, and the row pads itself past it —
 * which is what lets the identity sit at the sidebar's edge and glide with it.
 */
// The desktop's cluster is `left_0()` + `.px(TITLEBAR_CLUSTER_PAD)`
// (shell.rs:4025-4034); the web's `.titlebar-cluster` is the same shape
// (ticket 66), so this is the cluster's inline padding AND the controls'
// window-space start — and the island's `left(6).right_0()` resolves against
// the 20px-wider padding box that padding creates.
export const TITLEBAR_CLUSTER_PAD = 10;
export const TITLEBAR_CONTROL_GAP = 2;
export const TITLEBAR_GROUP_GAP = 8;
/** `TITLEBAR_IDENTITY_GAP` — SPACE_MD. */
export const TITLEBAR_IDENTITY_GAP = 12;
/** A 24px sidebar trigger, an 8px group gap, then two 24px history buttons. */
export const CLUSTER_BUTTONS_WIDTH = 24 * 3 + TITLEBAR_GROUP_GAP + TITLEBAR_CONTROL_GAP;
/** The new-session `+` budgets one slot so the title never sits under it. */
export const TITLEBAR_ACTION_SLOT_WIDTH = TITLEBAR_GROUP_GAP + 24;
/** The island wrapper's own left inset — `left(6).right_0()` (shell.rs:4027-4028). */
export const TITLEBAR_ISLAND_INSET = 6;
const SPACE_LG = 16;

// ---------------------------------------------------------------------------
// Per-platform cluster geometry — `shell.rs:211-285`, ported whole so the web
// tests can assert the desktop's cases AND the web collapse (no traffic lights,
// no Linux captions: spacer 0, cluster start 10).
// ---------------------------------------------------------------------------

/** Where the cluster starts off macOS traffic lights (`left: fullscreen ? 12 : 88`). */
export function titlebarClusterStart(fullscreen: boolean): number {
  return fullscreen ? 12 : 88;
}

/** The spacer ahead of the cluster exists only to clear traffic lights. */
export function titlebarSpacerWidth(isMacos: boolean, fullscreen: boolean, containerPad: number): number {
  if (!isMacos) {
    return 0;
  }
  return Math.max(titlebarClusterStart(fullscreen) - containerPad, 0);
}

/** A row of `count` caption buttons on the cluster's 24px/2px rhythm. */
export function captionButtonsWidth(count: number): number {
  if (count === 0) {
    return 0;
  }
  return count * 24 + (count - 1) * 2;
}

/** Where the cluster's first button starts, from the window's left edge. */
export function clusterButtonsStart(
  isMacos: boolean,
  fullscreen: boolean,
  linuxLeftCaptions: number,
): number {
  if (isMacos) {
    return titlebarClusterStart(fullscreen);
  }
  if (linuxLeftCaptions > 0) {
    return 10 + captionButtonsWidth(linuxLeftCaptions) + 2;
  }
  return 10;
}

/** Left clearance a full-bleed header needs to start past the overlay cluster. */
export function clusterClearance(
  isMacos: boolean,
  fullscreen: boolean,
  linuxLeftCaptions: number,
  containerPad: number,
): number {
  return Math.max(
    clusterButtonsStart(isMacos, fullscreen, linuxLeftCaptions) +
      CLUSTER_BUTTONS_WIDTH +
      TITLEBAR_GROUP_GAP -
      containerPad,
    0,
  );
}

/**
 * `titlebar_new_session_alpha` (`shell.rs:193-199`): the `+` shows only while
 * an existing chat is selected on the chat route — never on the blank canvas,
 * never in Settings.
 */
export function titlebarNewSessionAlpha(isChatRoute: boolean, hasSelectedChat: boolean): number {
  return isChatRoute && hasSelectedChat ? 1 : 0;
}

/**
 * Where titlebar content may start: past the cluster, plus its identity gap.
 * The browser owns the window, so there are no traffic lights or caption
 * buttons to clear — `cluster_buttons_start` is a flat 10 off macOS.
 */
export const TITLEBAR_CONTENT_START =
  TITLEBAR_CLUSTER_PAD + CLUSTER_BUTTONS_WIDTH + TITLEBAR_IDENTITY_GAP;

/**
 * The title row's left inset — `render_session_title_bar`'s `row_left`.
 *
 * Normally the conversation column's own left edge (`sidebar + 16`), so the
 * identity lines up with the transcript beneath it and GLIDES as the sidebar
 * collapses, clamped so it never slides under the control cluster.
 *
 * In takeover the title hides and the pane's strip owns the whole band, so the
 * inset pulls back 8px LEFT of the sidebar seam: the strip's width is capped
 * to the room left after the row's 8px child gap, and starting 8 early cancels
 * that, landing its first chip exactly on the pane's own gutter.
 */
export function titlebarRowLeft(options: {
  readonly sidebar: number;
  readonly showsNewSession: boolean;
  readonly takeover: boolean;
}): number {
  const plusInset = options.showsNewSession ? TITLEBAR_ACTION_SLOT_WIDTH : 0;
  if (options.takeover) {
    // The − 14 cancels `TITLEBAR_IDENTITY_GAP(12)` minus the strip's own left
    // pad (`tabs.rs:216-218`): in takeover the pane's band owns the row and
    // its first chip must land on the pane's own gutter, not 12px past it.
    const clusterEnd = TITLEBAR_CONTENT_START - TITLEBAR_IDENTITY_GAP + plusInset - 14;
    return Math.max(options.sidebar - 8, clusterEnd);
  }
  return Math.max(options.sidebar + SPACE_LG, TITLEBAR_CONTENT_START + plusInset);
}

/** The titlebar's right inset — `TITLEBAR_ACTION_EDGE_INSET`. */
export const TITLEBAR_EDGE_INSET = 6;
/** One fixed trailing control's slot: a 28px `header_icon_button` (shell.rs:7552). */
const TITLEBAR_TOGGLE_SLOT = 28;
/**
 * `PANEL_TOGGLE_SLOTS` (tabs.rs:48): the two fixed right-edge anchors — the
 * Files toggle and the pane toggle, two 28px `header_icon_button`s. They keep
 * their slots even while the pane is shut (`(files_visible -
 * right_pad).max(PANEL_TOGGLE_SLOTS)`, tabs.rs:59), so every band budgeting
 * the trailing group reserves the PAIR.
 */
export const PANEL_TOGGLE_SLOTS = TITLEBAR_TOGGLE_SLOT * 2;

/**
 * The pane header strip's width — `render_session_title_bar`'s
 * `animated_width`:
 *
 *     ((right_now - pr).min(avail) - 56).max(0)
 *
 * It rides the pane's own animated width, so the strip and the column move as
 * one. The `avail` cap matters: the row's left padding is part of its content
 * box, and a strip wider than what is left after it would overflow and clip at
 * the right edge instead of shrinking. The row's child gaps sit OUTSIDE the
 * strip, so they are budgeted too — one in takeover (the strip alone), three
 * when title, Actions and spacer are present (b1484015's budget). The
 * `PANEL_TOGGLE_SLOTS` pair — the Files toggle and the pane toggle — is
 * budgeted at the strip's right: both anchors keep their 28px slots even
 * while the pane is shut (tabs.rs:59), so the strip's width ends 56 short of
 * the pane's own left edge, exactly where the first anchor's slot starts.
 */
export function titlebarPaneBandWidth(options: {
  readonly viewport: number;
  /** The pane's laid-out width — `right_now`. */
  readonly paneWidth: number;
  readonly rowLeft: number;
  readonly takeover: boolean;
}): number {
  const gapBudget = options.takeover ? TITLEBAR_GROUP_GAP : TITLEBAR_GROUP_GAP * 3;
  const avail = options.viewport - options.rowLeft - TITLEBAR_EDGE_INSET - gapBudget;
  const width = Math.min(options.paneWidth - TITLEBAR_EDGE_INSET, avail);
  return Math.max(0, width - PANEL_TOGGLE_SLOTS);
}

/**
 * The room the project-actions control may claim in the title row
 * (`available_titlebar_width`, tabs.rs): viewport minus the row's left
 * inset, the edge inset, the trailing strip, and the row's three gaps.
 * The control's label reacts to THIS, not the raw viewport (b1484015).
 */
export function titlebarAvailableTitlebarWidth(options: {
  readonly viewport: number;
  readonly rowLeft: number;
  /** The trailing strip's width (0 when the pane is shut). */
  readonly trailingWidth: number;
}): number {
  return Math.max(
    0,
    options.viewport -
      options.rowLeft -
      TITLEBAR_EDGE_INSET -
      options.trailingWidth -
      TITLEBAR_GROUP_GAP * 3,
  );
}

export interface SidebarLayout {
  /** The dragged width, retained across a collapse so reopening restores it. */
  readonly width: number;
  readonly collapsed: boolean;
}

/** The laid-out width: zero while collapsed, as in `shell.rs::sidebar_target`. */
export function sidebarTarget(state: SidebarLayout): number {
  return state.collapsed ? 0 : state.width;
}

export function clampSidebarWidth(width: number): number {
  return clampOr(width, SIDEBAR_MIN, SIDEBAR_MAX, SIDEBAR_DEFAULT);
}

/**
 * The sidebar's persisted geometry, projected out of the settings store.
 *
 * There is no state of its own here: `sidebarWidth`/`sidebarCollapsed` live in
 * `ui-settings.ts` like every other device-local preference, and this class is
 * the narrow view onto them the shell has always used. The projection is
 * cached on the two values it reads, not on the settings snapshot's identity,
 * so `useSyncExternalStore` keeps the same object — and the shell skips the
 * re-render — when some unrelated preference moves.
 */
class SidebarWidthStore {
  #cache: SidebarLayout | null = null;

  getSnapshot = (): SidebarLayout => {
    const settings = uiSettings.getSnapshot();
    const cached = this.#cache;
    if (
      cached !== null &&
      cached.width === settings.sidebarWidth &&
      cached.collapsed === settings.sidebarCollapsed
    ) {
      return cached;
    }
    const next: SidebarLayout = {
      width: settings.sidebarWidth,
      collapsed: settings.sidebarCollapsed,
    };
    this.#cache = next;
    return next;
  };

  subscribe = (listener: () => void): (() => void) => uiSettings.subscribe(listener);

  /** A drag sample: the pointer's x IS the width, clamped to the bounds. */
  setWidth(width: number): void {
    // A drag is a stream of samples — coalesce them into one write.
    uiSettings.update({ sidebarWidth: width, sidebarCollapsed: false }, "debounced");
  }

  toggleCollapsed(): void {
    uiSettings.update({ sidebarCollapsed: !this.getSnapshot().collapsed }, "immediate");
  }

  /** Double-clicking the seam restores the default (`shell.rs:7930`). */
  reset(): void {
    uiSettings.update({ sidebarWidth: SIDEBAR_DEFAULT, sidebarCollapsed: false }, "immediate");
  }
}

export const sidebarLayout = new SidebarWidthStore();

export function useSidebarLayout(): SidebarLayout {
  return useSyncExternalStore(sidebarLayout.subscribe, sidebarLayout.getSnapshot, () => ({
    width: SIDEBAR_DEFAULT,
    collapsed: false,
  }));
}

// ---------------------------------------------------------------------------
// Bottom chrome clearance — the shell pushes, the transcript pads
// ---------------------------------------------------------------------------

/**
 * The measured height of the conversation's bottom chrome stack (status
 * strip + queue panel + composer + footer) — the web peer of the desktop's
 * `set_bottom_clearance` paint-time canvas (transcript.rs:2940). The shell's
 * `ResizeObserver` publishes it; the transcript pads its last row by
 * `clearance + TRANSCRIPT_FADE_BAND + 8` so the timestamp strip clears the
 * chrome the list scrolls under. Deltas ≤ 0.5px are ignored, exactly like the
 * desktop's, so idle layout never feeds back.
 */
class BottomClearanceStore {
  #height = 0;
  readonly #listeners = new Set<() => void>();

  getSnapshot = (): number => this.#height;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  set(height: number): void {
    if (Math.abs(this.#height - height) <= 0.5) {
      return;
    }
    this.#height = height;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

export const bottomClearance = new BottomClearanceStore();

/** The live measured bottom-chrome height (0 until the shell measures). */
export function useBottomClearance(): number {
  return useSyncExternalStore(bottomClearance.subscribe, bottomClearance.getSnapshot, () => 0);
}

/**
 * The window width — the desktop stamps `viewport_width` every render and the
 * width functions above all need it.
 */
function subscribeViewport(listener: () => void): () => void {
  window.addEventListener("resize", listener);
  return () => window.removeEventListener("resize", listener);
}

export function useViewportWidth(): number {
  return useSyncExternalStore(
    subscribeViewport,
    () => window.innerWidth,
    () => SIDEBAR_DEFAULT + CHAT_PANEL_MIN,
  );
}
