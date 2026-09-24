/**
 * The tool group's SHARED geometry contract (ticket 70,
 * .scratch/web-parity/issues/70): ONE pure resolver behind both the renderer
 * (`ToolGroupRow`, tool-group.tsx) and the scroller's estimator
 * (`estimateRowHeight`, transcript.tsx), so for the same row state, fold
 * state, effective payloads, timestamp, and reduced-motion flag the estimate
 * IS the rendered height. The rail row is 32px (`TOOL_TREE_ROW_HEIGHT`, the
 * renderer's `baseRowHeight`) — not the 38px standalone `CHIP_HEIGHT` the
 * old chipsHeight-based estimate self-confirmed.
 *
 * Side-effect-free by contract: everything reads the motion store through
 * the read-only `ToolGroupGeometryState` view and never fetches blobs,
 * changes request order, bumps versions, mutates folds, or publishes
 * heights. Callers pass ONE timestamp per pass so the estimate and the
 * render never disagree because time was read twice.
 */

import {
  BLOB_AFFORDANCE_HEIGHT,
  CHIP_CARD_HEIGHT,
  CHIP_HEIGHT,
  CHIPS_TOP_PAD,
  detailHeight,
  formatKb,
  isSpawnLink,
  toolGroupCollapses,
  TOOL_GROUP_HEADER_HEIGHT,
  TOOL_TREE_ROW_HEIGHT,
  type ToolDetail,
  type ToolItem,
  type TranscriptRow,
} from "./transcript";
import {
  TOOL_CONNECTOR_REVEAL_MS,
  toolConnectorRevealProgress,
  toolFoldProgress,
  toolRowRevealProgress,
  type BlobFetch,
  type FoldState,
  type ToolGroupReveal,
} from "./tool-motion";
import { DIFF_LINE_HEIGHT } from "./diff";

// ---------------------------------------------------------------------------
// The read-only state view
// ---------------------------------------------------------------------------

/**
 * The geometry-relevant, READ-ONLY slice of the tool-motion store.
 * `ToolGroupMotionStore` satisfies this structurally; tests can stub it.
 */
export interface ToolGroupGeometryState {
  groupFold(rowId: string): FoldState | null;
  detailFold(key: string): FoldState | null;
  revealOf(rowId: string): ToolGroupReveal | null;
  blobFetchOf(ref: string): BlobFetch | null;
  blobOrderOf(ref: string): number;
}

/** No folds, no reveals, no blobs — the bare row-data estimate's state. */
export const EMPTY_TOOL_GEOMETRY_STATE: ToolGroupGeometryState = {
  groupFold: () => null,
  detailFold: () => null,
  revealOf: () => null,
  blobFetchOf: () => null,
  blobOrderOf: () => 0,
};

/** The estimator's per-pass context: read-only state + ONE timestamp + flag. */
export interface ToolGroupEstimateContext {
  readonly state: ToolGroupGeometryState;
  readonly now: number;
  readonly reduced: boolean;
  /**
   * The code-size-scaled diff row / markdown code line the renderer is
   * painting against (lib/typography.ts). Absent = the 12.5px-code
   * setting's values, which keeps bare-data estimate callers honest.
   */
  readonly diffLineHeight?: number;
  readonly codeLineHeight?: number;
}

// ---------------------------------------------------------------------------
// Blob-upgrade resolution (render_tool_group :5880-5959) — the SAME rules the
// renderer uses, moved here so the estimator can never drift from them.
// ---------------------------------------------------------------------------

/** The resolved affordance slot (the renderer also shows its label). */
export interface ToolAffordance {
  readonly ref: string;
  readonly label: string;
  readonly loading: boolean;
}

/**
 * The best READY blob by request order: among the tool's diff and output
 * refs, the most recently REQUESTED ready fetch wins — the ONE loop both
 * blob-upgrade resolutions below project onto their own field.
 */
function bestReadyBlob(
  tool: ToolItem,
  state: ToolGroupGeometryState,
): { ref: string; detail: ToolDetail } | null {
  let best: { order: number; ref: string; detail: ToolDetail } | null = null;
  for (const ref of [tool.diffRef, tool.outputRef]) {
    if (ref === null) {
      continue;
    }
    const fetch = state.blobFetchOf(ref);
    if (fetch !== null && fetch.state === "ready") {
      const order = state.blobOrderOf(ref);
      if (best === null || order > best.order) {
        best = { order, ref, detail: fetch.detail };
      }
    }
  }
  return best;
}

/** The most recently REQUESTED Ready blob wins; else the doc detail. */
export function effectiveToolDetail(tool: ToolItem, state: ToolGroupGeometryState): ToolDetail | null {
  const best = bestReadyBlob(tool, state);
  return best !== null ? best.detail : tool.detail;
}

/** The ref of the blob whose upgrade is currently showing, if any. */
function shownBlobRef(tool: ToolItem, state: ToolGroupGeometryState): string | null {
  const best = bestReadyBlob(tool, state);
  return best !== null ? best.ref : null;
}

/**
 * The one affordance slot (:5913-5959): diff offered first (the richer
 * upgrade), then the output. A fetched-and-SHOWING ref hands the slot to the
 * next unfetched one; a fetched-but-not-showing ref stays offered as a
 * no-fetch recency toggle. Failure re-arms as a manual retry — there is no
 * backoff ladder.
 */
export function effectiveToolAffordance(tool: ToolItem, state: ToolGroupGeometryState): ToolAffordance | null {
  if (isSpawnLink(tool)) {
    return null;
  }
  const shown = shownBlobRef(tool, state);
  const candidates: { ref: string | null; what: string; bytes: number | null }[] = [
    { ref: tool.diffRef, what: "diff", bytes: null },
    { ref: tool.outputRef, what: "output", bytes: tool.outputBytes },
  ];
  for (const { ref, what, bytes } of candidates) {
    if (ref === null) {
      continue;
    }
    const fetch = state.blobFetchOf(ref);
    if (fetch === null) {
      return {
        ref,
        label: bytes !== null ? `Show full ${what} (${formatKb(bytes)})` : `Show full ${what}`,
        loading: false,
      };
    }
    if (fetch.state === "loading") {
      return { ref, label: `Loading full ${what}…`, loading: true };
    }
    if (fetch.state === "failed") {
      return { ref, label: `Couldn't load full ${what} — tap to retry`, loading: false };
    }
    if (ref === shown) {
      continue;
    }
    return { ref, label: `Show full ${what}`, loading: false };
  }
  return null;
}

// ---------------------------------------------------------------------------
// The shared resolver
// ---------------------------------------------------------------------------

export interface ToolGroupGeometryInput {
  readonly rowId: string;
  readonly tools: readonly ToolItem[];
  /** Set at row build time: streaming && this group is the entry's LAST part. */
  readonly autoOpen: boolean;
  /** The read-only motion-store view (folds, reveals, blob fetches). */
  readonly state: ToolGroupGeometryState;
  /** ONE timestamp for the whole pass (the renderer's and estimator's agree). */
  readonly now: number;
  readonly reduced: boolean;
  /**
   * The code-size-scaled diff row (`diff_line_height`, lib/typography.ts)
   * — the renderer and the estimator must pass the SAME value; defaults to
   * the 12.5px-code setting's 21px.
   */
  readonly diffLineHeight?: number;
}

export interface ToolGroupGeometry {
  readonly collapses: boolean;
  /** Effective group open: the explicit pin, else autoOpen OR arrivalPending. */
  readonly open: boolean;
  readonly arrivalPending: boolean;
  /** `autoOpen || arrivalPending` — the fold toggle's default resolution. */
  readonly effectiveAutoOpen: boolean;
  /** 32 on the rail, `CHIP_HEIGHT` (38) for standalone spawn cards. */
  readonly baseRowHeight: number;
  readonly details: readonly (ToolDetail | null)[];
  readonly invocations: readonly (ToolDetail | null)[];
  readonly affordances: readonly (ToolAffordance | null)[];
  readonly detailFolds: readonly (FoldState | null)[];
  readonly detailOpens: readonly boolean[];
  /** Tweened per-tool row heights (pre-reveal), in tool order. */
  readonly rowHeights: readonly number[];
  readonly revealProgress: readonly number[];
  readonly connectorProgress: readonly number[];
  /** 26 × header reveal progress for collapsible groups, 0 for standalone. */
  readonly headerHeight: number;
  /** The open body's settled height: top pad (collapsible) + revealed rows. */
  readonly revealedHeight: number;
  /** The fold-tweened body height (0 while closed). */
  readonly bodyHeight: number;
  /** headerHeight + bodyHeight — the row's analytic natural height. */
  readonly totalHeight: number;
  /** True while any tween/reveal is unfinished (the shared rAF gate). */
  readonly motionActive: boolean;
}

/**
 * `render_tool_group`'s analytic geometry (:5837-6087) as ONE pure function.
 * Standalone (all-agent) groups have no header and no fold: their `bodyHeight`
 * is the bare revealed sum (no `.tool-group-body` top pad renders), matching
 * the unwrapped-chips DOM — the estimator's standalone branch keeps its own
 * legacy `chipsHeight` fallback per ticket 70's preserve-standalone rule.
 */
export function toolGroupGeometry(input: ToolGroupGeometryInput): ToolGroupGeometry {
  const { rowId, tools, autoOpen, state, now, reduced } = input;
  const diffLine = input.diffLineHeight ?? DIFF_LINE_HEIGHT;
  const collapses = toolGroupCollapses(tools);
  const fold = state.groupFold(rowId);
  const reveal = state.revealOf(rowId);
  const starts = reveal?.starts ?? [];
  // A FUTURE start reads as pending (elapsed saturates at 0), exactly like
  // the desktop's checked_duration_since.
  const arrivalPending = !reduced && starts.some((start) => start !== null && now - start < TOOL_CONNECTOR_REVEAL_MS);
  const effectiveAutoOpen = autoOpen || arrivalPending;
  const open = !collapses || (fold?.open ?? effectiveAutoOpen);
  const baseRowHeight = collapses ? TOOL_TREE_ROW_HEIGHT : CHIP_HEIGHT;

  // ── The chips' effective payloads (:5874-5989) ──────────────────────────
  const details: (ToolDetail | null)[] = [];
  const invocations: (ToolDetail | null)[] = [];
  const affordances: (ToolAffordance | null)[] = [];
  const detailFolds: (FoldState | null)[] = [];
  const detailOpens: boolean[] = [];
  for (let ix = 0; ix < tools.length; ix += 1) {
    const tool = tools[ix]!;
    // Spawn chips never expand — the subagent doc is the record of what the
    // tool did; the whole chip is the "open that doc" click instead.
    if (isSpawnLink(tool)) {
      details.push(null);
      invocations.push(null);
      affordances.push(null);
      detailFolds.push(null);
      detailOpens.push(false);
      continue;
    }
    const detail = effectiveToolDetail(tool, state);
    const invocation = tool.invocation;
    const dfold = state.detailFold(`${rowId}#d${ix}`);
    details.push(detail);
    invocations.push(invocation);
    affordances.push(effectiveToolAffordance(tool, state));
    detailFolds.push(dfold);
    detailOpens.push((detail !== null || invocation !== null) && (dfold?.open ?? (tool.isThought && !tool.resolved)));
  }

  // ── The chips' heights (analytic — :6000-6042) ──────────────────────────
  let motionActive = false;
  const rowHeights: number[] = [];
  for (let ix = 0; ix < tools.length; ix += 1) {
    const target = detailOpens[ix]
      ? baseRowHeight +
        (invocations[ix] !== null ? detailHeight(invocations[ix]!, diffLine) : 0) +
        (details[ix] !== null ? detailHeight(details[ix]!, diffLine) : 0) +
        (affordances[ix] !== null ? BLOB_AFFORDANCE_HEIGHT : 0)
      : baseRowHeight;
    const dfold = detailFolds[ix] ?? null;
    const from = dfold !== null && dfold.toggledAt !== null ? dfold.from + baseRowHeight - CHIP_CARD_HEIGHT : null;
    const tweened = tweenHeight(from, target, dfold, now, reduced);
    if (tweened.motion) {
      motionActive = true;
    }
    rowHeights.push(tweened.height);
  }

  // ── Reveal progress per row (:6044-6079) ───────────────────────────────
  const revealProgress: number[] = [];
  const connectorProgress: number[] = [];
  for (let ix = 0; ix < tools.length; ix += 1) {
    const start = starts[ix] ?? null;
    revealProgress.push(toolRowRevealProgress(start, now, reduced));
    connectorProgress.push(toolConnectorRevealProgress(start, now, reduced));
  }
  const headerReveal = collapses ? toolRowRevealProgress(reveal?.headerStartedAt ?? null, now, reduced) : 1;
  if (headerReveal < 1 || revealProgress.some((p) => p < 1) || connectorProgress.some((p) => p < 1)) {
    motionActive = true;
  }

  // ── Group body height (:6080-6087, :6360-6387) ─────────────────────────
  const revealedHeight =
    (collapses ? CHIPS_TOP_PAD : 0) + rowHeights.reduce((sum, height, ix) => sum + height * revealProgress[ix]!, 0);
  const bodyTarget = open ? revealedHeight : 0;
  const bodyTweened = tweenHeight(fold?.from ?? null, bodyTarget, fold, now, reduced);
  if (bodyTweened.motion) {
    motionActive = true;
  }
  const bodyHeight = bodyTweened.height;
  const headerHeight = collapses ? TOOL_GROUP_HEADER_HEIGHT * headerReveal : 0;

  return {
    collapses,
    open,
    arrivalPending,
    effectiveAutoOpen,
    baseRowHeight,
    details,
    invocations,
    affordances,
    detailFolds,
    detailOpens,
    rowHeights,
    revealProgress,
    connectorProgress,
    headerHeight,
    revealedHeight,
    bodyHeight,
    totalHeight: headerHeight + bodyHeight,
    motionActive,
  };
}

/**
 * The height tween shared by the group fold and the chip cards (:6025-6041,
 * :6360-6375): lerp from `from` to `target` over TOOL_FOLD while the fold's
 * clock is armed; past 140ms it saturates at `target` (an aged tween renders
 * its endpoint — a remount never flashes). Null `from` renders the target.
 */
function tweenHeight(
  from: number | null,
  target: number,
  fold: FoldState | null,
  now: number,
  reduced: boolean,
): { height: number; motion: boolean } {
  if (from === null || reduced || fold === null || fold.toggledAt === null) {
    return { height: target, motion: false };
  }
  const t = toolFoldProgress(fold, now);
  if (t === null) {
    return { height: target, motion: false };
  }
  return { height: from + (target - from) * t, motion: t < 1 };
}

// ---------------------------------------------------------------------------
// The measured-height cache boundary (ticket 70 §2.4)
// ---------------------------------------------------------------------------

/**
 * The semantic fingerprint a cached tool-row measurement is valid FOR: the
 * group's auto-open flag and fold pin, and per chip the resolved detail-open
 * state, the analytic heights of its effective detail + invocation (a doc
 * growth or a fetched-payload swap moves them), and the affordance slot's
 * ref/loading identity. Pure read — repeated calls with unchanged state
 * return the same string and cause no fetches, bumps, or notifications.
 * Standalone spawn groups return null: their geometry is row-data only, with
 * nothing store-backed to go stale.
 */
export function toolGroupMeasurementKey(
  rowId: string,
  tools: readonly ToolItem[],
  autoOpen: boolean,
  state: ToolGroupGeometryState,
  diffLineHeight: number = DIFF_LINE_HEIGHT,
): string | null {
  if (!toolGroupCollapses(tools)) {
    return null;
  }
  const parts: (string | number | boolean)[] = [autoOpen ? 1 : 0, state.groupFold(rowId)?.open ?? "-"];
  for (let ix = 0; ix < tools.length; ix += 1) {
    const tool = tools[ix]!;
    if (isSpawnLink(tool)) {
      parts.push("s");
      continue;
    }
    const detail = effectiveToolDetail(tool, state);
    const invocation = tool.invocation;
    const dfold = state.detailFold(`${rowId}#d${ix}`);
    const detailOpen = (detail !== null || invocation !== null) && (dfold?.open ?? (tool.isThought && !tool.resolved));
    const affordance = effectiveToolAffordance(tool, state);
    parts.push(
      detailOpen ? 1 : 0,
      invocation === null ? 0 : detailHeight(invocation, diffLineHeight),
      detail === null ? 0 : detailHeight(detail, diffLineHeight),
      affordance === null ? "-" : `${affordance.ref}/${affordance.loading ? "l" : "r"}`,
    );
  }
  return parts.join(",");
}

/** The scroller's per-render pass: the current semantic key of each collapsible tool row. */
export function computeToolMeasurementKeys(
  rows: readonly TranscriptRow[],
  state: ToolGroupGeometryState,
  diffLineHeight: number = DIFF_LINE_HEIGHT,
): Map<string, string> {
  const keys = new Map<string, string>();
  for (const row of rows) {
    if (row.rowKind.kind !== "toolGroup") {
      continue;
    }
    const key = toolGroupMeasurementKey(row.id, row.rowKind.tools, row.rowKind.autoOpen, state, diffLineHeight);
    if (key !== null) {
      keys.set(row.id, key);
    }
  }
  return keys;
}

/**
 * The scroller's ONE semantic invalidation path: drop the cached measurement
 * of each tool row whose measure-time key no longer matches its current
 * inputs — an unmounted row's in-flight blob fetch completing is the
 * motivating case — so the (now analytic-exact) estimate stands in until the
 * row remounts and re-measures. Unchanged tool rows and every non-tool row
 * keep their cached measurements. Pure map edits: no fetches, no
 * notifications, and a second call with unchanged inputs drops nothing.
 * Returns the dropped ids (test observability).
 */
export function pruneStaleToolMeasurements(
  heights: Map<string, number>,
  measuredKeys: Map<string, string>,
  currentKeys: ReadonlyMap<string, string>,
): string[] {
  const dropped: string[] = [];
  for (const id of [...heights.keys()]) {
    const current = currentKeys.get(id);
    if (current === undefined) {
      continue;
    }
    if (measuredKeys.get(id) !== current) {
      heights.delete(id);
      measuredKeys.delete(id);
      dropped.push(id);
    }
  }
  return dropped;
}
