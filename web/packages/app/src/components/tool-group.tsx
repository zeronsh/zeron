import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type ReactNode,
} from "react";
import { Icon } from "@zeron/icons";
import type { FetchToolBlobReply } from "@zeron/proto";
import { methods } from "@zeron/engine-client";
import { useResolvedAppearance } from "../state/appearance";
import { useUiSettings } from "../state/ui-settings";
import { diffLineHeight as scaledDiffLineHeight } from "../lib/typography";
import type { FileDiff } from "../lib/diff";
import type { InlineRun } from "../lib/markdown";
import {
  CHIP_CARD_HEIGHT,
  CHIP_HEIGHT,
  fileBadgeName,
  isAgentTool,
  isSpawnLink,
  subagentModel,
  subagentTabTitle,
  toolChipContent,
  toolGroupTitle,
  toolIconName,
  type ToolDetail,
  type ToolItem,
} from "../lib/transcript";
import { wellBg } from "../lib/file-icons";
import { toolGroupGeometry } from "../lib/tool-group-geometry";
import {
  FOLD_TWEEN_WINDOW_MS,
  ACTIVITY_TEXT_GAP,
  toolConnectorContinuation,
  toolConnectorParts,
  toolDisclosureProgress,
  toolRevealClock,
  ToolGroupMotionStore,
  type FoldState,
} from "../lib/tool-motion";
import { ActivityRail } from "./activity-rail";
import { FileBodyUpto, FilePlaneScroll } from "./diff-view";
import { FileIcon } from "./files/file-icon";
import { GlyphSpinner } from "./glyph-spinner";

/**
 * The transcript's task tree — the desktop's `render_tool_group`
 * (transcript.rs:5837) and its chip family (`chip_header_row` :6871,
 * `tool_chip` :7327, `subagent_chip` :7392, `detail_body` :6688).
 *
 * A group is either **collapsible** (at least one non-agent tool — a quiet
 * summary header with a shimmering title and a chevron) or a **standalone
 * spawn card row** (all agent chips, always open, no header). Each step is a
 * 32px rail row drawn against the activity rail, staggering in on arrival
 * (90ms first-row delay for a new group, 65ms per arrival, 360ms height
 * clip, 480ms connector draw, a 4px lift + fade on the content only). Every
 * step expands in place: invocation block, separator, detail block, then a
 * blob affordance row when one is offered; spawn chips are LINKS whose whole
 * card opens the subagent's transcript as a right-pane tab.
 *
 * While any tween/reveal is unfinished the row re-renders on the SHARED rAF
 * clock (ticket 59, the desktop's invisible per-frame canvas — ONE loop for
 * every live row, armed by the first subscriber and stopped by the last);
 * per-row timings are unchanged.
 */

// ---------------------------------------------------------------------------
// The host's contract
// ---------------------------------------------------------------------------

/** `TranscriptEvent::OpenSubagent` (transcript.rs:2740). */
export interface SubagentOpen {
  readonly chatId: string;
  readonly docId: string;
  readonly title: string;
  readonly frozen: boolean;
}

export interface ToolGroupRowProps {
  readonly rowId: string;
  readonly tools: readonly ToolItem[];
  /** Set at row build time: streaming && this group is the entry's LAST part. */
  readonly autoOpen: boolean;
  /** The transcript's chat id (the primary chat, or the subagent doc itself). */
  readonly chatId: string;
  /** The surface's tool-motion store (folds, reveals, blob fetches). */
  readonly motion: ToolGroupMotionStore;
  readonly client: {
    call: <T>(method: string, params: unknown) => Promise<T>;
  };
  readonly onOpenSubagent: (payload: SubagentOpen) => void;
  /**
   * The explicit fold-navigation callback (ticket 71 A): called with the
   * CLICKED header element synchronously, BEFORE the fold state flips, so
   * the scroller can capture the header's screen position, release the
   * follow/hold (retaining any live reservation), and arm the compensation.
   */
  readonly onFoldNav?: (nav: ToolFoldNav) => void;
}

/** One explicit fold click's navigation payload (ticket 71 A). */
export interface ToolFoldNav {
  readonly rowId: string;
  /** The clicked header button/card head — its rect is the anchor. */
  readonly header: HTMLElement;
}

const prefersReducedMotion = (): boolean =>
  typeof globalThis.matchMedia === "function" &&
  globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches;

// ---------------------------------------------------------------------------
// The group row (render_tool_group :5837)
// ---------------------------------------------------------------------------

export function ToolGroupRow({ rowId, tools, autoOpen, chatId, motion, client, onOpenSubagent, onFoldNav }: ToolGroupRowProps) {
  // Folds/fetches/reveals live in the surface's store: a virtualized row
  // scrolling back into view is a remount and must find its fold.
  useSyncExternalStore(motion.subscribe, motion.getVersion);
  const reduced = prefersReducedMotion();
  const [now, setNow] = useState(() => performance.now());
  const diffLine = scaledDiffLineHeight(useUiSettings().codeFontSize);

  // The SHARED geometry contract (ticket 70): the scroller's estimator calls
  // the same pure resolver with the same inputs — including the code-size-
  // scaled diff row — so an unmeasured group mounts at the height this row
  // actually renders.
  const geometry = toolGroupGeometry({ rowId, tools, autoOpen, state: motion, now, reduced, diffLineHeight: diffLine });
  const {
    collapses,
    open,
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
    motionActive,
  } = geometry;
  const fold = motion.groupFold(rowId);
  const active = collapses && autoOpen;

  // The SHARED rAF clock (ticket 59): the desktop keeps requesting frames
  // while a tween/reveal is unfinished — ONE loop drives every live row and
  // stops when the last row's progress reaches 1. The row still computes
  // every progress from the delivered `now`, so its timings are untouched.
  useEffect(() => {
    if (!motionActive) {
      return;
    }
    return toolRevealClock.subscribe(setNow);
  }, [motionActive]);

  // The rendered-open flip without a user click (auto-open expiring) seeds
  // the fold's tween from the last RENDERED height (:5862-5870) — in a
  // layout effect so the seeded re-render lands before paint.
  useLayoutEffect(() => {
    motion.noteRendered(rowId, open, bodyHeight);
  });

  // Ticket 71 B — the renderer's card-height report per expandable chip:
  // while a thought streams open this records the settled open height that
  // the animated completion close tweens from. One effect per group row,
  // fed by the SHARED geometry (the estimator's numbers agree by contract).
  useLayoutEffect(() => {
    for (let ix = 0; ix < tools.length; ix += 1) {
      if (details[ix] === null && invocations[ix] === null) {
        continue;
      }
      motion.noteDetailRendered(
        `${rowId}#d${ix}`,
        (rowHeights[ix] ?? baseRowHeight) - baseRowHeight + CHIP_CARD_HEIGHT,
      );
    }
  });

  const disclosure = reduced ? (open ? 1 : 0) : toolDisclosureProgress(open, fold, now);

  const onToggleGroup = useCallback(
    (event: React.MouseEvent) => {
      event.stopPropagation();
      // The click owns the viewport FIRST: the header is measured at its
      // pre-toggle position and the follow/hold releases before the fold
      // state flips (ticket 71 A).
      onFoldNav?.({ rowId, header: event.currentTarget as HTMLElement });
      motion.toggleGroupFold(rowId, revealedHeight, effectiveAutoOpen);
    },
    [rowId, revealedHeight, effectiveAutoOpen, motion, onFoldNav],
  );

  const fetchBlob = useCallback(
    (ref: string): void => {
      motion.beginBlobFetch(ref, () =>
        client.call<FetchToolBlobReply>(methods.FETCH_TOOL_BLOB, { blobRef: ref }).then((reply) => reply.text),
      );
    },
    [client, motion],
  );

  const onOpenSubagentFor = useCallback(
    (tool: ToolItem): void => {
      if (tool.subagentRef === null) {
        return;
      }
      const frozen = tool.subagentStatus === "done" || tool.subagentStatus === "failed";
      onOpenSubagent({
        chatId,
        docId: tool.subagentRef,
        title: subagentTabTitle(tool.call),
        frozen,
      });
    },
    [chatId, onOpenSubagent],
  );

  const shimmerActive = active && !reduced;

  const chips = tools.map((tool, ix) => (
    <ToolChipRow
      key={ix}
      rowId={rowId}
      ix={ix}
      tool={tool}
      collapses={collapses}
      continues={ix + 1 < tools.length}
      baseRowHeight={baseRowHeight}
      rowHeight={rowHeights[ix] ?? baseRowHeight}
      revealProgress={revealProgress[ix] ?? 1}
      connectorReveal={connectorProgress[ix] ?? 1}
      continuationReveal={toolConnectorContinuation(ix + 1 < tools.length ? (connectorProgress[ix + 1] ?? null) : null)}
      detail={details[ix] ?? null}
      invocation={invocations[ix] ?? null}
      detailFold={detailFolds[ix] ?? null}
      detailOpen={detailOpens[ix] ?? false}
      affordance={affordances[ix] ?? null}
      now={now}
      motion={motion}
      fetchBlob={fetchBlob}
      onOpenSubagent={onOpenSubagentFor}
      onFoldNav={onFoldNav}
    />
  ));

  if (!collapses) {
    // A spawn-only group renders its chips unwrapped — no fold, no header.
    return <div className="tool-group">{chips}</div>;
  }

  return (
    <div className="tool-group">
      <div className="tool-reveal" style={{ height: headerHeight }}>
        <ToolGroupHeader
          rowId={rowId}
          summary={toolGroupTitle(tools)}
          open={open}
          disclosure={disclosure}
          shimmer={shimmerActive}
          onToggle={onToggleGroup}
        />
      </div>
      <div className="tool-group-fold" style={{ height: bodyHeight }}>
        <div className="tool-group-body">{chips}</div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The group header (:6107-6158)
// ---------------------------------------------------------------------------

function ToolGroupHeader({
  rowId,
  summary,
  open,
  disclosure,
  shimmer,
  onToggle,
}: {
  rowId: string;
  summary: string;
  open: boolean;
  disclosure: number;
  shimmer: boolean;
  onToggle: (event: React.MouseEvent) => void;
}) {
  // −90° closed → 0° open over TOOL_FOLD.
  const rotation = -90 * (1 - disclosure);
  return (
    <button type="button" id={`${rowId}-hdr`} className="tool-group-header" aria-expanded={open} onClick={onToggle}>
      <span className="tool-group-chevron" aria-hidden>
        <Icon name="altArrowDown" size={14} style={{ transform: `rotate(${rotation}deg)` }} />
      </span>
      <span className={`tool-group-title ${shimmer ? "tool-shimmer" : ""}`}>{summary}</span>
    </button>
  );
}

// ---------------------------------------------------------------------------
// One chip row — plain rail chip, expandable card, or spawn link
// ---------------------------------------------------------------------------

interface ToolChipRowProps {
  readonly rowId: string;
  readonly ix: number;
  readonly tool: ToolItem;
  readonly collapses: boolean;
  readonly continues: boolean;
  readonly baseRowHeight: number;
  readonly rowHeight: number;
  readonly revealProgress: number;
  readonly connectorReveal: number;
  readonly continuationReveal: number;
  readonly detail: ToolDetail | null;
  readonly invocation: ToolDetail | null;
  readonly detailFold: FoldState | null;
  readonly detailOpen: boolean;
  readonly affordance: { ref: string; label: string; loading: boolean } | null;
  readonly now: number;
  readonly motion: ToolGroupMotionStore;
  readonly fetchBlob: (ref: string) => void;
  readonly onOpenSubagent: (tool: ToolItem) => void;
  /** The explicit fold-navigation callback (ticket 71 A). */
  readonly onFoldNav?: (nav: ToolFoldNav) => void;
}

function ToolChipRow(props: ToolChipRowProps) {
  const { tool, collapses, ix, rowHeight, revealProgress, detail, invocation } = props;

  // A spawn link: the WHOLE card opens the subagent's tab (:6175-6197).
  if (isSpawnLink(tool)) {
    return <SubagentChip tool={tool} rail={collapses} onOpen={() => props.onOpenSubagent(tool)} />;
  }

  const { connectorReveal, continuationReveal } = props;
  const contentReveal = toolConnectorParts(connectorReveal, ix > 0).branch;
  const expandable = detail !== null || invocation !== null;

  if (!expandable) {
    // `tool_chip` (:7327) — a detail-less row: rail + a borderless card.
    return revealRow(
      <div className="tool-chip" style={{ height: props.baseRowHeight }}>
        {collapses && (
          <ActivityRail
            tool={tool}
            hasPredecessor={ix > 0}
            continues={props.continues}
            connectorReveal={connectorReveal}
            continuationReveal={continuationReveal}
            bendRowHeight={props.baseRowHeight}
            canvasHeight={props.baseRowHeight}
          />
        )}
        <div
          className={`tool-chip-card ${collapses ? "" : "tool-chip-card-bordered"}`}
          style={{
            height: CHIP_CARD_HEIGHT,
            marginTop: (props.baseRowHeight - CHIP_CARD_HEIGHT) / 2,
            marginBottom: (props.baseRowHeight - CHIP_CARD_HEIGHT) / 2,
            // The rail margin (transcript.rs:7363): the label breaks 8px off
            // the rail icon when the rail renders.
            marginLeft: collapses ? ACTIVITY_TEXT_GAP : undefined,
            ...(collapses && contentReveal < 1 ? liftStyle(contentReveal) : null),
          }}
        >
          <ChipHeaderRow tool={tool} trail={null} />
        </div>
      </div>,
      rowHeight,
      revealProgress,
    );
  }

  // The expandable card (:6231-6352).
  const key = `${props.rowId}#d${ix}`;
  const animating =
    props.detailFold !== null &&
    props.detailFold.epoch > 0 &&
    props.detailFold.toggledAt !== null &&
    props.now - props.detailFold.toggledAt < FOLD_TWEEN_WINDOW_MS;
  const cardHeight = rowHeight - props.baseRowHeight + CHIP_CARD_HEIGHT;
  const defaultOpen = tool.isThought && !tool.resolved;
  const onToggle = (event: React.MouseEvent): void => {
    event.stopPropagation();
    // The click owns the viewport FIRST (ticket 71 A): measure the chip
    // header at its pre-toggle position and release the follow/hold
    // before the fold state flips.
    props.onFoldNav?.({ rowId: props.rowId, header: event.currentTarget as HTMLElement });
    props.motion.toggleDetailFold(key, cardHeight, defaultOpen);
  };
  return revealRow(
    <div className="tool-chip" style={{ height: rowHeight }}>
      {collapses && (
        <ActivityRail
          tool={tool}
          hasPredecessor={ix > 0}
          continues={props.continues}
          connectorReveal={connectorReveal}
          continuationReveal={continuationReveal}
          bendRowHeight={props.baseRowHeight}
          canvasHeight={rowHeight}
        />
      )}
      <div
        className={`tool-chip-card tool-chip-card-expandable ${collapses ? "" : "tool-chip-card-bordered"}`}
        style={{
          height: cardHeight,
          marginTop: (props.baseRowHeight - CHIP_CARD_HEIGHT) / 2,
          marginBottom: (props.baseRowHeight - CHIP_CARD_HEIGHT) / 2,
          // The rail margin (transcript.rs:6233): same 8px icon→label break
          // when the rail renders.
          marginLeft: collapses ? ACTIVITY_TEXT_GAP : undefined,
          ...(collapses && contentReveal < 1 ? liftStyle(contentReveal) : null),
        }}
      >
        <ChipHeaderRow tool={tool} trail="chevron" open={props.detailOpen} toggle={{ onToggle, defaultOpen }} />
        {/* The body stays mounted while the close tween shrinks over it
            (FOLD_TWEEN_WINDOW). */}
        {(props.detailOpen || animating) && (
          <ToolDetailPane
            invocation={invocation}
            detail={detail}
            affordance={props.affordance}
            collapses={collapses}
            fetchBlob={props.fetchBlob}
          />
        )}
      </div>
    </div>,
    rowHeight,
    revealProgress,
  );
}

/** The content-only 4px lift + fade (the connector keeps full contrast). */
function liftStyle(contentReveal: number): CSSProperties {
  return {
    transform: `translateY(${4 * (1 - contentReveal)}px)`,
    opacity: contentReveal,
  };
}

/** `reveal_tool_row` (:7210) — only the height clips. */
function revealRow(children: ReactNode, height: number, progress: number): ReactNode {
  if (progress >= 1) {
    return children;
  }
  return (
    <div className="tool-reveal" style={{ height: height * progress }}>
      {children}
    </div>
  );
}

// ---------------------------------------------------------------------------
// `chip_header_row` (:6871) — the chip's content row
// ---------------------------------------------------------------------------

function ChipHeaderRow({
  tool,
  trail,
  open = false,
  toggle,
}: {
  tool: ToolItem;
  trail: "chevron" | "openArrow" | null;
  open?: boolean;
  /**
   * The expandable card's click target — the desktop's `chip_header` is the
   * SAME row with `cursor_pointer` + the toggle, never a wrapper.
   */
  toggle?: { onToggle: (event: React.MouseEvent) => void; defaultOpen: boolean };
}) {
  const appearance = useResolvedAppearance();
  const activity = !isAgentTool(tool);
  const { label, detail } = tool.isThought
    ? { label: "Thought process", detail: "" }
    : toolChipContent(tool.call);
  const filePath =
    tool.call.kind === "readFile" || tool.call.kind === "writeFile" || tool.call.kind === "editFile"
      ? tool.call.path
      : tool.call.kind === "applyPatch" && tool.call.path !== null && tool.call.path !== undefined
        ? tool.call.path
        : null;
  const running = tool.subagentRef !== null && tool.subagentStatus === "running";
  const failed = tool.isError || (tool.subagentRef !== null && tool.subagentStatus === "failed");
  // Group-hover lights the label/detail/chevron only on expandable rail rows
  // that did not fail (:6898).
  const hoverText = activity && trail !== null && !failed;
  const model = tool.call.kind === "unknown" || tool.call.kind === "mcp" ? subagentModel(tool.call) : null;

  return (
    <div
      className={`tool-chip-head ${activity ? "" : "tool-chip-head-card"} ${toggle !== undefined ? "tool-chip-head-button" : ""}`}
      data-hover-text={hoverText ? "1" : undefined}
      data-failed={failed ? "1" : undefined}
      role={toggle !== undefined ? "button" : undefined}
      tabIndex={toggle !== undefined ? 0 : undefined}
      aria-expanded={toggle !== undefined ? open : undefined}
      onClick={toggle?.onToggle}
      onKeyDown={
        toggle !== undefined
          ? (event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                toggle.onToggle(event as unknown as React.MouseEvent);
              }
            }
          : undefined
      }
    >
      {!activity && (
        <span className="tool-chip-icon-tile" aria-hidden>
          <Icon name={tool.isThought ? "chatRoundLine" : toolIconName(tool.call)} size={12} />
        </span>
      )}
      <span className="tool-chip-label">{label}</span>
      {filePath !== null ? (
        <FileBadge path={filePath} failed={failed} appearance={appearance} />
      ) : activity && detail.length === 0 ? null : (
        <span className="tool-chip-detail">{detail}</span>
      )}
      {model !== null && <span className="tool-chip-model">{model}</span>}
      {running && <GlyphSpinner size={8} className="tool-chip-spinner" />}
      {trail !== null && (
        <span className={`tool-chip-trail ${activity ? "" : "tool-chip-trail-card"} ${trail === "openArrow" ? "tool-chip-trail-open" : ""}`}>
          {trail === "chevron" ? (
            <Icon name={open ? "altArrowDown" : "altArrowRight"} size={12} />
          ) : (
            <Icon name="arrowUpRight" size={11} />
          )}
        </span>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// `FileBadge` — the frosted file-action chip (:6986-7037)
// ---------------------------------------------------------------------------

function FileBadge({
  path,
  failed,
  appearance,
}: {
  path: string;
  failed: boolean;
  appearance: "light" | "dark";
}) {
  return (
    <span className="tool-chip-detail-slot">
      <span className={`tool-file-badge ${failed ? "tool-file-badge-failed" : ""}`}>
        <span className="tool-file-badge-well" style={{ background: wellBg(appearance, true) }}>
          <FileIcon kind="file" name={path} appearance={appearance} size={14} />
        </span>
        <span className="tool-file-badge-name">{fileBadgeName(path)}</span>
      </span>
    </span>
  );
}

// ---------------------------------------------------------------------------
// `subagent_chip` (:7392) — the spawn link
// ---------------------------------------------------------------------------

function SubagentChip({ tool, rail, onOpen }: { tool: ToolItem; rail: boolean; onOpen: () => void }) {
  return (
    <div className="tool-chip" style={{ height: CHIP_HEIGHT }}>
      {rail && <span className="tool-agent-guide" aria-hidden />}
      <div
        className="tool-chip-card tool-chip-card-bordered tool-agent-link"
        role="button"
        tabIndex={0}
        title="Open subagent"
        style={{
          height: CHIP_CARD_HEIGHT,
          marginTop: (CHIP_HEIGHT - CHIP_CARD_HEIGHT) / 2,
          marginBottom: (CHIP_HEIGHT - CHIP_CARD_HEIGHT) / 2,
          marginLeft: rail ? 12 : undefined,
        }}
        onClick={onOpen}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            onOpen();
          }
        }}
      >
        <ChipHeaderRow tool={tool} trail="openArrow" />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The detail panel + bodies (:6273-6324, `detail_body` :6688)
// ---------------------------------------------------------------------------

function ToolDetailPane({
  invocation,
  detail,
  affordance,
  collapses,
  fetchBlob,
}: {
  invocation: ToolDetail | null;
  detail: ToolDetail | null;
  affordance: { ref: string; label: string; loading: boolean } | null;
  collapses: boolean;
  fetchBlob: (ref: string) => void;
}) {
  return (
    <div className="tool-chip-body">
      {invocation !== null && (
        <>
          <div className={`tool-detail-separator ${collapses ? "tool-detail-separator-rail" : ""}`} />
          <DetailBody detail={invocation} invocation />
        </>
      )}
      {detail !== null && (
        <>
          <div className={`tool-detail-separator ${collapses ? "tool-detail-separator-rail" : ""}`} />
          <DetailBody detail={detail} />
        </>
      )}
      {affordance !== null && (
        <button
          type="button"
          className="tool-full-button"
          disabled={affordance.loading}
          onClick={(event) => {
            event.stopPropagation();
            fetchBlob(affordance.ref);
          }}
        >
          {affordance.label}
        </button>
      )}
    </div>
  );
}

function DetailBody({ detail, invocation = false }: { detail: ToolDetail; invocation?: boolean }) {
  const appearance = useResolvedAppearance();
  switch (detail.kind) {
    case "output":
      return (
        <div className={`tool-output ${invocation ? "tool-invocation" : ""}`}>
          {detail.lines.map((line, ix) => (
            <div key={ix} className="tool-output-line">
              <span className="tool-output-text">{line}</span>
            </div>
          ))}
          {detail.truncatedBy > 0 && (
            <div className="tool-output-line tool-output-more">… {detail.truncatedBy} more lines</div>
          )}
        </div>
      );
    case "thought":
      return (
        <div className="tool-output tool-thought">
          {detail.lines.map((line, ix) => (
            <div key={ix} className="tool-output-line">
              {line.length === 0 ? null : (
                <span className="tool-thought-line">
                  {line.map((run, runIx) => (
                    <ThoughtRun key={runIx} run={run} />
                  ))}
                </span>
              )}
            </div>
          ))}
          {detail.truncatedBy > 0 && (
            <div className="tool-output-line tool-output-more">… {detail.truncatedBy} more lines</div>
          )}
        </div>
      );
    case "stats":
      return (
        <div className="tool-output tool-stats">
          {detail.stats.map((stat, ix) => (
            <div key={ix} className="tool-stat-row">
              <FileIcon kind="file" name={stat.path} appearance={appearance} size={14} />
              <span className="tool-stat-path">{stat.path}</span>
              <span className="tool-stat-add">+{stat.additions}</span>
              <span className="tool-stat-del">−{stat.deletions}</span>
            </div>
          ))}
        </div>
      );
    case "diff":
      return <ToolDiffBody file={detail.file} />;
  }
}

/**
 * `thought_line_text` (:6812) — faint prose, semibold bold, mono code,
 * underlined links that are NOT clickable (a thought is a record, not a
 * surface), 1px strikethrough.
 */
function ThoughtRun({ run }: { run: InlineRun }) {
  let content: ReactNode = run.text;
  if (run.style.code) {
    content = <code className="tool-thought-code">{content}</code>;
  }
  if (run.style.bold) {
    content = <strong className="tool-thought-strong">{content}</strong>;
  }
  if (run.style.italic) {
    content = <em>{content}</em>;
  }
  if (run.style.strikethrough) {
    content = <s>{content}</s>;
  }
  if (run.style.link !== null && run.style.link !== undefined) {
    // Underlined span, never an anchor — a thought link is decoration.
    content = <span className="tool-thought-link">{content}</span>;
  }
  return <>{content}</>;
}

/**
 * The diff detail: the Changes pane's own body renderer (`FileBodyUpto`,
 * ticket 22's port of `render_file_body_with_syntax`), comments disabled —
 * an inline tool diff is a record, not a review surface.
 */
function ToolDiffBody({ file }: { file: FileDiff }) {
  const scrollRef = useRef<FilePlaneScroll | null>(null);
  if (scrollRef.current === null) {
    scrollRef.current = new FilePlaneScroll();
  }
  const lineHeight = scaledDiffLineHeight(useUiSettings().codeFontSize);
  return (
    <div className="tool-diff-body">
      <FileBodyUpto file={file} maxPx={Number.POSITIVE_INFINITY} layout="unified" scroll={scrollRef.current} lineHeight={lineHeight} />
      <div className="diff-body-pad" aria-hidden />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Blob-upgrade resolution (render_tool_group :5880-5959) lives in
// ../lib/tool-group-geometry.ts (ticket 70) — the estimator shares it.
// ---------------------------------------------------------------------------
