/**
 * The composer's route dock — the web port of `crates/ui/src/composer_dock.rs`
 * and `composer_dock/panel_handoff.rs`: one retargetable clock for the shared
 * composer's choreography between the new-thread canvas and an established
 * chat.
 *
 * The whole point of the module (and of ticket 15) is that ONE composer entity
 * glides between the two routes: its wrapper is carried by a critically
 * damped spring on both axes, its width on the same clock, its chrome cross
 * fades on four independently staged channels, and a short fade-through
 * covers the frames where the conversation column's horizontal geometry
 * switches. Everything here is pure geometry/time math driven by the caller's
 * monotonic clock (`performance.now()`), so the desktop's motion tests port
 * 1:1 (`tests/composer-dock.test.ts`).
 *
 * The glide constants live in `crates/proto/src/motion.rs` (`DOCK_GLIDE_*`,
 * motion.rs:476-484) precisely so both clients glide identically; they are
 * transcribed here with their citations.
 */

import { composerTotalHeight, COMPACT_TOTAL_HEIGHT, TEXTAREA_MIN, TEXTAREA_PAD_V, ACTIONS_ROW_HEIGHT, PILL_BORDER_V, TEXTAREA_MAX } from "./composer-flip";

// ---------------------------------------------------------------------------
// Constants (motion.rs:476-484)
// ---------------------------------------------------------------------------

/** Twelve time constants settle within a fraction of a pixel. */
export const DOCK_GLIDE_TIME_CONSTANTS = 12.0;
/** Glide duration (seconds) when docking into an established thread. */
export const DOCK_GLIDE_DOCK_SECONDS = 0.42;
/** Glide duration (seconds) when returning to the new-thread hero. */
export const DOCK_GLIDE_UNDOCK_SECONDS = 0.47;
/** Position epsilon below which the glide is settled. */
export const DOCK_GLIDE_SETTLE_POSITION = 0.0005;
/** Velocity epsilon below which the glide is settled. */
export const DOCK_GLIDE_SETTLE_VELOCITY = 0.005;
/** The panel handoff's fade-through duration (composer_dock.rs:213). */
export const PANEL_HANDOFF_SECONDS = 0.32;

/** `motion::lerp` (motion.rs:430). */
export function lerp(from: number, to: number, t: number): number {
  return from + (to - from) * t;
}

// ---------------------------------------------------------------------------
// `stage` — smoothstep (composer_dock.rs:57-60)
// ---------------------------------------------------------------------------

/** `t²(3 − 2t)` clamped onto [start, end]. */
export function stage(value: number, start: number, end: number): number {
  const t = Math.min(Math.max((value - start) / (end - start), 0), 1);
  return t * t * (3 - 2 * t);
}

// ---------------------------------------------------------------------------
// `Glide` — critically damped motion (composer_dock.rs:23-55)
// ---------------------------------------------------------------------------

/**
 * Critically damped motion: no oscillation, and both position and velocity
 * survive a new target. Twelve time constants settle within a fraction of a
 * pixel over the intended 420/470ms handoff, even across a large window.
 */
export class Glide {
  value: number;
  velocity: number;
  target: number;

  constructor(value: number) {
    this.value = value;
    this.velocity = 0;
    this.target = value;
  }

  advance(target: number, seconds: number, duration: number): void {
    this.target = target;
    const omega = DOCK_GLIDE_TIME_CONSTANTS / duration;
    const displacement = this.value - target;
    const c = this.velocity + omega * displacement;
    const decay = Math.exp(-omega * seconds);
    this.value = target + (displacement + c * seconds) * decay;
    this.velocity = (this.velocity - omega * c * seconds) * decay;
    if (!this.active()) {
      this.value = target;
      this.velocity = 0;
    }
  }

  active(): boolean {
    return (
      Math.abs(this.value - this.target) > DOCK_GLIDE_SETTLE_POSITION ||
      Math.abs(this.velocity) > DOCK_GLIDE_SETTLE_VELOCITY
    );
  }
}

/** `duration(docked)` (composer_dock.rs:314-320) — seconds for the glide. */
export function dockGlideSeconds(docked: boolean): number {
  return docked ? DOCK_GLIDE_DOCK_SECONDS : DOCK_GLIDE_UNDOCK_SECONDS;
}

// ---------------------------------------------------------------------------
// `route_chrome_opacities` (composer.rs:85-92)
// ---------------------------------------------------------------------------

/**
 * Route chrome dissolves around the middle of the shared-element move. The
 * two ramps never overlap, which avoids duplicate picker ids/popovers while
 * still letting their surrounding geometry collapse continuously.
 */
export function routeChromeOpacities(newThreadChrome: number): { newThread: number; session: number } {
  const clamped = Math.min(Math.max(newThreadChrome, 0), 1);
  const newThread = Math.min(Math.max((clamped - 0.5) * 2, 0), 1);
  const session = Math.min(Math.max((1 - clamped - 0.5) * 2, 0), 1);
  return { newThread, session };
}

// ---------------------------------------------------------------------------
// `dock_height` + `dock_clearance_correction` (composer.rs:7488-7500, 7583)
// ---------------------------------------------------------------------------

/**
 * The interpolated pill height across the route. The `76 − 16 = 60` floor on
 * the session side is what lets a short draft in an established chat sit
 * skinnier than the hero's 76px textarea floor.
 */
export function dockHeight(amount: number, contentHeight: number, sessionExpanded: boolean): number {
  const hero = composerTotalHeight(contentHeight);
  const session = sessionExpanded
    ? Math.min(Math.max(contentHeight + TEXTAREA_PAD_V, TEXTAREA_MIN - 16), TEXTAREA_MAX) +
      ACTIONS_ROW_HEIGHT +
      PILL_BORDER_V
    : COMPACT_TOTAL_HEIGHT;
  return lerp(hero, session, Math.min(Math.max(amount, 0), 1));
}

/**
 * The shell reserves the DESTINATION footprint, never the animated height,
 * so the transcript's clearance does not pump during the route change
 * (composer.rs:7583-7589). `strips` are the extra rows riding on the pill
 * (attachment strip etc.).
 */
export function dockClearanceCorrection(
  frame: { readonly docked: boolean },
  contentHeight: number,
  sessionExpanded: boolean,
  strips: number,
  pillHeight: number,
): number {
  return dockHeight(frame.docked ? 1 : 0, contentHeight, sessionExpanded) + strips - pillHeight;
}

// ---------------------------------------------------------------------------
// `bottom_stack_measurement_matches` (shell.rs:837)
// ---------------------------------------------------------------------------

/**
 * `transcript_geometry_ready`: the measured stack must agree with what this
 * frame expects to be in it — one frame of disagreement hides the transcript
 * (opacity 0) so it never flashes under unmeasured chrome.
 */
export function bottomStackMeasurementMatches(measuredHasComposer: boolean, expectedHasComposer: boolean): boolean {
  return measuredHasComposer === expectedHasComposer;
}

// ---------------------------------------------------------------------------
// `Visuals` — the four staged channels (composer_dock.rs:95-148)
// ---------------------------------------------------------------------------

/** The four independently staged visual channels of a dock frame. */
export interface DockVisuals {
  readonly transcript: number;
  readonly selectors: number;
  readonly footer: number;
  readonly dissolve: number;
}

/** `Visuals::settled` (composer_dock.rs:104-112). */
export function dockVisualsSettled(docked: boolean): DockVisuals {
  const value = docked ? 1 : 0;
  return {
    transcript: value,
    selectors: 1 - value,
    footer: value,
    dissolve: value,
  };
}

/**
 * `Visuals::advance` (composer_dock.rs:114-134): each channel blends from its
 * current value toward the settled target over its own staged window. Docking
 * and undocking are direction-specific.
 */
export function dockVisualsAdvance(from: DockVisuals, docked: boolean, time: number): DockVisuals {
  const target = dockVisualsSettled(docked);
  const blend = (a: number, b: number, start: number, end: number): number =>
    lerp(a, b, stage(time, start, end));
  if (docked) {
    return {
      transcript: blend(from.transcript, target.transcript, 0.2, 0.65),
      selectors: blend(from.selectors, target.selectors, 0.55, 0.78),
      footer: blend(from.footer, target.footer, 0.78, 1.0),
      dissolve: blend(from.dissolve, target.dissolve, 0.06, 0.88),
    };
  }
  // On return, release thread chrome first; unfold the hero behind the
  // rising input and restore destination selectors near arrival.
  return {
    transcript: blend(from.transcript, target.transcript, 0.0, 0.25),
    selectors: blend(from.selectors, target.selectors, 0.5, 0.95),
    footer: blend(from.footer, target.footer, 0.0, 0.18),
    dissolve: blend(from.dissolve, target.dissolve, 0.08, 0.85),
  };
}

/**
 * `Visuals::return_from_panel` (composer_dock.rs:136-147) — the short
 * fade-through's own clock: destination controls must arrive with the input,
 * not trail the longer vertical-glide schedule.
 */
export function dockVisualsReturnFromPanel(from: DockVisuals, time: number): DockVisuals {
  return {
    transcript: from.transcript * (1 - stage(time, 0.0, 0.18)),
    footer: from.footer * (1 - stage(time, 0.0, 0.18)),
    selectors: lerp(from.selectors, 1, stage(time, 0.26, 0.85)),
    // The mask follows the actual surface. Keep it hidden through the 0.22
    // horizontal geometry switch, then reveal both together.
    dissolve: from.dissolve * (1 - stage(time, 0.26, 0.8)),
  };
}

// ---------------------------------------------------------------------------
// `PanelHandoff` (panel_handoff.rs:4-52)
// ---------------------------------------------------------------------------

/**
 * A fade-through when navigation changes the conversation's horizontal frame
 * — the right pane opening/closing across a route change. Ordinary resizing
 * and same-column navigation never fade: the handoff only arms when the
 * DOCKED flag flips while the column width differs by more than 0.5px (or a
 * handoff is already running).
 */
export class PanelHandoff {
  #previous: { docked: boolean; width: number } | null = null;
  #started: number | null = null;
  #fromOpacity = 1;
  progress: number | null = null;

  opacity(): number {
    if (this.progress === null) {
      return 1;
    }
    const p = this.progress;
    const ease = (value: number, start: number, end: number): number => {
      const t = Math.min(Math.max((value - start) / (end - start), 0), 1);
      return t * t * (3 - 2 * t);
    };
    return this.#fromOpacity * (1 - ease(p, 0, 0.18)) + ease(p, 0.26, 1);
  }

  /** Sample the column's (docked, width) state; returns whether a handoff is live. */
  sample(docked: boolean, width: number, enabled: boolean, nowMs: number, durationSeconds: number): boolean {
    if (!enabled) {
      this.#previous = null;
      this.#started = null;
      this.progress = null;
      this.#fromOpacity = 1;
      return false;
    }
    if (
      this.#previous !== null &&
      this.#previous.docked !== docked &&
      (Math.abs(this.#previous.width - width) > 0.5 || this.#started !== null)
    ) {
      this.#fromOpacity = this.opacity();
      this.#started = nowMs;
    }
    this.#previous = { docked, width };
    if (this.#started !== null) {
      const p = (nowMs - this.#started) / 1000 / durationSeconds;
      this.progress = p < 1 ? p : null;
      if (this.progress === null) {
        this.#started = null;
      }
    } else {
      this.progress = null;
    }
    return this.progress !== null;
  }
}

// ---------------------------------------------------------------------------
// `DockFrame` + `DockState` (composer_dock.rs:62-312)
// ---------------------------------------------------------------------------

/** The frame the shell hands the composer each paint. */
export interface DockFrame {
  /** Canonical position: zero is the hero, one is the established thread. */
  readonly amount: number;
  readonly docked: boolean;
  readonly active: boolean;
  readonly visuals: DockVisuals;
}

/** `DockFrame::settled` (composer_dock.rs:71-79). */
export function dockFrameSettled(docked: boolean): DockFrame {
  return {
    amount: docked ? 1 : 0,
    docked,
    active: false,
    visuals: dockVisualsSettled(docked),
  };
}

// ---------------------------------------------------------------------------
// The hero layer's mount rule (shell.rs:5883, ticket 35)
// ---------------------------------------------------------------------------

/**
 * `(!has_selection || dock_frame.active)` — the new-thread hero layer's
 * mount rule, decided from the frame ticked in the SAME render (the tick at
 * shell.rs:5865-5868 precedes the layer at 5883): the layer stays mounted
 * while the dock is still dissolving one away, so no painted frame of a
 * route change exists with the layer — and the artwork state inside it —
 * absent. The phone layer mounts the hero too (ticket 53's amendment to
 * spec decision 5) — the web caller passes the rule through unchanged.
 */
export function heroLayerMounted(hasSelection: boolean, frame: { readonly active: boolean }): boolean {
  return !hasSelection || frame.active;
}

/** Equality on the frame's observable fields (React render guard). */
export function dockFrameEquals(a: DockFrame, b: DockFrame): boolean {
  return (
    a.amount === b.amount &&
    a.docked === b.docked &&
    a.active === b.active &&
    a.visuals.transcript === b.visuals.transcript &&
    a.visuals.selectors === b.visuals.selectors &&
    a.visuals.footer === b.visuals.footer &&
    a.visuals.dissolve === b.visuals.dissolve
  );
}

/**
 * The dock's mutable state — the web peer of the desktop's `SharedDock`
 * `Rc<RefCell<DockState>>`. The caller ticks it per frame (`tick`), samples
 * the column's horizontal frame (`observePane`), and runs the prepaint-side
 * glide of the composer's wrapper (`prepaint`). `nowMs` is a monotonic
 * millisecond clock; durations are seconds exactly as on the desktop.
 */
export class DockState {
  readonly pane = new PanelHandoff();
  #phase = new Glide(0);
  #lastFrameMs: number | null = null;
  #position: { x: Glide; y: Glide } | null = null;
  #lastGeometryMs: number | null = null;
  #lastDocked = false;
  #moving = false;
  #width: Glide | null = null;
  #lastWidthFrameMs: number | null = null;
  #routeChanged = false;
  #choreography: { startedMs: number; from: DockVisuals } | null = null;
  #panelReturn = false;
  #panelDeparture = false;
  #columnWidth: number | null = null;
  #departingColumnWidth: number | null = null;
  frame: DockFrame = dockFrameSettled(false);

  /**
   * `DockState::transcript_width` (composer_dock.rs:195-204): retained
   * transcript pixels belong to the source column — letting them reflow into
   * the hero's wider layout before fading creates an exit flash.
   */
  transcriptWidth(target: number, docked: boolean, panelHandoff: boolean): number {
    if (!docked && this.frame.docked && panelHandoff) {
      this.#departingColumnWidth = this.#columnWidth;
    }
    if (docked || !panelHandoff) {
      this.#departingColumnWidth = null;
    }
    this.#columnWidth = target;
    return this.#departingColumnWidth ?? target;
  }

  /** `observe_pane` — feed the column's horizontal frame; returns handoff-live. */
  observePane(docked: boolean, target: number, enabled: boolean, nowMs: number): boolean {
    return this.pane.sample(docked, target, enabled, nowMs, PANEL_HANDOFF_SECONDS);
  }

  /** The column's fade-through opacity (1 when no handoff is live). */
  opacity(): number {
    return this.pane.opacity();
  }

  /** The handoff's live progress, or null when idle. */
  paneProgress(): number | null {
    return this.pane.progress;
  }

  /**
   * `DockState::layout_width` (composer_dock.rs:220-241): the composer's own
   * width glides on the same clock — EXCEPT inside the handoff's invisible
   * interval (`progress >= 0.22`) where it snaps to the new geometry.
   */
  layoutWidth(target: number, reduced: boolean, nowMs: number): number {
    const dt =
      this.#routeChanged || this.#lastWidthFrameMs === null
        ? 0
        : Math.max(nowMs - this.#lastWidthFrameMs, 0) / 1000;
    this.#lastWidthFrameMs = nowMs;
    if (this.#width === null) {
      this.#width = new Glide(target);
    }
    const progress = this.pane.progress;
    if (progress !== null) {
      // Change horizontal geometry only inside the invisible interval.
      if (progress >= 0.22) {
        this.#width = new Glide(target);
      }
    } else if (reduced || (!this.frame.active && !this.#moving)) {
      this.#width = new Glide(target);
    } else {
      this.#width.advance(target, dt, dockGlideSeconds(this.frame.docked));
    }
    return Math.max(this.#width.value, 0);
  }

  /**
   * `DockState::tick` (composer_dock.rs:243-311): advance the phase glide and
   * the choreography clock; returns the frame for this paint.
   */
  tick(docked: boolean, reduced: boolean, nowMs: number): DockFrame {
    this.#routeChanged = docked !== this.frame.docked;
    if (this.#routeChanged || reduced) {
      this.#panelReturn = !reduced && !docked && this.pane.progress !== null;
      this.#panelDeparture = !reduced && docked && this.pane.progress !== null;
    }
    const target = docked ? 1 : 0;
    if (reduced || this.#lastFrameMs === null || this.#position === null) {
      this.#phase = new Glide(target);
      this.#choreography = null;
    } else {
      // A click after an idle window is the START of the new motion, not
      // elapsed animation time. Keep the last painted velocity.
      const dt = docked !== this.frame.docked ? 0 : Math.max(nowMs - this.#lastFrameMs, 0) / 1000;
      this.#phase.advance(target, dt, dockGlideSeconds(docked));
      if (this.#routeChanged) {
        // Capture the exact previous visual state on interruption.
        this.#choreography = { startedMs: nowMs, from: this.frame.visuals };
      }
    }
    this.#lastFrameMs = nowMs;
    let visuals: DockVisuals;
    if (this.#choreography !== null) {
      const { startedMs, from } = this.#choreography;
      const total = this.#panelReturn ? PANEL_HANDOFF_SECONDS : dockGlideSeconds(docked);
      const time = (nowMs - startedMs) / 1000 / total;
      if (time >= 1) {
        this.#choreography = null;
      }
      visuals = this.#panelReturn
        ? dockVisualsReturnFromPanel(from, time)
        : dockVisualsAdvance(from, docked, time);
      if (this.#panelDeparture) {
        const panelTime = (nowMs - startedMs) / 1000 / PANEL_HANDOFF_SECONDS;
        visuals = {
          ...visuals,
          dissolve: lerp(from.dissolve, 1, stage(panelTime, 0.0, 0.18)),
        };
      }
    } else {
      visuals = dockVisualsSettled(docked);
    }
    if (this.#panelReturn) {
      const amount =
        this.pane.progress !== null && this.pane.progress < 0.22 ? this.frame.amount : 0;
      // Keep the retargetable state aligned with what was painted so a
      // reversal cannot revive the old, longer height animation.
      this.#phase = new Glide(amount);
    }
    this.frame = {
      amount: Math.min(Math.max(this.#phase.value, 0), 1),
      docked,
      active: this.#phase.active() || this.#choreography !== null,
      visuals,
    };
    return this.frame;
  }

  /**
   * `DockedComposer::prepaint` (composer_dock.rs:368-427): the wrapper stays
   * in its layout slot; only its painted origin moves. `bounds` is the slot's
   * measured rect (window space), `viewportHeight` the window's height.
   * Returns the translate offset to apply and whether another frame is
   * needed.
   */
  prepaint(
    bounds: { readonly left: number; readonly top: number; readonly height: number },
    viewportHeight: number,
    reduced: boolean,
    nowMs: number,
  ): { readonly dx: number; readonly dy: number; readonly moving: boolean } {
    const docked = this.frame.docked;
    const x = bounds.left;
    // Anchor by the top of the input surface, not its shrinking bottom.
    const y = docked ? bounds.top : (viewportHeight - bounds.height) * 0.5 + 8;
    const dt =
      this.#lastDocked !== docked || this.#lastGeometryMs === null
        ? 0
        : Math.max(nowMs - this.#lastGeometryMs, 0) / 1000;
    this.#lastGeometryMs = nowMs;
    this.#moving = this.#moving || this.#lastDocked !== docked || this.frame.active;
    this.#lastDocked = docked;
    const handoff = this.pane.progress;
    if (this.#position === null) {
      this.#position = { x: new Glide(x), y: new Glide(y) };
    }
    if (handoff !== null) {
      if (handoff >= 0.22) {
        // Snap both axes to the new position with a travel offset decaying
        // over the rest of the handoff.
        const travel = docked ? 12 : 8;
        this.#position = {
          x: new Glide(x),
          y: new Glide(y + travel * (1 - stage(handoff, 0.22, 1.0))),
        };
      }
    } else if (reduced || !this.#moving) {
      this.#position = { x: new Glide(x), y: new Glide(y) };
    } else {
      this.#position.x.advance(x, dt, dockGlideSeconds(docked));
      this.#position.y.advance(y, dt, dockGlideSeconds(docked));
    }
    const position = this.#position;
    const dx = position.x.value - x;
    const dy = position.y.value - bounds.top;
    const unsettled =
      Math.abs(position.x.value - x) > 0.1 ||
      Math.abs(position.y.value - y) > 0.1 ||
      Math.abs(position.x.velocity) > 1 ||
      Math.abs(position.y.velocity) > 1;
    this.#moving =
      !reduced &&
      (unsettled ||
        this.frame.active ||
        (this.#width !== null && this.#width.active()));
    return { dx, dy, moving: this.#moving };
  }
}

// ---------------------------------------------------------------------------
// `FlipMorph::new_thread_transition` (composer.rs:5826-5875, ticket §2.5)
// ---------------------------------------------------------------------------

import type { FlipMorph } from "./composer-flip";

/**
 * The 420 ms `motion::NEW_THREAD_TRANSITION` (`EASE_RESORT` =
 * `EASE_OUT_QUINT` = cubic-bezier(0.22, 1, 0.36, 1)) height morph that arms
 * when the composer's draft swaps across the route boundary. On the web the
 * shell's dock frame owns the height while a frame is active (the desktop's
 * `set_dock_frame` kills both morphs); this constructor exists so the
 * morph's pure timeline (armed when no dock frame drives the height —
 * reduced-motion snaps aside) is the same object the desktop tests pin.
 */
export function newThreadTransitionMorph(from: number, startMs: number): FlipMorph {
  return { from, startMs, spec: "newThreadTransition" };
}
