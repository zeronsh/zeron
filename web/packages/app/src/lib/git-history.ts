/**
 * The History pane's pure logic — a port of the desktop's `history.rs`
 * graph/geometry/transition family plus the engine's `git_history_matches`
 * (`crates/engine/src/repos.rs`). Everything here is UI-framework-agnostic
 * and unit-tested against the desktop's test names (§3 of ticket 27).
 *
 * The graph model: commits arrive **child-before-parent** (topo order);
 * `layoutGraph` walks a list of active lanes (each waiting to reach the sha
 * it targets) and emits one `GraphRow` per commit — node position/color plus
 * the 0-3 `Incoming`/`Outgoing`/`Through` segments a row paints. Pagination
 * only ever appends older commits, so the loaded prefix's rows are stable.
 *
 * Geometry is one `GraphGeometry` for the whole graph (`laneCount` is the
 * widest lane count ever seen) with a fitted/compact derivation and a
 * hysteresis'd compact switch; the web drops the desktop's device-pixel
 * snapping (§6 — browsers rasterize SVG strokes at any DPR).
 *
 * Colors: no literal hex — the 6-lane palette reads the `--rb-*` role vars
 * (`graphColor` multiplies HSL saturation by 0.72, `history.rs:3312-3319`).
 */

import type { GitHistoryCommit, GitHistoryRef } from "@zeron/proto";

// ---------------------------------------------------------------------------
// Constants (history.rs:36-69, plus the sizing/paging family)
// ---------------------------------------------------------------------------

export const HISTORY_ROW_HEIGHT = 36;
export const HISTORY_LANE_SPACING = 12;
export const HISTORY_NODE_RADIUS = 3;
export const HISTORY_HEAD_RING_PADDING = 2;
export const HISTORY_STROKE_WIDTH = 1.5;
export const HISTORY_GRAPH_FOCUSED_STROKE_WIDTH = 2.25;
export const HISTORY_GRAPH_SATURATION = 0.72;
/** `EDGE_INSET*2 - NODE_RADIUS` — the first lane centers on the header gutter. */
export const HISTORY_GRAPH_SIDE_PADDING = 13;
export const HISTORY_GRAPH_TRAILING_PADDING = 12;
export const HISTORY_GRAPH_MIN_COMPACT_WIDTH = 48;
export const HISTORY_GRAPH_MAX_WIDTH_RATIO = 0.34;
export const HISTORY_GRAPH_RESIZE_STEP = 2;
export const HISTORY_GRAPH_COMPACT_ENTER_SUBJECT_WIDTH = 160;
export const HISTORY_GRAPH_COMPACT_EXIT_SUBJECT_WIDTH = 184;
export const HISTORY_GRAPH_COMPACT_ENTER_LANE_SPACING = 4;
export const HISTORY_GRAPH_COMPACT_EXIT_LANE_SPACING = 5;
export const HISTORY_GRAPH_ROW_OVERLAP = 0.75;
export const HISTORY_GRAPH_HIT_RADIUS = 5.5;
export const HISTORY_GRAPH_UNFOCUSED_OPACITY = 0.24;
export const HISTORY_ROW_UNFOCUSED_OPACITY = 0.6;
export const HISTORY_COMMIT_SUBJECT_MIN_WIDTH = 80;
export const HISTORY_REF_AREA_RATIO = 0.45;
export const HISTORY_REF_BADGE_MAX_WIDTH = 112;
export const HISTORY_REF_GAP = 5;
export const HISTORY_SEARCH_WIDTH = 196;
export const HISTORY_SEARCH_DEBOUNCE_MS = 70;
export const HISTORY_SEARCH_IDLE_DISMISS_MS = 1500;
export const HISTORY_COMPARISON_MIN_WIDTH = 260;
/** The client's uniform page size on every history RPC (history.rs:36). */
export const HISTORY_PAGE_SIZE = 100;
/** The Bezier hit-test polyline's sample count (history.rs:1081). */
const HIT_SAMPLES = 10;
/** A NUL-prefixed sentinel that can never collide with a hex sha. */
export const LOAD_MORE_KEY = "\0history-load-more";

// Column limits — `GitHistoryColumnWidths` (settings.rs), Commit is the
// elastic remainder with the subject's minimum as its floor.
export const HISTORY_AUTHOR_MIN = 44;
export const HISTORY_AUTHOR_MAX = 220;
export const HISTORY_DATE_MIN = 68;
export const HISTORY_DATE_MAX = 180;
export const HISTORY_SHA_MIN = 58;
export const HISTORY_SHA_MAX = 140;
export const DEFAULT_HISTORY_COLUMN_WIDTHS = { author: 88, date: 88, sha: 74 } as const;

// ---------------------------------------------------------------------------
// The graph model (history.rs:85-110, 638-733)
// ---------------------------------------------------------------------------

export type GraphSegmentShape = "through" | "incoming" | "outgoing";

export interface GraphSegment {
  readonly fromLane: number;
  readonly toLane: number;
  readonly colorId: number;
  readonly shape: GraphSegmentShape;
}

export interface GraphRow {
  readonly sha: string;
  readonly nodeLane: number;
  readonly nodeColorId: number;
  readonly segments: readonly GraphSegment[];
  readonly isHead: boolean;
}

export interface GraphLayout {
  readonly rows: readonly GraphRow[];
  readonly maxLaneCount: number;
}

interface ActiveLane {
  readonly id: number;
  readonly colorId: number;
  readonly targetSha: string;
}

const laneIndexOf = (lanes: readonly ActiveLane[], id: number): number =>
  lanes.findIndex((lane) => lane.id === id);

/**
 * `layout_graph` (history.rs:638). For each commit: find the lanes already
 * targeting its sha (its children's lanes) — the first one's position and
 * color become the node's; those lanes resolve away; `parentShas[0]` reuses
 * the incoming lane's id/color straight down, extra parents mint lanes
 * inserted right after it; every surviving lane contributes a `Through`
 * segment from its old to its new position (lanes shift when merges remove
 * them). Appending older commits never disturbs the loaded prefix.
 */
export function layoutGraph(commits: readonly GitHistoryCommit[], headSha: string | null): GraphLayout {
  let activeLanes: ActiveLane[] = [];
  let nextLaneId = 0;
  let nextColorId = 0;
  let maxLaneCount = 0;
  const rows: GraphRow[] = [];

  for (const commit of commits) {
    const before = activeLanes;
    const incoming: number[] = [];
    for (let index = 0; index < before.length; index += 1) {
      if (before[index]!.targetSha === commit.sha) {
        incoming.push(index);
      }
    }
    const primaryIncoming = incoming.length > 0 ? incoming[0] : undefined;
    const nodeLane = primaryIncoming ?? before.length;
    const primaryLane = primaryIncoming !== undefined ? before[primaryIncoming] : undefined;
    const nodeColorId = primaryLane !== undefined ? primaryLane.colorId : nextColorId++;
    const resolvedIds = new Set(incoming.map((index) => before[index]!.id));
    const nextLanes: ActiveLane[] = before.filter((lane) => !resolvedIds.has(lane.id));
    const outgoing: Array<{ id: number; colorId: number }> = [];

    let primaryOutgoingId: number | null = null;
    const firstParent = commit.parentShas[0];
    if (firstParent !== undefined) {
      const id = primaryLane !== undefined ? primaryLane.id : nextLaneId++;
      const lane: ActiveLane = { id, colorId: nodeColorId, targetSha: firstParent };
      nextLanes.splice(Math.min(nodeLane, nextLanes.length), 0, lane);
      primaryOutgoingId = id;
      outgoing.push({ id, colorId: nodeColorId });
    }

    let parentOffset = 1;
    for (const parentSha of commit.parentShas.slice(1)) {
      const existing = nextLanes.find((lane) => lane.targetSha === parentSha);
      if (existing !== undefined) {
        outgoing.push({ id: existing.id, colorId: existing.colorId });
        continue;
      }
      const lane: ActiveLane = { id: nextLaneId++, colorId: nextColorId++, targetSha: parentSha };
      const primaryIndex =
        primaryOutgoingId !== null
          ? laneIndexOf(nextLanes, primaryOutgoingId)
          : Math.min(nodeLane, nextLanes.length);
      nextLanes.splice(Math.min(primaryIndex + parentOffset, nextLanes.length), 0, lane);
      parentOffset += 1;
      outgoing.push({ id: lane.id, colorId: lane.colorId });
    }

    const segments: GraphSegment[] = [];
    for (let index = 0; index < before.length; index += 1) {
      const lane = before[index]!;
      if (resolvedIds.has(lane.id)) {
        continue;
      }
      segments.push({
        fromLane: index,
        toLane: laneIndexOf(nextLanes, lane.id),
        colorId: lane.colorId,
        shape: "through",
      });
    }
    for (const fromLane of incoming) {
      segments.push({
        fromLane,
        toLane: nodeLane,
        colorId: before[fromLane]!.colorId,
        shape: "incoming",
      });
    }
    for (const { id, colorId } of outgoing) {
      segments.push({ fromLane: nodeLane, toLane: laneIndexOf(nextLanes, id), colorId, shape: "outgoing" });
    }

    maxLaneCount = Math.max(maxLaneCount, before.length, nextLanes.length, nodeLane + 1);
    rows.push({ sha: commit.sha, nodeLane, nodeColorId, segments, isHead: headSha === commit.sha });
    activeLanes = nextLanes;
  }

  return { rows, maxLaneCount };
}

// ---------------------------------------------------------------------------
// Collapse / compaction (history.rs:760-922, repos.rs:1632-1735)
// ---------------------------------------------------------------------------

/** `branch_ref_key` (history.rs:350) — tags cannot be folded. */
export function branchRefKey(reference: GitHistoryRef): string | null {
  if (reference.kind === "tag") {
    return null;
  }
  const prefix = reference.kind === "branch" ? "local" : "remote";
  return `${prefix}:${reference.label}`;
}

/**
 * `compact_commits_to_visible` (history.rs:825): retain a subset in order,
 * contracting parent edges across omitted commits. The hidden-chain walk is
 * iterative with memoization — a 20,000-deep linear gap must not blow the
 * stack.
 */
export function compactCommitsToVisible(
  commits: readonly GitHistoryCommit[],
  visible: ReadonlySet<string>,
): GitHistoryCommit[] {
  const bySha = new Map<string, GitHistoryCommit>();
  for (const commit of commits) {
    bySha.set(commit.sha, commit);
  }

  /** Resolve one hidden commit's nearest visible ancestors (iterative). */
  const nearestVisibleParents = (
    sha: string,
    memo: Map<string, string[]>,
  ): string[] => {
    if (visible.has(sha) || !bySha.has(sha)) {
      return [sha];
    }
    const cached = memo.get(sha);
    if (cached !== undefined) {
      return cached;
    }

    interface Frame {
      sha: string;
      nextParent: number;
      resolved: string[];
      seen: Set<string>;
    }
    const extend = (frame: Frame, parents: readonly string[]): void => {
      for (const parent of parents) {
        if (!frame.seen.has(parent)) {
          frame.seen.add(parent);
          frame.resolved.push(parent);
        }
      }
    };

    const visiting = new Set<string>([sha]);
    const stack: Frame[] = [{ sha, nextParent: 0, resolved: [], seen: new Set() }];
    for (;;) {
      const frame = stack[stack.length - 1]!;
      const parents = bySha.get(frame.sha)!.parentShas;
      if (frame.nextParent >= parents.length) {
        stack.pop();
        visiting.delete(frame.sha);
        const resolved = frame.resolved;
        memo.set(frame.sha, resolved);
        const caller = stack[stack.length - 1];
        if (caller !== undefined) {
          extend(caller, resolved);
          continue;
        }
        return resolved;
      }
      const parent = parents[frame.nextParent]!;
      frame.nextParent += 1;

      if (visible.has(parent) || !bySha.has(parent)) {
        extend(frame, [parent]);
      } else {
        const cached = memo.get(parent);
        if (cached !== undefined) {
          extend(frame, cached);
        } else if (visiting.has(parent)) {
          // Malformed (cyclic) input — keep the walk finite.
        } else {
          visiting.add(parent);
          stack.push({ sha: parent, nextParent: 0, resolved: [], seen: new Set() });
        }
      }
    }
  };

  const memo = new Map<string, string[]>();
  const compacted: GitHistoryCommit[] = [];
  for (const commit of commits) {
    if (!visible.has(commit.sha)) {
      continue;
    }
    const seen = new Set<string>();
    const parentShas: string[] = [];
    for (const parent of commit.parentShas) {
      for (const resolved of nearestVisibleParents(parent, memo)) {
        if (!seen.has(resolved)) {
          seen.add(resolved);
          parentShas.push(resolved);
        }
      }
    }
    compacted.push({ ...commit, parentShas });
  }
  return compacted;
}

/**
 * `collapse_branch_runs` (history.rs:760): hide the linear interior of the
 * collapsed refs' lanes — a commit is hidden iff its lane color is one of
 * the collapsed colors, it is not a ref tip itself, carries no refs of its
 * own, and is not a junction (a merge, a fork, or the lane's last commit).
 */
export function collapseBranchRuns(
  commits: readonly GitHistoryCommit[],
  collapsedRefs: ReadonlySet<string>,
  headSha: string | null,
): { visible: GitHistoryCommit[]; hiddenCounts: Map<string, number> } {
  if (collapsedRefs.size === 0 || commits.length === 0) {
    return { visible: [...commits], hiddenCounts: new Map() };
  }

  const sourceGraph = layoutGraph(commits, headSha);
  const colorsByRef = new Map<string, number>();
  for (let index = 0; index < commits.length; index += 1) {
    const row = sourceGraph.rows[index]!;
    for (const reference of commits[index]!.refs) {
      const key = branchRefKey(reference);
      if (key !== null && collapsedRefs.has(key)) {
        colorsByRef.set(key, row.nodeColorId);
      }
    }
  }
  if (colorsByRef.size === 0) {
    return { visible: [...commits], hiddenCounts: new Map() };
  }

  const collapsedColors = new Set(colorsByRef.values());
  const childCounts = new Map<string, number>();
  for (const commit of commits) {
    for (const parent of commit.parentShas) {
      childCounts.set(parent, (childCounts.get(parent) ?? 0) + 1);
    }
  }

  const visible = new Set<string>();
  const hiddenCounts = new Map<string, number>();
  for (let index = 0; index < commits.length; index += 1) {
    const commit = commits[index]!;
    const row = sourceGraph.rows[index]!;
    const isSelectedTip = commit.refs.some(
      (reference) => branchRefKey(reference) !== null && collapsedRefs.has(branchRefKey(reference)!),
    );
    const isJunction =
      commit.parentShas.length !== 1 || (childCounts.get(commit.sha) ?? 0) > 1;
    const hide =
      collapsedColors.has(row.nodeColorId) && !isSelectedTip && commit.refs.length === 0 && !isJunction;
    if (hide) {
      for (const [key, color] of colorsByRef) {
        if (color === row.nodeColorId) {
          hiddenCounts.set(key, (hiddenCounts.get(key) ?? 0) + 1);
        }
      }
    } else {
      visible.add(commit.sha);
    }
  }

  return { visible: compactCommitsToVisible(commits, visible), hiddenCounts };
}

// ---------------------------------------------------------------------------
// Search matching (repos.rs:1579-1630)
// ---------------------------------------------------------------------------

function asciiLowercase(value: string): string {
  let out = "";
  for (const ch of value) {
    out += ch >= "A" && ch <= "Z" ? ch.toLowerCase() : ch;
  }
  return out;
}

/** Case-insensitive subsequence score; lower is better (repos.rs:1579). */
function fuzzyScore(query: string, candidate: string): number | null {
  const lowered = candidate.toLowerCase();
  let total = 0;
  for (const term of query.split(/\s+/)) {
    if (term.length === 0) {
      continue;
    }
    const loweredTerm = term.toLowerCase();
    let at = 0;
    let score = 0;
    let previousEnd: number | null = null;
    for (const needle of [...loweredTerm]) {
      const found = lowered.indexOf(needle, at);
      if (found < 0) {
        return null;
      }
      score += found;
      if (previousEnd === found) {
        score = Math.max(0, score - 2);
      }
      at = found + needle.length;
      previousEnd = at;
    }
    total += score;
  }
  return total;
}

/**
 * `git_history_matches` (repos.rs:1615) — the shared matcher so the engine's
 * RPC filtering and the client's instant re-filter cannot disagree about
 * Unicode case normalization. Empty/whitespace matches everything; a sha
 * prefix (ASCII-lowercased) or a fuzzy subject match qualifies.
 */
export function gitHistoryMatches(query: string, commit: GitHistoryCommit): boolean {
  const trimmed = query.trim();
  if (trimmed.length === 0) {
    return true;
  }
  const normalized = asciiLowercase(trimmed);
  return (
    asciiLowercase(commit.sha).startsWith(normalized) ||
    fuzzyScore(trimmed, `${commit.sha} ${commit.subject}`) !== null
  );
}

// ---------------------------------------------------------------------------
// Transitions (history.rs:226-331)
// ---------------------------------------------------------------------------

export type HistoryRowTransition = "stable" | "entering" | "exiting";

export interface ScrollAnchor {
  readonly sha: string;
  readonly offsetInItem: number;
}

/**
 * `resolve_history_scroll_anchor` (history.rs:226): keep the anchored sha if
 * it survives; else walk forward to the next survivor, else backward; the
 * sub-item offset only survives an exact match.
 */
export function resolveHistoryScrollAnchor(
  anchor: ScrollAnchor | null,
  old: readonly GitHistoryCommit[],
  target: readonly GitHistoryCommit[],
): ScrollAnchor | null {
  if (anchor === null) {
    return null;
  }
  if (target.some((commit) => commit.sha === anchor.sha)) {
    return anchor;
  }
  const oldIndex = old.findIndex((commit) => commit.sha === anchor.sha);
  if (oldIndex < 0) {
    return null;
  }
  const targetShas = new Set(target.map((commit) => commit.sha));
  let replacement = old
    .slice(oldIndex + 1)
    .find((commit) => targetShas.has(commit.sha));
  if (replacement === undefined) {
    const before = old.slice(0, oldIndex);
    for (let index = before.length - 1; index >= 0; index -= 1) {
      if (targetShas.has(before[index]!.sha)) {
        replacement = before[index];
        break;
      }
    }
  }
  if (replacement === undefined) {
    return null;
  }
  return { sha: replacement.sha, offsetInItem: 0 };
}

/**
 * `history_list_splice` (history.rs:253): the minimal contiguous splice that
 * turns the old key list into the target — a trailing load-more sentinel
 * counts as one more key. `null` when nothing changed.
 */
export function historyListSplice(
  old: readonly GitHistoryCommit[],
  oldHasLoadMore: boolean,
  target: readonly GitHistoryCommit[],
  targetHasLoadMore: boolean,
): { start: number; end: number; count: number } | null {
  const oldKeys = old.map((commit) => commit.sha);
  const targetKeys = target.map((commit) => commit.sha);
  if (oldHasLoadMore) {
    oldKeys.push(LOAD_MORE_KEY);
  }
  if (targetHasLoadMore) {
    targetKeys.push(LOAD_MORE_KEY);
  }
  let prefix = 0;
  while (
    prefix < oldKeys.length &&
    prefix < targetKeys.length &&
    oldKeys[prefix] === targetKeys[prefix]
  ) {
    prefix += 1;
  }
  const suffixLimit = Math.max(Math.min(oldKeys.length, targetKeys.length) - prefix, 0);
  let suffix = 0;
  while (
    suffix < suffixLimit &&
    oldKeys[oldKeys.length - 1 - suffix] === targetKeys[targetKeys.length - 1 - suffix]
  ) {
    suffix += 1;
  }
  if (prefix === oldKeys.length && prefix === targetKeys.length) {
    return null;
  }
  return { start: prefix, end: oldKeys.length - suffix, count: targetKeys.length - prefix - suffix };
}

/**
 * `history_transition_rows` (history.rs:294): the interim list containing
 * every old row plus every target row — old-only rows sit right after their
 * preceding stable anchor (`Exiting`), target rows emit in final order
 * tagged `Stable`/`Entering`.
 */
export function historyTransitionRows(
  old: readonly GitHistoryCommit[],
  target: readonly GitHistoryCommit[],
): { commits: GitHistoryCommit[]; transitions: HistoryRowTransition[] } {
  const targetShas = new Set(target.map((commit) => commit.sha));
  const oldShas = new Set(old.map((commit) => commit.sha));
  const beforeFirst: GitHistoryCommit[] = [];
  const afterAnchor = new Map<string, GitHistoryCommit[]>();
  let anchor: string | null = null;

  for (const commit of old) {
    if (targetShas.has(commit.sha)) {
      anchor = commit.sha;
    } else if (anchor !== null) {
      const bucket = afterAnchor.get(anchor);
      if (bucket === undefined) {
        afterAnchor.set(anchor, [commit]);
      } else {
        bucket.push(commit);
      }
    } else {
      beforeFirst.push(commit);
    }
  }

  const commits: GitHistoryCommit[] = [];
  const transitions: HistoryRowTransition[] = [];
  for (const commit of beforeFirst) {
    commits.push(commit);
    transitions.push("exiting");
  }
  for (const commit of target) {
    commits.push(commit);
    transitions.push(oldShas.has(commit.sha) ? "stable" : "entering");
    const exiting = afterAnchor.get(commit.sha);
    if (exiting !== undefined) {
      afterAnchor.delete(commit.sha);
      for (const oldCommit of exiting) {
        commits.push(oldCommit);
        transitions.push("exiting");
      }
    }
  }
  // Unreachable with well-formed input; keep the rows rather than lose the
  // exit animation (the settled target still removes them).
  for (const exiting of afterAnchor.values()) {
    for (const oldCommit of exiting) {
      commits.push(oldCommit);
      transitions.push("exiting");
    }
  }
  return { commits, transitions };
}

// ---------------------------------------------------------------------------
// Graph geometry (history.rs:112-152, 938-1030)
// ---------------------------------------------------------------------------

export interface GraphGeometry {
  readonly laneCount: number;
  readonly width: number;
  readonly laneSpacing: number;
  readonly compact: boolean;
}

export function naturalGeometry(laneCount: number): GraphGeometry {
  const count = Math.max(laneCount, 1);
  return {
    laneCount: count,
    width: HISTORY_GRAPH_SIDE_PADDING + HISTORY_GRAPH_TRAILING_PADDING + HISTORY_NODE_RADIUS * 2 + (count - 1) * HISTORY_LANE_SPACING,
    laneSpacing: HISTORY_LANE_SPACING,
    compact: false,
  };
}

/** The fixed part every lane's spacing is derived against. */
const graphFixedWidth = (): number =>
  HISTORY_GRAPH_SIDE_PADDING + HISTORY_GRAPH_TRAILING_PADDING + HISTORY_NODE_RADIUS * 2;

export function fittedGeometry(laneCount: number, width: number): GraphGeometry {
  const count = Math.max(laneCount, 1);
  const natural = naturalGeometry(count);
  if (count === 1 || width >= natural.width) {
    return natural;
  }
  const fixed = graphFixedWidth();
  const clamped = Math.min(Math.max(width, fixed), natural.width);
  return {
    laneCount: count,
    width: clamped,
    laneSpacing: (clamped - fixed) / (count - 1),
    compact: false,
  };
}

export function compactGeometry(laneCount: number): GraphGeometry {
  // `natural(1)` with every lane collapsed onto its x — the rail.
  return { laneCount: Math.max(laneCount, 1), width: graphFixedWidth(), laneSpacing: 0, compact: true };
}

/** `lane_x` (history.rs:145) — device-pixel rounding is desktop-only (§6). */
export function laneX(geometry: GraphGeometry, lane: number): number {
  return HISTORY_GRAPH_SIDE_PADDING + HISTORY_NODE_RADIUS + lane * geometry.laneSpacing;
}

/**
 * `interpolate_graph_geometry` (history.rs:170): lerps width/spacing; the
 * compact flag only flips at the ends so lanes converge BEFORE the rail
 * takes over and leave it on the expand's first frame.
 */
export function interpolateGraphGeometry(
  from: GraphGeometry,
  to: GraphGeometry,
  progress: number,
): GraphGeometry {
  const clamped = Math.min(Math.max(progress, 0), 1);
  const lerp = (start: number, end: number): number => start + (end - start) * clamped;
  return {
    laneCount: to.laneCount,
    width: lerp(from.width, to.width),
    laneSpacing: lerp(from.laneSpacing, to.laneSpacing),
    compact: from.compact ? clamped <= 0.001 : to.compact && clamped >= 0.999,
  };
}

/**
 * `responsive_graph_geometry` (history.rs:938): the graph keeps natural
 * spacing when it fits, else fits into the smaller of the content budget
 * (the subject's minimum plus the optional columns) and a 34% share.
 */
export function responsiveGraphGeometry(
  laneCount: number,
  containerWidth: number,
  optionalColumnsWidth: number,
): GraphGeometry {
  const natural = naturalGeometry(laneCount);
  if (natural.width <= HISTORY_GRAPH_MIN_COMPACT_WIDTH) {
    return natural;
  }
  const contentBudget = Math.max(
    containerWidth - optionalColumnsWidth - HISTORY_COMMIT_SUBJECT_MIN_WIDTH,
    HISTORY_GRAPH_MIN_COMPACT_WIDTH,
  );
  const shareBudget = Math.max(
    containerWidth * HISTORY_GRAPH_MAX_WIDTH_RATIO,
    HISTORY_GRAPH_MIN_COMPACT_WIDTH,
  );
  return fittedGeometry(laneCount, Math.min(natural.width, Math.min(contentBudget, shareBudget)));
}

/**
 * `should_use_compact_graph` (history.rs:962) — hysteresis: the exit
 * thresholds are looser than the enter ones so the mode doesn't flap at the
 * boundary.
 */
export function shouldUseCompactGraph(
  target: GraphGeometry,
  previous: GraphGeometry,
  containerWidth: number,
  optionalColumnsWidth: number,
): boolean {
  if (target.laneCount <= 1) {
    return false;
  }
  const subjectWidth = containerWidth - optionalColumnsWidth - target.width;
  if (previous.compact) {
    return (
      subjectWidth < HISTORY_GRAPH_COMPACT_EXIT_SUBJECT_WIDTH ||
      target.laneSpacing < HISTORY_GRAPH_COMPACT_EXIT_LANE_SPACING
    );
  }
  return (
    subjectWidth < HISTORY_GRAPH_COMPACT_ENTER_SUBJECT_WIDTH ||
    target.laneSpacing < HISTORY_GRAPH_COMPACT_ENTER_LANE_SPACING
  );
}

/**
 * `stabilized_graph_geometry` (history.rs:981) — snap the fitted width to
 * the 2px resize step; a previous geometry within one step of the snapped
 * value is kept verbatim, absorbing drag jitter.
 */
export function stabilizedGraphGeometry(
  target: GraphGeometry,
  previous: GraphGeometry,
  compact: boolean,
): GraphGeometry {
  if (compact) {
    return compactGeometry(target.laneCount);
  }
  const natural = naturalGeometry(target.laneCount);
  const targetIsNatural = Math.abs(target.width - natural.width) < Number.EPSILON;
  const snappedWidth = targetIsNatural
    ? natural.width
    : Math.floor(target.width / HISTORY_GRAPH_RESIZE_STEP) * HISTORY_GRAPH_RESIZE_STEP;
  const next = fittedGeometry(target.laneCount, snappedWidth);
  if (
    !targetIsNatural &&
    previous.laneCount === next.laneCount &&
    Math.abs(previous.width - next.width) < HISTORY_GRAPH_RESIZE_STEP
  ) {
    return previous;
  }
  return next;
}

// ---------------------------------------------------------------------------
// Hover hit testing (history.rs:1006-1115)
// ---------------------------------------------------------------------------

function cubicCoordinate(start: number, control1: number, control2: number, end: number, t: number): number {
  const inverse = 1 - t;
  return (
    inverse * inverse * inverse * start +
    3 * inverse * inverse * t * control1 +
    3 * inverse * t * t * control2 +
    t * t * t * end
  );
}

function pointToSegmentDistance(
  pointX: number,
  pointY: number,
  startX: number,
  startY: number,
  endX: number,
  endY: number,
): number {
  const deltaX = endX - startX;
  const deltaY = endY - startY;
  const lengthSquared = deltaX * deltaX + deltaY * deltaY;
  if (lengthSquared <= Number.EPSILON) {
    return Math.hypot(pointX - startX, pointY - startY);
  }
  const projection = Math.min(
    Math.max(((pointX - startX) * deltaX + (pointY - startY) * deltaY) / lengthSquared, 0),
    1,
  );
  return Math.hypot(pointX - (startX + projection * deltaX), pointY - (startY + projection * deltaY));
}

function segmentDistance(
  segment: GraphSegment,
  pointX: number,
  pointY: number,
  geometry: GraphGeometry,
): number {
  const middle = HISTORY_ROW_HEIGHT / 2;
  const fromX = laneX(geometry, segment.fromLane);
  const toX = laneX(geometry, segment.toLane);
  let startY: number;
  let endY: number;
  let control1Y: number;
  let control2Y: number;
  switch (segment.shape) {
    case "incoming":
      startY = -HISTORY_GRAPH_ROW_OVERLAP;
      endY = middle;
      control1Y = middle * 0.55;
      control2Y = middle * 0.55;
      break;
    case "outgoing":
      startY = middle;
      endY = HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP;
      control1Y = middle * 1.45;
      control2Y = middle * 1.45;
      break;
    case "through":
      startY = -HISTORY_GRAPH_ROW_OVERLAP;
      endY = HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP;
      control1Y = middle;
      control2Y = middle;
      break;
  }
  if (segment.shape === "through" && segment.fromLane === segment.toLane) {
    return pointToSegmentDistance(pointX, pointY, fromX, startY, toX, endY);
  }
  let closest = Number.POSITIVE_INFINITY;
  let previousX = fromX;
  let previousY = startY;
  for (let sample = 1; sample <= HIT_SAMPLES; sample += 1) {
    const t = sample / HIT_SAMPLES;
    const currentX = cubicCoordinate(fromX, fromX, toX, toX, t);
    const currentY = cubicCoordinate(startY, control1Y, control2Y, endY, t);
    closest = Math.min(closest, pointToSegmentDistance(pointX, pointY, previousX, previousY, currentX, currentY));
    previousX = currentX;
    previousY = currentY;
  }
  return closest;
}

/**
 * `hovered_graph_path` (history.rs:1098): node first (the compact rail's x
 * counts as the node), then the closest segment within the hit radius.
 */
export function hoveredGraphPath(
  row: GraphRow,
  pointX: number,
  pointY: number,
  geometry: GraphGeometry,
): number | null {
  const nodeX = laneX(geometry, row.nodeLane);
  const nodeY = HISTORY_ROW_HEIGHT / 2;
  const nodeDistance = Math.hypot(pointX - nodeX, pointY - nodeY);
  if (nodeDistance <= HISTORY_GRAPH_HIT_RADIUS + HISTORY_NODE_RADIUS) {
    return row.nodeColorId;
  }
  if (geometry.compact && Math.abs(pointX - laneX(geometry, 0)) <= HISTORY_GRAPH_HIT_RADIUS) {
    return row.nodeColorId;
  }
  let best: number | null = null;
  let bestDistance = HISTORY_GRAPH_HIT_RADIUS;
  for (const segment of row.segments) {
    const distance = segmentDistance(segment, pointX, pointY, geometry);
    if (distance <= bestDistance) {
      best = segment.colorId;
      bestDistance = distance;
    }
  }
  return best;
}

// ---------------------------------------------------------------------------
// The palette (history.rs:3312-3319)
// ---------------------------------------------------------------------------

interface Hsla {
  readonly h: number;
  readonly s: number;
  readonly l: number;
  readonly a: number;
}

function hexToHsla(hex: string): Hsla | null {
  const body = hex.startsWith("#") ? hex.slice(1) : hex;
  if (body.length !== 3 && body.length !== 6 && body.length !== 8) {
    return null;
  }
  const channels: number[] = [];
  for (let index = 0; index < body.length; index += 2) {
    const value = Number.parseInt(body.slice(index, index + 2), 16);
    if (!Number.isFinite(value)) {
      return null;
    }
    channels.push(value);
  }
  const [r, g, b, a = 255] = channels;
  if (r === undefined || g === undefined || b === undefined) {
    return null;
  }
  const red = r / 255;
  const green = g / 255;
  const blue = b / 255;
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const l = (max + min) / 2;
  let h = 0;
  let s = 0;
  if (max !== min) {
    const delta = max - min;
    s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min);
    if (max === red) {
      h = ((green - blue) / delta + (green < blue ? 6 : 0)) / 6;
    } else if (max === green) {
      h = ((blue - red) / delta + 2) / 6;
    } else {
      h = ((red - green) / delta + 4) / 6;
    }
  }
  return { h, s, l, a: a / 255 };
}

function parseCssHsla(text: string): Hsla | null {
  const match = /^hsla?\(\s*([\d.]+)(?:deg)?\s*[, ]\s*([\d.]+)%\s*[, ]\s*([\d.]+)%\s*(?:[,/]\s*([\d.]+)\s*)?\)$/i.exec(text.trim());
  if (match === null) {
    return null;
  }
  return {
    h: Number(match[1]) / 360,
    s: Number(match[2]) / 100,
    l: Number(match[3]) / 100,
    a: match[4] === undefined ? 1 : Number(match[4]),
  };
}

/** Parse a `#rrggbb(aa)`, `rgb()`, or `hsl()` string into HSLA channels. */
export function parseCssColorHsla(text: string): Hsla | null {
  const trimmed = text.trim();
  const fromHex = hexToHsla(trimmed);
  if (fromHex !== null) {
    return fromHex;
  }
  const fromHsl = parseCssHsla(trimmed);
  if (fromHsl !== null) {
    return fromHsl;
  }
  const rgbMatch = /^rgba?\(\s*([\d.]+)\s*[, ]\s*([\d.]+)\s*[, ]\s*([\d.]+)\s*(?:[,/]\s*([\d.]+)\s*)?\)$/i.exec(trimmed);
  if (rgbMatch === null) {
    return null;
  }
  const rgbHex = toHex(Number(rgbMatch[1]), Number(rgbMatch[2]), Number(rgbMatch[3]));
  const parsed = hexToHsla(rgbHex);
  if (parsed === null) {
    return null;
  }
  return { ...parsed, a: rgbMatch[4] === undefined ? 1 : Number(rgbMatch[4]) };
}

function toHex(r: number, g: number, b: number): string {
  return [r, g, b]
    .map((value) => Math.round(Math.min(Math.max(value, 0), 255)).toString(16).padStart(2, "0"))
    .join("");
}

/**
 * `graph_color` (history.rs:1300): multiplies HSL saturation by
 * `HISTORY_GRAPH_SATURATION`, leaving hue/lightness/alpha untouched. Accepts
 * any CSS color the `--rb-*` tokens emit; returns an `hsla()` string.
 */
export function graphColor(color: string): string {
  const parsed = parseCssColorHsla(color);
  if (parsed === null) {
    return color;
  }
  const h = parsed.h * 360;
  const s = parsed.s * HISTORY_GRAPH_SATURATION * 100;
  const l = parsed.l * 100;
  const a = parsed.a;
  return `hsla(${h}, ${s}%, ${l}%, ${a})`;
}

// ---------------------------------------------------------------------------
// Ref badge sizing (history.rs:1136-1167)
// ---------------------------------------------------------------------------

/** `estimated_ref_badge_width` — icon + gap + padding + the label estimate. */
export function estimatedRefBadgeWidth(reference: GitHistoryRef): number {
  return Math.min(22 + [...reference.label].length * 5.7, HISTORY_REF_BADGE_MAX_WIDTH);
}

export function estimatedRefOverflowWidth(hidden: number): number {
  return [...`+${hidden}`].length * 5.7;
}

/** `ref_area_width` — capped at 45% of the inner width, never into the subject. */
export function refAreaWidth(commitColumnWidth: number): number {
  const inner = Math.max(commitColumnWidth - 8, 0);
  return Math.min(
    inner * HISTORY_REF_AREA_RATIO,
    Math.max(inner - HISTORY_COMMIT_SUBJECT_MIN_WIDTH - HISTORY_REF_GAP, 0),
  );
}

/** `visible_ref_count` — the largest badge prefix (plus overflow chip) that fits. */
export function visibleRefCount(refs: readonly GitHistoryRef[], availableWidth: number): number {
  if (refs.length === 0) {
    return 0;
  }
  let visible = 0;
  for (let count = 1; count <= refs.length; count += 1) {
    const hidden = refs.length - count;
    const itemCount = count + (hidden > 0 ? 1 : 0);
    let badges = 0;
    for (let index = 0; index < count; index += 1) {
      badges += estimatedRefBadgeWidth(refs[index]!);
    }
    const overflow = hidden > 0 ? estimatedRefOverflowWidth(hidden) : 0;
    const gaps = Math.max(itemCount - 1, 0) * HISTORY_REF_GAP;
    if (badges + overflow + gaps <= availableWidth) {
      visible = count;
    }
  }
  return visible;
}

/** `ref_description` (history.rs:1252). */
export function refDescription(reference: GitHistoryRef): string {
  switch (reference.kind) {
    case "branch":
      return `Branch: ${reference.label}`;
    case "remote":
      return `Remote branch: ${reference.label}`;
    case "tag":
      return `Tag: ${reference.label}`;
  }
}

// ---------------------------------------------------------------------------
// Text helpers (history.rs:1169-1298)
// ---------------------------------------------------------------------------

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** `format_date` — RFC 3339 → `MMM D, YYYY`; unparsable → em dash. */
export function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "—";
  }
  const month = MONTHS[date.getMonth()];
  const day = date.getDate();
  const year = date.getFullYear();
  if (month === undefined) {
    return "—";
  }
  return `${month} ${day}, ${year}`;
}

/** `history_author_name` (history.rs:1274). */
export function historyAuthorName(name: string): string {
  return name.trim().length === 0 ? "Unknown" : name;
}

/** `history_author_initial` (history.rs:1283) — first visible char, uppercased. */
export function historyAuthorInitial(name: string): string {
  for (const character of name) {
    if (!/\s/.test(character)) {
      return character.toUpperCase();
    }
  }
  return "?";
}

const AVATAR_FORMATS: ReadonlyArray<{ readonly mime: string; readonly signature: number[] }> = [
  { mime: "image/png", signature: [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] },
  { mime: "image/jpeg", signature: [0xff, 0xd8, 0xff] },
  { mime: "image/gif", signature: [0x47, 0x49, 0x46, 0x38, 0x37, 0x61] },
  { mime: "image/gif", signature: [0x47, 0x49, 0x46, 0x38, 0x39, 0x61] },
];

function base64ToBytes(encoded: string): Uint8Array | null {
  try {
    const binary = atob(encoded);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return bytes;
  } catch {
    return null;
  }
}

/**
 * `decode_history_avatar` (history.rs:1285), web-shaped: sniff the magic
 * bytes for the mime prefix (the server sends none and browsers honor the
 * declared mime strictly), unknown/bad base64 → `null`.
 */
export function decodeHistoryAvatar(encoded: string): string | null {
  const bytes = base64ToBytes(encoded);
  if (bytes === null) {
    return null;
  }
  let mime: string | null = null;
  for (const format of AVATAR_FORMATS) {
    if (bytes.length >= format.signature.length && format.signature.every((byte, index) => bytes[index] === byte)) {
      mime = format.mime;
      break;
    }
  }
  if (mime === null && bytes.length >= 12) {
    const riff = String.fromCharCode(...bytes.slice(0, 4));
    const webp = String.fromCharCode(...bytes.slice(8, 12));
    if (riff === "RIFF" && webp === "WEBP") {
      mime = "image/webp";
    }
  }
  if (mime === null) {
    return null;
  }
  return `data:${mime};base64,${encoded}`;
}

// ---------------------------------------------------------------------------
// Column layout math (history.rs:460-627)
// ---------------------------------------------------------------------------

export type HistoryColumn = "author" | "date" | "sha";
export type HistoryDataColumn = "commit" | HistoryColumn;

export interface HistoryColumnWidths {
  readonly author: number;
  readonly date: number;
  readonly sha: number;
}

export interface HistoryColumnVisibility {
  readonly author: boolean;
  readonly date: boolean;
  readonly sha: boolean;
}

export const HISTORY_COLUMN_LABELS: Readonly<Record<HistoryColumn, string>> = {
  author: "Author",
  date: "Date",
  sha: "SHA",
};

export function historyColumnWidth(column: HistoryDataColumn, widths: HistoryColumnWidths): number {
  switch (column) {
    case "commit":
      return HISTORY_COMMIT_SUBJECT_MIN_WIDTH;
    case "author":
      return widths.author;
    case "date":
      return widths.date;
    case "sha":
      return widths.sha;
  }
}

export function historyColumnLimits(column: HistoryDataColumn): { min: number; max: number } {
  switch (column) {
    case "commit":
      return { min: HISTORY_COMMIT_SUBJECT_MIN_WIDTH, max: Number.POSITIVE_INFINITY };
    case "author":
      return { min: HISTORY_AUTHOR_MIN, max: HISTORY_AUTHOR_MAX };
    case "date":
      return { min: HISTORY_DATE_MIN, max: HISTORY_DATE_MAX };
    case "sha":
      return { min: HISTORY_SHA_MIN, max: HISTORY_SHA_MAX };
  }
}

function setHistoryColumnWidth(
  widths: HistoryColumnWidths,
  column: HistoryDataColumn,
  width: number,
): HistoryColumnWidths {
  switch (column) {
    case "commit":
      return widths;
    case "author":
      return { ...widths, author: width };
    case "date":
      return { ...widths, date: width };
    case "sha":
      return { ...widths, sha: width };
  }
}

export interface HistoryColumnDragAnchor {
  readonly left: HistoryDataColumn;
  readonly right: HistoryDataColumn;
  readonly leftWidth: number;
  readonly rightWidth: number;
}

/**
 * `resized_history_column_widths` (history.rs:598): the Commit divider only
 * resizes its one fixed neighbor; an interior divider preserves the pair's
 * total, clamping so neither side passes its own limits.
 */
export function resizedHistoryColumnWidths(
  widths: HistoryColumnWidths,
  anchor: HistoryColumnDragAnchor,
  requestedDelta: number,
): HistoryColumnWidths {
  if (anchor.left === "commit") {
    const { min, max } = historyColumnLimits(anchor.right);
    return setHistoryColumnWidth(
      widths,
      anchor.right,
      Math.min(Math.max(anchor.rightWidth - requestedDelta, min), max),
    );
  }
  const left = historyColumnLimits(anchor.left);
  const right = historyColumnLimits(anchor.right);
  const minDelta = Math.max(left.min - anchor.leftWidth, anchor.rightWidth - right.max);
  const maxDelta = Math.min(left.max - anchor.leftWidth, anchor.rightWidth - right.min);
  const delta = Math.min(Math.max(requestedDelta, minDelta), maxDelta);
  return setHistoryColumnWidth(
    setHistoryColumnWidth(widths, anchor.left, anchor.leftWidth + delta),
    anchor.right,
    anchor.rightWidth - delta,
  );
}

/**
 * `history_column_drop_index` (history.rs:504): rescale the pointer's x from
 * the rendered width to the desired total, then walk the column midpoints.
 */
export function historyColumnDropIndex(
  relativeX: number,
  renderedWidth: number,
  columns: readonly HistoryColumn[],
  widths: HistoryColumnWidths,
): number {
  if (columns.length === 0 || renderedWidth <= 0) {
    return 0;
  }
  const desiredWidth = columns.reduce(
    (total, column) => total + historyColumnWidth(column, widths),
    0,
  );
  const x = Math.min(Math.max(relativeX, 0), renderedWidth) * desiredWidth / renderedWidth;
  let cursor = 0;
  for (let index = 0; index < columns.length; index += 1) {
    const width = historyColumnWidth(columns[index]!, widths);
    if (x < cursor + width / 2) {
      return index;
    }
    cursor += width;
  }
  return columns.length - 1;
}

/**
 * `reordered_history_columns` (history.rs:529): remove the dragged column,
 * reinsert after the target when moving rightward, before it otherwise.
 */
export function reorderedHistoryColumns(
  order: readonly HistoryColumn[],
  dragged: HistoryColumn,
  target: HistoryColumn,
): HistoryColumn[] {
  if (dragged === target) {
    return [...order];
  }
  const from = order.indexOf(dragged);
  if (from < 0) {
    return [...order];
  }
  const over = order.indexOf(target);
  if (over < 0) {
    return [...order];
  }
  const columns = order.filter((column) => column !== dragged);
  const targetAfterRemoval = columns.indexOf(target);
  const insertion = from < over ? targetAfterRemoval + 1 : targetAfterRemoval;
  const clamped = Math.min(insertion, columns.length);
  return [...columns.slice(0, clamped), dragged, ...columns.slice(clamped)];
}

/** `visible_history_columns` (history.rs:491). */
export function visibleHistoryColumns(
  order: readonly HistoryColumn[],
  columns: HistoryColumnVisibility,
): HistoryColumn[] {
  return order.filter((column) =>
    column === "author" ? columns.author : column === "date" ? columns.date : columns.sha,
  );
}

/** `GitHistoryColumnOrder::normalized` — dedupe, append missing. */
export function normalizedHistoryColumnOrder(value: readonly unknown[]): HistoryColumn[] {
  const columns: HistoryColumn[] = [];
  for (const entry of value) {
    if (
      (entry === "author" || entry === "date" || entry === "sha") &&
      !columns.includes(entry)
    ) {
      columns.push(entry);
    }
  }
  for (const column of ["author", "date", "sha"] as const) {
    if (!columns.includes(column)) {
      columns.push(column);
    }
  }
  return columns;
}
