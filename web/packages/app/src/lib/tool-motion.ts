/**
 * The tool group's motion + geometry model — a port of the desktop's
 * `FoldState` / `ToolGroupReveal` and the reveal/connector/shimmer helpers
 * (`crates/ui/src/transcript.rs:2094-2248`, `:2161-2218`), plus the activity
 * rail's pure path builders (`activity_branch_points`/`activity_ribbon`,
 * `:2183-2218`, `:7306-7324`).
 *
 * Pure by design: every progress function takes `now` and returns a number,
 * so the unit tests can drive the stagger and the arc-length cut directly.
 * The per-instance state (folds, reveals, blob fetches) lives in
 * `ToolGroupMotionStore` below — one instance per transcript SURFACE (the
 * web's transcript entity), because a virtualized row scrolling back into
 * view is a remount that must find its fold where it left it.
 */

import { motion } from "@zeron/theme";
import type { ChatArrivalWindow } from "./chat-arrival";
import { blobDetail, isSpawnLink, toolGroupCollapses, type ToolDetail, type TranscriptRow } from "./transcript";

// ---------------------------------------------------------------------------
// Catalog curves (proto/motion.rs:215-237) — solved locally so this module
// stays free of component-layer imports.
// ---------------------------------------------------------------------------

const EASE_OUT: readonly [number, number, number, number] = motion.curves.easeOut ?? [0, 0, 0.58, 1];
const EASE_OUT_EXPO: readonly [number, number, number, number] =
  motion.curves.easeOutExpo ?? [0.16, 1, 0.3, 1];
const EASE_OUT_QUINT: readonly [number, number, number, number] =
  motion.curves.easeOutQuint ?? [0.22, 1, 0.36, 1];

/** Solve one point of a CSS cubic-bezier (as browsers evaluate transitions). */
function evalCurve(curve: readonly [number, number, number, number], progress: number): number {
  const [x1, y1, x2, y2] = curve;
  if (progress <= 0) {
    return 0;
  }
  if (progress >= 1) {
    return 1;
  }
  const x = (u: number): number => 3 * x1 * (1 - u) * (1 - u) * u + 3 * x2 * (1 - u) * u * u + u ** 3;
  const dx = (u: number): number =>
    3 * x1 * (1 - u) * (1 - 3 * u) + 3 * x2 * u * (2 - 3 * u) + 3 * u * u;
  const y = (u: number): number => 3 * y1 * (1 - u) * (1 - u) * u + 3 * y2 * (1 - u) * u * u + u ** 3;
  let u = progress;
  for (let i = 0; i < 8; i += 1) {
    const err = x(u) - progress;
    if (Math.abs(err) < 1e-6) {
      return y(u);
    }
    const slope = dx(u);
    if (Math.abs(slope) < 1e-6) {
      break;
    }
    u -= err / slope;
  }
  let low = 0;
  let high = 1;
  u = progress;
  for (let i = 0; i < 24; i += 1) {
    const err = x(u) - progress;
    if (Math.abs(err) < 1e-6) {
      break;
    }
    if (err > 0) {
      high = u;
    } else {
      low = u;
    }
    u = (low + high) / 2;
  }
  return y(u);
}

const clamp01 = (value: number): number => Math.min(1, Math.max(0, value));
const lerp = (from: number, to: number, t: number): number => from + (to - from) * t;

// ---------------------------------------------------------------------------
// Motion specs (transcript.rs:123-136, :127-129)
// ---------------------------------------------------------------------------

/** `TOOL_FOLD` — 140ms `EASE_OUT`; the group and chip-detail fold tween. */
export const TOOL_FOLD_MS = 140;
/** `TOOL_ROW_REVEAL` — 360ms `EASE_OUT_EXPO`; the row height clip. */
export const TOOL_ROW_REVEAL_MS = 360;
/** `TOOL_CONNECTOR_REVEAL` — 480ms `EASE_OUT_QUINT`; the tree draw. */
export const TOOL_CONNECTOR_REVEAL_MS = 480;
/** `TOOL_FIRST_ROW_DELAY_MS` — a NEW group's first row waits this long. */
export const TOOL_FIRST_ROW_DELAY_MS = 90;
/** `TOOL_ROW_STAGGER_MS` — per-arrival stagger inside one group. */
export const TOOL_ROW_STAGGER_MS = 65;
/** `TOOL_GROUP_SHIMMER_DURATION` — the title sweep period. */
export const TOOL_GROUP_SHIMMER_DURATION_MS = 3400;
/** `TOOL_GROUP_SHIMMER_HALF_WIDTH` — the highlight shoulder, in title widths. */
export const TOOL_GROUP_SHIMMER_HALF_WIDTH = 0.36;
/** `FOLD_TWEEN_WINDOW` — how long a toggle keeps its tween armed (:170). */
export const FOLD_TWEEN_WINDOW_MS = 400;

// ---------------------------------------------------------------------------
// Activity-rail geometry (transcript.rs:110-116)
// ---------------------------------------------------------------------------

/** `ACTIVITY_GUTTER_WIDTH` — the rail column's width. */
export const ACTIVITY_GUTTER_WIDTH = 48;
/** `ACTIVITY_TEXT_GAP` — the card's left margin off the rail. */
export const ACTIVITY_TEXT_GAP = 8;
/** `ACTIVITY_TRUNK_X` — the trunk's x inside the gutter. */
export const ACTIVITY_TRUNK_X = 12.5;
/** `ACTIVITY_BEND_RADIUS` — the elbow's corner radius. */
export const ACTIVITY_BEND_RADIUS = 6;
/** `ACTIVITY_BRANCH_END_X` — where the branch's straight leg ends. */
export const ACTIVITY_BRANCH_END_X = 28;
/** `ACTIVITY_ICON_LEFT` — the tool glyph's x inside the gutter. */
export const ACTIVITY_ICON_LEFT = 32;
/** `ACTIVITY_ICON_SIZE` — the tool glyph's edge. */
export const ACTIVITY_ICON_SIZE = 16;

// ---------------------------------------------------------------------------
// Fold + reveal state (transcript.rs:2094-2133)
// ---------------------------------------------------------------------------

/**
 * One fold (group or chip detail) — the desktop's `FoldState`. `open: null`
 * follows the auto-open rule; a user click pins it. `from` is the height at
 * the moment of the toggle; the destination is always the CURRENT target so
 * content growth after a toggle snaps instead of replaying a stale tween.
 * `toggledAt` is only honored within `FOLD_TWEEN_WINDOW` of the click for
 * mount retention (a virtualized row scrolling back into view is a remount;
 * the row-height lerp itself saturates past 140ms, so an aged tween renders
 * its endpoint — no flash).
 */
export interface FoldState {
  readonly open: boolean | null;
  readonly epoch: number;
  readonly from: number;
  readonly toggledAt: number | null;
  readonly disclosureAt: number | null;
}

const DEFAULT_FOLD: FoldState = { open: null, epoch: 0, from: 0, toggledAt: null, disclosureAt: null };

/**
 * Reveal epochs for one live ordinary tool group. `null` starts/`headerStartedAt`
 * mean the row was already present when this transcript attached (or has
 * finished revealing), so replaying history and scrolling a virtualized row
 * back into view stay completely still.
 */
export interface ToolGroupReveal {
  headerStartedAt: number | null;
  starts: (number | null)[];
  shimmerStartedAt: number | null;
  renderedOpen: boolean | null;
  renderedHeight: number;
}

const DEFAULT_REVEAL: ToolGroupReveal = {
  headerStartedAt: null,
  starts: [],
  shimmerStartedAt: null,
  renderedOpen: null,
  renderedHeight: 0,
};

// ---------------------------------------------------------------------------
// Progress helpers (transcript.rs:2135-2248)
// ---------------------------------------------------------------------------

/** `tool_row_reveal_progress` (:2135) — 1.0 for null starts / reduced motion. */
export function toolRowRevealProgress(start: number | null, now: number, reduceMotion: boolean): number {
  if (start === null || reduceMotion) {
    return 1;
  }
  const raw = Math.max(0, now - start) / TOOL_ROW_REVEAL_MS;
  return evalCurve(EASE_OUT_EXPO, clamp01(raw));
}

/** `tool_connector_reveal_progress` (:2144) — 480ms `EASE_OUT_QUINT`. */
export function toolConnectorRevealProgress(
  start: number | null,
  now: number,
  reduceMotion: boolean,
): number {
  if (start === null || reduceMotion) {
    return 1;
  }
  const raw = Math.max(0, now - start) / TOOL_CONNECTOR_REVEAL_MS;
  return evalCurve(EASE_OUT_QUINT, clamp01(raw));
}

/**
 * `tool_disclosure_progress` (:2220) — the chevron rotation clock. When
 * `disclosureAt` is null it snaps to the open endpoint; reduced motion snaps
 * both ways (the caller passes the resolved value).
 */
export function toolDisclosureProgress(open: boolean, fold: FoldState | null, now: number): number {
  const at = fold?.disclosureAt ?? null;
  if (at === null) {
    return open ? 1 : 0;
  }
  const raw = clamp01((now - at) / TOOL_FOLD_MS);
  const progress = evalCurve(EASE_OUT, raw);
  return open ? progress : 1 - progress;
}

/** `TOOL_FOLD`'s eased open/close progress for a height tween. */
export function toolFoldProgress(fold: FoldState | null, now: number): number | null {
  const at = fold?.toggledAt ?? null;
  if (at === null) {
    return null;
  }
  const raw = clamp01((now - at) / TOOL_FOLD_MS);
  return evalCurve(EASE_OUT, raw);
}

/**
 * `tool_connector_parts` (:2161) — split one arrival into a continuous tree
 * draw. The branch overlaps the end of the incoming phase so there is no dead
 * frame at the bend.
 */
export function toolConnectorParts(progress: number, hasPredecessor: boolean): { incoming: number; branch: number } {
  const p = clamp01(progress);
  const [incomingStart, incomingEnd, branchStart] = hasPredecessor ? [0.45, 0.72, 0.68] : [0, 0.62, 0.58];
  const incoming = clamp01((p - incomingStart) / (incomingEnd - incomingStart));
  const branch = clamp01((p - branchStart) / (1 - branchStart));
  return { incoming, branch };
}

/**
 * `tool_connector_continuation` (:2175) — the outgoing trunk of row *i* is
 * driven by row *i+1*'s progress; visually it belongs to the present row,
 * temporally to the next row's arrival.
 */
export function toolConnectorContinuation(nextProgress: number | null): number {
  return nextProgress === null ? 0 : clamp01(nextProgress / 0.45);
}

/** `tool_title_shimmer_amount` (:2237) — the narrow highlight's strength at x. */
export function toolTitleShimmerAmount(x: number, phase: number): number {
  const primaryCenter = -2.5 + clamp01(phase) * 6;
  let best = 0;
  for (let copy = -2; copy <= 2; copy += 1) {
    const center = primaryCenter + copy * 3;
    const amount = clamp01(1 - Math.abs(x - center) / TOOL_GROUP_SHIMMER_HALF_WIDTH);
    if (amount > best) {
      best = amount;
    }
  }
  return best;
}

/** `tool_title_shimmer_phase` (:2245) — `fract(elapsed / 3400ms)`. */
export function toolTitleShimmerPhase(elapsed: number): number {
  const ratio = Math.max(0, elapsed) / TOOL_GROUP_SHIMMER_DURATION_MS;
  return ratio - Math.floor(ratio);
}

// ---------------------------------------------------------------------------
// Activity-rail path builders (transcript.rs:2183-2218, :7250-7324)
// ---------------------------------------------------------------------------

/** A point in gutter-local coordinates. */
export interface RailPoint {
  readonly x: number;
  readonly y: number;
}

/**
 * `activity_branch_points` (:2183) — the elbow + straight leg, 25 samples of
 * the quadratic corner plus the final `(15.5, 6)` leg end. For `progress < 1`
 * the polyline is cut by ARC LENGTH (last segment lerped) so changing the
 * branch length introduces no speed jump at the elbow/leg junction.
 */
export function activityBranchPoints(progress: number): RailPoint[] {
  const path: RailPoint[] = [];
  for (let step = 0; step <= 24; step += 1) {
    const t = step / 24;
    path.push({
      x: ACTIVITY_BEND_RADIUS * t * t,
      y: ACTIVITY_BEND_RADIUS * (2 * t - t * t),
    });
  }
  path.push({ x: ACTIVITY_BRANCH_END_X - ACTIVITY_TRUNK_X, y: ACTIVITY_BEND_RADIUS });
  const p = clamp01(progress);
  if (p >= 1) {
    return path;
  }
  const lengths: number[] = [];
  let total = 0;
  for (let ix = 0; ix + 1 < path.length; ix += 1) {
    const len = Math.hypot(path[ix + 1]!.x - path[ix]!.x, path[ix + 1]!.y - path[ix]!.y);
    lengths.push(len);
    total += len;
  }
  let remaining = total * p;
  const visible: RailPoint[] = [path[0]!];
  for (let ix = 0; ix < lengths.length; ix += 1) {
    if (remaining <= 0) {
      break;
    }
    const length = lengths[ix]!;
    const t = Math.min(remaining / length, 1);
    visible.push({
      x: lerp(path[ix]!.x, path[ix + 1]!.x, t),
      y: lerp(path[ix]!.y, path[ix + 1]!.y, t),
    });
    remaining -= length;
  }
  return visible;
}

/**
 * `activity_ribbon` (:7306) — a 1px-wide closed contour around the polyline,
 * offsetting each point ±0.5 along the local normal (computed from the
 * neighbours), walking the left side forward and the right side backward,
 * then closing. Returned as an SVG `d` string.
 */
export function activityRibbon(points: readonly RailPoint[]): string {
  if (points.length === 0) {
    return "";
  }
  const left: RailPoint[] = [];
  const right: RailPoint[] = [];
  for (let ix = 0; ix < points.length; ix += 1) {
    const a = points[Math.max(0, ix - 1)]!;
    const b = points[Math.min(points.length - 1, ix + 1)]!;
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const length = Math.max(Math.hypot(dx, dy), 0.0001);
    const nx = (-dy / length) * 0.5;
    const ny = (dx / length) * 0.5;
    const p = points[ix]!;
    left.push({ x: p.x + nx, y: p.y + ny });
    right.push({ x: p.x - nx, y: p.y - ny });
  }
  const d: string[] = [];
  d.push(`M ${left[0]!.x} ${left[0]!.y}`);
  for (let ix = 1; ix < left.length; ix += 1) {
    d.push(`L ${left[ix]!.x} ${left[ix]!.y}`);
  }
  for (let ix = right.length - 1; ix >= 0; ix -= 1) {
    d.push(`L ${right[ix]!.x} ${right[ix]!.y}`);
  }
  d.push("Z");
  return d.join(" ");
}

/** The rail's incoming-trunk endpoints, computed like the desktop paint (:7259). */
function trunkPoints(
  bendRowHeight: number,
  canvasHeight: number,
  incomingReveal: number,
  continues: boolean,
  continuationReveal: number,
): RailPoint[] | null {
  if (incomingReveal <= 0) {
    return null;
  }
  const x = ACTIVITY_TRUNK_X;
  const branchY = bendRowHeight / 2;
  const bendY = branchY - ACTIVITY_BEND_RADIUS;
  let bottomY = (bendRowHeight / 2 - ACTIVITY_BEND_RADIUS) * incomingReveal;
  if (incomingReveal >= 1 && continues && continuationReveal > 0) {
    const continuationHeight = Math.max(canvasHeight - (bendRowHeight / 2 - ACTIVITY_BEND_RADIUS), 0);
    bottomY = bendY + continuationHeight * continuationReveal;
  }
  return [
    { x, y: 0 },
    { x, y: bottomY },
  ];
}

/**
 * The ONE `d` for a row's rail: both contours (trunk + branch) in a single
 * path, painted with the non-zero fill rule so their intersection blends
 * once — the desktop unions the ribbons into one fill before painting
 * (stroke tessellation would double-blend the fork; test:
 * `connector_intersection_is_tessellated_only_once`). `bendRowHeight` is the
 * row's compact height (the elbow's band); `canvasHeight` is the row's FULL
 * height — the trunk continues down alongside an expanded detail body.
 */
export function railPath(input: {
  readonly bendRowHeight: number;
  readonly canvasHeight: number;
  readonly hasPredecessor: boolean;
  readonly continues: boolean;
  readonly connectorReveal: number;
  readonly continuationReveal: number;
}): string | null {
  const { incoming, branch } = toolConnectorParts(input.connectorReveal, input.hasPredecessor);
  const contours: string[] = [];
  const trunk = trunkPoints(
    input.bendRowHeight,
    input.canvasHeight,
    incoming,
    input.continues,
    input.continuationReveal,
  );
  if (trunk !== null) {
    contours.push(activityRibbon(trunk));
  }
  if (branch > 0) {
    const points = activityBranchPoints(branch).map((p) => ({
      x: ACTIVITY_TRUNK_X + p.x,
      y: input.bendRowHeight / 2 - ACTIVITY_BEND_RADIUS + p.y,
    }));
    contours.push(activityRibbon(points));
  }
  if (contours.length === 0) {
    return null;
  }
  return contours.join(" ");
}

// ---------------------------------------------------------------------------
// The shared reveal clock (ticket 59, audit fix plan P5)
// ---------------------------------------------------------------------------

/** What one subscribing row does with a frame's timestamp (`setNow`). */
export type RevealClockListener = (now: number) => void;

/** Constructor seams — injectable so the unit tests drive the clock headless. */
export interface ToolRevealClockSeams {
  readonly schedule?: (callback: () => void) => number;
  readonly cancel?: (handle: number) => void;
  readonly now?: () => number;
  readonly reduced?: () => boolean;
}

let reducedQuery: MediaQueryList | null = null;

/** `prefers-reduced-motion` as a live read (cached query, node-safe). */
const prefersReducedMotion = (): boolean => {
  if (typeof globalThis.matchMedia !== "function") {
    return false;
  }
  reducedQuery ??= globalThis.matchMedia("(prefers-reduced-motion: reduce)");
  return reducedQuery.matches;
};

/**
 * The ONE rAF clock behind every tool-group row's reveal/fold tween. Each
 * `ToolGroupRow` used to run its own `requestAnimationFrame` + `setNow` loop
 * while its motion was active, so a live reveal re-rendered every group row
 * from N independent loops; this clock shares the loop — the first
 * subscriber arms it, the last unsubscribe stops it.
 *
 * Per-row TIMINGS are preserved exactly: every subscriber is handed the same
 * `performance.now()` a per-row loop would have sampled that frame, and each
 * row still computes its own progress from that timestamp — only the loop is
 * shared, never the tween state.
 *
 * `prefers-reduced-motion` never rides the loop: rows gate their
 * `motionActive` off the same media query, so nothing subscribes under
 * reduce; the two reduced arms here are the safety net for a flip landing
 * between a row's render and its effect — a subscribe under reduce delivers
 * ONE immediate tick (the snap frame; the row renders its endpoint and
 * unsubscribes), and a flip mid-flight makes the current frame the last.
 */
export class ToolRevealClock {
  readonly #listeners = new Set<RevealClockListener>();
  readonly #schedule: (callback: () => void) => number;
  readonly #cancel: (handle: number) => void;
  readonly #now: () => number;
  readonly #reduced: () => boolean;
  #raf = 0;
  #running = false;

  constructor(seams: ToolRevealClockSeams = {}) {
    this.#schedule = seams.schedule ?? ((callback) => requestAnimationFrame(callback));
    this.#cancel = seams.cancel ?? ((handle) => cancelAnimationFrame(handle));
    this.#now = seams.now ?? (() => performance.now());
    this.#reduced = seams.reduced ?? prefersReducedMotion;
  }

  /** True while the shared loop is armed (the start/stop test seam). */
  isRunning(): boolean {
    return this.#running;
  }

  subscribe = (listener: RevealClockListener): (() => void) => {
    this.#listeners.add(listener);
    if (this.#listeners.size === 1) {
      this.#arm();
    }
    return () => {
      this.#listeners.delete(listener);
      if (this.#listeners.size === 0) {
        this.#disarm();
      }
    };
  };

  #arm(): void {
    if (this.#reduced()) {
      // The snap: one synchronous tick, no loop — the row renders its
      // endpoint and unsubscribes on its own.
      this.#deliver(this.#now());
      return;
    }
    this.#running = true;
    this.#raf = this.#schedule(this.#tick);
  }

  #disarm(): void {
    if (!this.#running) {
      return;
    }
    this.#running = false;
    this.#cancel(this.#raf);
    this.#raf = 0;
  }

  #tick = (): void => {
    if (!this.#running) {
      return;
    }
    this.#deliver(this.#now());
    if (!this.#running) {
      // The last listener unsubscribed mid-dispatch and stopped the clock.
      return;
    }
    if (this.#reduced()) {
      // Reduced motion flipped mid-flight: this frame is the snap, and it
      // is the last — the rows render their endpoints and unsubscribe.
      this.#running = false;
      this.#raf = 0;
      return;
    }
    this.#raf = this.#schedule(this.#tick);
  };

  #deliver(now: number): void {
    // A copy: a listener may unsubscribe (or subscribe) inside its tick.
    for (const listener of [...this.#listeners]) {
      listener(now);
    }
  }
}

/** The app's ONE shared reveal clock — every surface's rows ride this loop. */
export const toolRevealClock = new ToolRevealClock();

// ---------------------------------------------------------------------------
// The per-instance state store (transcript.rs:2588-2731 fields)
// ---------------------------------------------------------------------------

/** `BlobFetch` (:2731) — one sidecar blob fetch's lifecycle. */
export type BlobFetch = { state: "loading" } | { state: "failed" } | { state: "ready"; detail: ToolDetail };

/** The 20s fetch timeout (spawn_blob_fetch, :4349). */
const BLOB_TIMEOUT_MS = 20_000;

/**
 * One AUTOMATIC fold transition (ticket 71): a thought chip completing
 * (unresolved→resolved, no explicit pin) or the rendered-open flip of a
 * live group (auto-open expiring). Explicit clicks never emit — they take
 * the viewport through the component's fold-navigation callback instead.
 * The scroller subscribes to arm its reading-anchor compensation, gated by
 * the one-owner rule (lib/tool-fold-scroll.ts).
 */
export interface AutomaticFoldTransition {
  readonly rowId: string;
  /** The detail fold's key (`"{rowId}#d{ix}"`); null for the group fold. */
  readonly key: string | null;
  readonly toggledAt: number;
}

/**
 * The explicit fold pins of one chat/doc (ticket 68): group folds and chip
 * detail folds keyed by the store's existing stable identities (row id;
 * `"{rowId}#d{ix}"`). Capture keeps ONLY the `open` pins — no tween clocks,
 * no reveal state, no blob payload — so restoring a choice renders its
 * endpoint without scheduling any motion.
 */
export interface ExplicitFoldSnapshot {
  readonly groups: ReadonlyMap<string, boolean>;
  readonly details: ReadonlyMap<string, boolean>;
}

/**
 * The tool groups' render-local state: group folds, chip detail folds, reveal
 * epochs, and sidecar blob fetches — the desktop's per-Transcript entity
 * fields (`folds`, `tool_details`, `tool_group_reveals`, `blob_details` +
 * `blob_fetch_order`). One instance per transcript SURFACE (the web's
 * transcript-entity), so a virtualized row scrolling back into view (a
 * remount) finds its fold where it left it, reveal epochs survive row
 * splices, and a subagent tab's sync can never touch the primary chat's
 * reveals. There is **no automatic backoff ladder** for blob fetches:
 * failure re-arms the affordance and the user retries (the label says so).
 */
export class ToolGroupMotionStore {
  readonly #reveals = new Map<string, ToolGroupReveal>();
  readonly #folds = new Map<string, FoldState>();
  readonly #detailFolds = new Map<string, FoldState>();
  readonly #blobs = new Map<string, BlobFetch>();
  readonly #blobOrder = new Map<string, number>();
  readonly #counts = new Map<string, number>();
  /**
   * Ticket 71 B — the rendered card height per expandable chip key, as the
   * renderer reported it: the animated thought-close tween's `from`. A chip
   * that never rendered open has no entry, so its completion snaps (an
   * unmounted row's close is invisible anyway).
   */
  readonly #detailCardHeights = new Map<string, number>();
  /**
   * Ticket 69/71 — the last-seen `resolved` per thought chip key. Only a
   * LIVE flip Some(false)→true is a genuine completion; the replay baseline
   * clears this map so a reset/replay never impersonates one.
   */
  readonly #thoughtSeenResolved = new Map<string, boolean>();
  readonly #transitionListeners = new Set<(transition: AutomaticFoldTransition) => void>();
  readonly #arrival: ChatArrivalWindow | null;
  #blobCounter = 0;
  #version = 0;
  readonly #listeners = new Set<() => void>();

  /**
   * @param arrival The chat-switch arrival window (ticket 58) — the surface's
   * ONE predicate, shared with the scroller. While it is armed, fold flips
   * render their endpoint without a tween and `sync` does not arm the
   * shimmer: a switch's arrival is atomic, not choreographed.
   */
  constructor(arrival: ChatArrivalWindow | null = null) {
    this.#arrival = arrival;
  }

  getVersion = (): number => this.#version;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /**
   * Subscribe to AUTOMATIC fold transitions (ticket 71): a thought chip's
   * animated completion close, or a live group's rendered-open flip. The
   * scroller uses this to arm its reading-anchor compensation under the
   * one-owner gate; clicks never appear here.
   */
  onAutomaticFoldTransition = (listener: (transition: AutomaticFoldTransition) => void): (() => void) => {
    this.#transitionListeners.add(listener);
    return () => {
      this.#transitionListeners.delete(listener);
    };
  };

  #emitAutomaticFoldTransition(transition: AutomaticFoldTransition): void {
    for (const listener of [...this.#transitionListeners]) {
      listener(transition);
    }
  }

  groupFold(rowId: string): FoldState | null {
    return this.#folds.get(rowId) ?? null;
  }

  detailFold(key: string): FoldState | null {
    return this.#detailFolds.get(key) ?? null;
  }

  revealOf(rowId: string): ToolGroupReveal | null {
    return this.#reveals.get(rowId) ?? null;
  }

  blobFetchOf(ref: string): BlobFetch | null {
    return this.#blobs.get(ref) ?? null;
  }

  blobOrderOf(ref: string): number {
    return this.#blobOrder.get(ref) ?? 0;
  }

  /**
   * `toggle_fold` (:4518) — the group header's click. `openHeight` is the
   * CURRENT revealed height (the tween's start when closing).
   */
  toggleGroupFold(rowId: string, openHeight: number, autoOpen: boolean): void {
    const prev = this.#folds.get(rowId) ?? DEFAULT_FOLD;
    const currentlyOpen = prev.open ?? autoOpen;
    this.#folds.set(rowId, {
      open: !currentlyOpen,
      epoch: prev.epoch + 1,
      from: currentlyOpen ? openHeight : 0,
      toggledAt: performance.now(),
      disclosureAt: performance.now(),
    });
    this.#bump();
  }

  /**
   * The expandable chip's header click (:6257-6266) — `from` is the card's
   * rendered height at the toggle (`row_height − base + CHIP_CARD_HEIGHT`).
   */
  toggleDetailFold(key: string, from: number, defaultOpen: boolean): void {
    const prev = this.#detailFolds.get(key) ?? DEFAULT_FOLD;
    const currentlyOpen = prev.open ?? defaultOpen;
    this.#detailFolds.set(key, {
      open: !currentlyOpen,
      epoch: prev.epoch + 1,
      from,
      toggledAt: performance.now(),
      disclosureAt: null,
    });
    this.#bump();
  }

  /**
   * The rendered-open flip without a user click (auto-open expiring,
   * :5862-5870): seed the fold's tween from the last RENDERED height. A user
   * toggle's own `from`/`toggledAt` agree with these values, so the seed
   * never fights a click. On a chat-switch ARRIVAL the flip records its
   * endpoint WITHOUT the tween (ticket 58): the arrival predicate is armed,
   * so the render is the destination state — only a same-chat flip (the
   * chat live again) animates. Ticket 71: the seeded flip also announces
   * itself so the scroller can preserve the reading anchor through the
   * shrink (never fighting a pinned tail-follow or a held runway).
   */
  noteRendered(rowId: string, open: boolean, bodyHeight: number): void {
    const reveal = this.#reveals.get(rowId);
    if (reveal === undefined) {
      return;
    }
    if (reveal.renderedOpen !== null && reveal.renderedOpen !== open) {
      if (this.#arrival?.isArrival(performance.now()) !== true) {
        const prev = this.#folds.get(rowId) ?? DEFAULT_FOLD;
        const now = performance.now();
        this.#folds.set(rowId, { ...prev, from: reveal.renderedHeight, toggledAt: now, disclosureAt: now });
        this.#emitAutomaticFoldTransition({ rowId, key: null, toggledAt: now });
        this.#bump();
      }
    }
    reveal.renderedOpen = open;
    reveal.renderedHeight = bodyHeight;
  }

  /**
   * The renderer's per-paint card-height report for one expandable chip
   * (ticket 70's measurement key feeds the same geometry): while a thought
   * streams open this records its settled open height, which the animated
   * completion close uses as the tween's `from`. Pure recording — seeding
   * is `sync`'s job, so replay and arrivals can never impersonate a live
   * completion.
   */
  noteDetailRendered(key: string, cardHeight: number): void {
    this.#detailCardHeights.set(key, cardHeight);
  }

  /**
   * The reveal-epoch assignment on every row sync (:4059-4116). `baseline` is
   * the replay baseline (the first populated frame after attach): it clears
   * every reveal and strips the group folds' tween clocks, so replaying
   * history never re-animates an existing task tree. `replaying` marks a
   * TRANSIENT empty window (the store re-subscribing — a desync, a
   * reconnect): the reset lands as an atomic swap, so the live-set cleanup
   * below is skipped for that call and the reveal counts survive it
   * (the desktop reads `previous_tool_counts` off the LIVE rows, which never
   * empty mid-session, transcript.rs:4073-4097).
   */
  sync(rows: readonly TranscriptRow[], baseline: boolean, replaying = false): void {
    const now = performance.now();
    if (baseline) {
      this.#reveals.clear();
      // Ticket 71/69: the completion tracker resets with the baseline — a
      // replayed frame re-records its resolved states without ever reading
      // as an unresolved→resolved flip, and the recorded card heights (the
      // animated close's `from`) never outlive the transcript they belong
      // to.
      this.#thoughtSeenResolved.clear();
      this.#detailCardHeights.clear();
      // Retain explicit user pins, but never resume an old arrival or
      // closing animation when revisiting the retained transcript.
      for (const [key, fold] of this.#folds) {
        if (fold.toggledAt !== null || fold.disclosureAt !== null) {
          this.#folds.set(key, { ...fold, toggledAt: null, disclosureAt: null });
        }
      }
    }
    const live = new Set<string>();
    const liveDetailKeys = new Set<string>();
    const transitions: AutomaticFoldTransition[] = [];
    for (const row of rows) {
      if (row.rowKind.kind !== "toolGroup") {
        continue;
      }
      const tools = row.rowKind.tools;
      // Ticket 71 B — the animated thought completion. Every tool group
      // (standalone spawn cards included) tracks its thought chips' seen
      // `resolved`: a LIVE flip to resolved with no explicit pin seeds the
      // detail fold's close tween from the renderer-reported card height,
      // so the chip closes over the existing 140ms EASE_OUT fold instead
      // of snapping. Explicit pins win in every case (the seed never fires
      // under a pin); a fresh store, an armed arrival, or a baseline frame
      // never reads as a completion.
      for (let ix = 0; ix < tools.length; ix += 1) {
        const tool = tools[ix]!;
        if (isSpawnLink(tool)) {
          continue;
        }
        const key = `${row.id}#d${ix}`;
        if (tool.detail !== null || tool.invocation !== null) {
          liveDetailKeys.add(key);
        }
        if (!tool.isThought) {
          continue;
        }
        const previouslyResolved = this.#thoughtSeenResolved.get(key) ?? null;
        this.#thoughtSeenResolved.set(key, tool.resolved);
        if (
          previouslyResolved === false &&
          tool.resolved &&
          (tool.detail !== null || tool.invocation !== null) &&
          (this.#detailFolds.get(key)?.open ?? null) === null &&
          this.#detailCardHeights.has(key) &&
          this.#arrival?.isArrival(now) !== true
        ) {
          const prev = this.#detailFolds.get(key) ?? DEFAULT_FOLD;
          this.#detailFolds.set(key, {
            open: null,
            epoch: prev.epoch + 1,
            from: this.#detailCardHeights.get(key)!,
            toggledAt: now,
            disclosureAt: null,
          });
          transitions.push({ rowId: row.id, key, toggledAt: now });
        }
      }
      // Agent/spawn groups are standalone cards, not task trees.
      if (!toolGroupCollapses(tools)) {
        continue;
      }
      live.add(row.id);
      const oldCount = Math.min(baseline ? tools.length : this.#counts.get(row.id) ?? 0, tools.length);
      const isNewGroup = !baseline && !this.#counts.has(row.id);
      let reveal = this.#reveals.get(row.id);
      if (reveal === undefined) {
        reveal = { ...DEFAULT_REVEAL, starts: [] };
        this.#reveals.set(row.id, reveal);
      }
      // Ticket 58: no shimmer restart on a chat switch's arrival — the
      // window is armed, so this is the first paint of an already-loaded
      // transcript; a later live sync (real streaming content) arms it.
      if (reveal.shimmerStartedAt === null && this.#arrival?.isArrival(now) !== true) {
        reveal.shimmerStartedAt = now;
      }
      if (isNewGroup && reveal.headerStartedAt === null) {
        reveal.headerStartedAt = now;
      }
      reveal.starts.length = tools.length;
      const firstRowDelay = isNewGroup ? TOOL_FIRST_ROW_DELAY_MS : 0;
      let arrivalIx = 0;
      for (let toolIx = oldCount; toolIx < tools.length; toolIx += 1) {
        reveal.starts[toolIx] = now + firstRowDelay + arrivalIx * TOOL_ROW_STAGGER_MS;
        arrivalIx += 1;
      }
      this.#counts.set(row.id, tools.length);
    }
    // The live-set cleanup: reveals and counts for rows absent from `rows`.
    // A transient empty window while the store is replaying is NOT an
    // authoritative empty — the reset frame replaces the rows atomically —
    // so the cleanup is skipped entirely for that call.
    const transientEmpty = rows.length === 0 && replaying;
    for (const id of [...this.#reveals.keys()]) {
      if (!transientEmpty && !live.has(id)) {
        this.#reveals.delete(id);
      }
    }
    for (const id of [...this.#counts.keys()]) {
      if (!transientEmpty && !live.has(id)) {
        this.#counts.delete(id);
      }
    }
    for (const key of [...this.#thoughtSeenResolved.keys()]) {
      if (!transientEmpty && !liveDetailKeys.has(key)) {
        this.#thoughtSeenResolved.delete(key);
      }
    }
    for (const key of [...this.#detailCardHeights.keys()]) {
      if (!transientEmpty && !liveDetailKeys.has(key)) {
        this.#detailCardHeights.delete(key);
      }
    }
    if (transitions.length > 0) {
      // The listener may re-render (arming the scroller's compensation);
      // deliver it before the store's own bump so both land in one commit.
      for (const transition of transitions) {
        this.#emitAutomaticFoldTransition(transition);
      }
    }
    this.#bump();
  }

  /**
   * Serialize the explicit fold pins for chat-switch memory (ticket 68):
   * every group/detail fold the user pinned, reduced to its `open` value.
   * Unpinned entries (auto-following) and tween metadata are dropped — the
   * replay baseline owns reveal state, and a restored choice must not carry
   * a click or tween timestamp.
   */
  captureExplicitFolds(): ExplicitFoldSnapshot {
    const groups = new Map<string, boolean>();
    for (const [key, fold] of this.#folds) {
      if (fold.open !== null) {
        groups.set(key, fold.open);
      }
    }
    const details = new Map<string, boolean>();
    for (const [key, fold] of this.#detailFolds) {
      if (fold.open !== null) {
        details.set(key, fold.open);
      }
    }
    return { groups, details };
  }

  /**
   * Reinstall saved explicit pins as SETTLED folds (ticket 68): the pin wins
   * over auto-open and arrival-pending through the shared geometry resolver,
   * and `toggledAt`/`disclosureAt` stay null so no tween runs. Keys whose
   * rows vanished since the capture are harmless misses — the maps only
   * surface through lookups for rows that render.
   */
  restoreExplicitFolds(saved: ExplicitFoldSnapshot): void {
    for (const [key, open] of saved.groups) {
      this.#folds.set(key, { open, epoch: 1, from: 0, toggledAt: null, disclosureAt: null });
    }
    for (const [key, open] of saved.details) {
      this.#detailFolds.set(key, { open, epoch: 1, from: 0, toggledAt: null, disclosureAt: null });
    }
    if (saved.groups.size > 0 || saved.details.size > 0) {
      this.#bump();
    }
  }

  /**
   * `spawn_blob_fetch` (:4319) — rank BEFORE the guard (clicking a Ready ref
   * is the "show me this one again" toggle), Loading re-entry is a no-op,
   * Failed re-arms as a retry. The 20s timeout maps to the desktop's
   * `call_with_timeout`.
   */
  beginBlobFetch(blobRef: string, fetchText: () => Promise<string>): void {
    this.#blobCounter += 1;
    this.#blobOrder.set(blobRef, this.#blobCounter);
    const current = this.#blobs.get(blobRef);
    if (current !== undefined && current.state === "ready") {
      this.#bump();
      return;
    }
    if (current !== undefined && current.state === "loading") {
      return;
    }
    this.#blobs.set(blobRef, { state: "loading" });
    this.#bump();
    let timer = 0;
    const timedOut = new Promise<never>((_, reject) => {
      timer = window.setTimeout(() => reject(new Error("blob fetch timed out")), BLOB_TIMEOUT_MS);
    });
    const settle = (): void => {
      window.clearTimeout(timer);
    };
    Promise.race([fetchText(), timedOut]).then(
      (text) => {
        settle();
        const detail = blobDetail(text, blobRef.endsWith(".diff"));
        this.#blobs.set(blobRef, detail !== null ? { state: "ready", detail } : { state: "failed" });
        this.#bump();
      },
      () => {
        settle();
        this.#blobs.set(blobRef, { state: "failed" });
        this.#bump();
      },
    );
  }

  /** Test seam — drops every fold/reveal/fetch without notifying. */
  reset(): void {
    this.#reveals.clear();
    this.#folds.clear();
    this.#detailFolds.clear();
    this.#blobs.clear();
    this.#blobOrder.clear();
    this.#counts.clear();
    this.#detailCardHeights.clear();
    this.#thoughtSeenResolved.clear();
    this.#blobCounter = 0;
    this.#bump();
  }

  #bump(): void {
    this.#version += 1;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}
