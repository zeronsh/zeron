import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { Icon } from "@zeron/icons";
import type { GitHistoryCommit, GitHistoryRef } from "@zeron/proto";
import {
  branchRefKey,
  historyAuthorInitial,
  historyAuthorName,
  laneX,
  refAreaWidth,
  refDescription,
  visibleRefCount,
  formatDate,
  HISTORY_NODE_RADIUS,
  HISTORY_ROW_HEIGHT,
  type GraphGeometry,
  type GraphRow,
  type HistoryColumn,
  type HistoryColumnWidths,
  type HistoryRowTransition,
} from "../../lib/git-history";
import { Tooltip, TOOLTIP_VIEW_OPTIONS_MS } from "../ui/Tooltip";

/**
 * One commit row (§2.9 / history.rs:3910 `render_row`): the graph cell
 * (node + optional fold control, pointer hit-testing), the subject + refs
 * cell (its own measured ref area), then the optional Author/Date/SHA cells
 * in persisted order. A click anywhere except the SHA pill and the fold
 * control opens the commit as its own pinned diff tab; hovering the row or
 * its graph sets the shared lane focus.
 */

const FOLD_TOOLTIP_MS = 250;
const AUTHOR_TOOLTIP_MS = 300;

export interface HistoryRowProps {
  readonly commit: GitHistoryCommit;
  readonly graphRow: GraphRow;
  readonly geometry: GraphGeometry;
  readonly palette: readonly string[];
  readonly hoveredColorId: number | null;
  readonly transition: HistoryRowTransition | null;
  readonly columns: readonly HistoryColumn[];
  readonly widths: HistoryColumnWidths;
  readonly authorDisplay: "avatar" | "name";
  readonly avatar: string | null;
  readonly copied: boolean;
  readonly collapsedBranches: ReadonlySet<string>;
  readonly collapsedCounts: ReadonlyMap<string, number>;
  readonly showFoldControl: boolean;
  readonly onOpenCommit: (commit: GitHistoryCommit) => void;
  readonly onCopySha: (sha: string) => void;
  readonly onHoverLane: (colorId: number | null) => void;
  readonly onGraphPointer: (row: GraphRow, x: number, y: number) => void;
  readonly onToggleFold: (refKey: string) => void;
}

export function HistoryRow(props: HistoryRowProps) {
  const {
    commit,
    graphRow,
    geometry,
    palette,
    hoveredColorId,
    transition,
    columns,
    widths,
    authorDisplay,
    avatar,
    copied,
    collapsedBranches,
    collapsedCounts,
    showFoldControl,
    onOpenCommit,
    onCopySha,
    onHoverLane,
    onGraphPointer,
    onToggleFold,
  } = props;

  const cellRef = useRef<HTMLDivElement | null>(null);
  const subjectRef = useRef<HTMLDivElement | null>(null);
  const [subjectWidth, setSubjectWidth] = useState(0);

  useLayoutEffect(() => {
    const el = subjectRef.current;
    if (el === null) {
      return;
    }
    const measure = (): void => {
      setSubjectWidth((current) => (current === el.clientWidth ? current : el.clientWidth));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => {
      observer.disconnect();
    };
  }, []);

  const rowFocused = hoveredColorId === graphRow.nodeColorId;
  const rowDimmed = hoveredColorId !== null && !rowFocused;
  const color = palette[graphRow.nodeColorId % palette.length] ?? palette[0]!;
  const subject = commit.subject.length === 0 ? "(no subject)" : commit.subject;
  const authorName = historyAuthorName(commit.authorName);

  return (
    <div
      className={[
        "history-row",
        rowFocused ? "history-row-focused" : "",
        rowDimmed ? "history-row-dim" : "",
        transition === "entering" ? "history-row-entering" : "",
        transition === "exiting" ? "history-row-exiting" : "",
      ]
        .filter((part) => part.length > 0)
        .join(" ")}
      style={{ height: HISTORY_ROW_HEIGHT }}
      role="button"
      tabIndex={0}
      aria-label={`${subject} by ${authorName}`}
      onClick={(event) => {
        if (event.defaultPrevented) {
          return;
        }
        onOpenCommit(commit);
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onOpenCommit(commit);
        }
      }}
      onPointerEnter={() => onHoverLane(graphRow.nodeColorId)}
      onPointerLeave={() => onHoverLane(null)}
    >
      <div
        ref={cellRef}
        className="history-graph-cell"
        style={{ width: geometry.width }}
        onPointerMove={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          onGraphPointer(graphRow, event.clientX - rect.left, event.clientY - rect.top);
        }}
        onPointerLeave={() => onGraphPointer(graphRow, -1, -1)}
      >
        {graphRow.isHead ? (
          <span className={`history-node-ring ${rowFocused ? "history-node-ring-focused" : ""}`} style={ringStyle(graphRow, geometry, color)} />
        ) : null}
        <span
          className={`history-node ${rowFocused ? "history-node-focused" : ""} ${rowDimmed ? "history-node-dim" : ""}`}
          style={nodeStyle(graphRow, geometry, color)}
        />
        {showFoldControl ? (
          <FoldControl
            commit={commit}
            graphRow={graphRow}
            geometry={geometry}
            color={color}
            collapsedBranches={collapsedBranches}
            collapsedCounts={collapsedCounts}
            onToggleFold={onToggleFold}
          />
        ) : null}
      </div>
      <div ref={subjectRef} className="history-subject">
        <span className="history-subject-text">{subject}</span>
        {commit.refs.length > 0 ? (
          <RefArea refs={commit.refs} availableWidth={refAreaWidth(subjectWidth)} />
        ) : null}
      </div>
      <div className="history-optional-cells">
        {columns.map((column) => {
          switch (column) {
            case "author":
              return (
                <AuthorCell
                  key={column}
                  width={widths.author}
                  display={authorDisplay}
                  name={authorName}
                  avatar={avatar}
                />
              );
            case "date":
              return <DateCell key={column} width={widths.date} authoredAt={commit.authoredAt} />;
            case "sha":
              return (
                <ShaCell
                  key={column}
                  width={widths.sha}
                  sha={commit.sha}
                  copied={copied}
                  onCopy={onCopySha}
                />
              );
          }
        })}
      </div>
    </div>
  );
}

function nodeStyle(row: GraphRow, geometry: GraphGeometry, color: string): CSSProperties {
  const radius = HISTORY_NODE_RADIUS;
  const x = laneX(geometry, row.nodeLane);
  return {
    left: x - radius,
    top: HISTORY_ROW_HEIGHT / 2 - radius,
    width: radius * 2,
    height: radius * 2,
    background: color,
  };
}

function ringStyle(row: GraphRow, geometry: GraphGeometry, color: string): CSSProperties {
  const radius = HISTORY_NODE_RADIUS + 2;
  const x = laneX(geometry, row.nodeLane);
  return {
    left: x - radius,
    top: HISTORY_ROW_HEIGHT / 2 - radius,
    width: radius * 2,
    height: radius * 2,
    borderColor: color,
  };
}

/**
 * The branch fold control (§2.8): 16×16, just right of the node, revealed
 * when the branch is collapsed OR the row's graph cell is hovered.
 */
function FoldControl({
  commit,
  graphRow,
  geometry,
  color,
  collapsedBranches,
  collapsedCounts,
  onToggleFold,
}: {
  commit: GitHistoryCommit;
  graphRow: GraphRow;
  geometry: GraphGeometry;
  color: string;
  collapsedBranches: ReadonlySet<string>;
  collapsedCounts: ReadonlyMap<string, number>;
  onToggleFold: (refKey: string) => void;
}) {
  const reference =
    commit.refs.find((entry) => branchRefKey(entry) !== null) ?? null;
  const key = reference !== null ? branchRefKey(reference) : null;
  if (key === null) {
    return null;
  }
  const collapsed = collapsedBranches.has(key);
  const hidden = collapsedCounts.get(key) ?? 0;
  const tooltip = collapsed
    ? `Expand ${reference!.label}${hidden > 0 ? ` (${hidden} hidden)` : ""}`
    : `Collapse ${reference!.label}`;
  return (
    <Tooltip label={tooltip} delay={FOLD_TOOLTIP_MS}
      trigger={
        <button
          type="button"
          className={`history-fold ${collapsed ? "history-fold-collapsed" : ""}`}
          style={{
            left: laneX(geometry, graphRow.nodeLane) + HISTORY_NODE_RADIUS + 3,
            borderColor: `color-mix(in srgb, ${color} 32%, transparent)`,
            color: `color-mix(in srgb, ${color} 90%, transparent)`,
          }}
          aria-label={tooltip}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onToggleFold(key);
          }}
        >
          <Icon name={collapsed ? "expandArrows" : "foldVertical"} size={9} />
        </button>
      }
    />
  );
}

/** `render_ref_area` (§3.11a) — the visible badge prefix + `+N` overflow. */
function RefArea({ refs, availableWidth }: { refs: readonly GitHistoryRef[]; availableWidth: number }) {
  const visible = visibleRefCount(refs, availableWidth);
  const hidden = refs.length - visible;
  const hiddenDescriptions = refs.slice(visible).map(refDescription);
  return (
    <span className="history-refs" style={{ maxWidth: availableWidth }}>
      {refs.slice(0, visible).map((reference, index) => (
        <RefBadge key={`${reference.kind}:${reference.label}:${index}`} reference={reference} />
      ))}
      {hidden > 0 ? (
        <Tooltip
          label={<RefTooltip descriptions={hiddenDescriptions} />}
          delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <span className="history-ref-overflow" aria-label={`${hidden} more refs`}>
              {`+${hidden}`}
            </span>
          }
        />
      ) : null}
    </span>
  );
}

/** `render_ref` (§3.11) — the branch/remote/tag pill. */
function RefBadge({ reference }: { reference: GitHistoryRef }) {
  const color = refBadgeColor(reference);
  const icon = reference.kind === "branch" ? "gitBranch" : reference.kind === "remote" ? "cloud" : "tag";
  return (
    <Tooltip label={refDescription(reference)} delay={TOOLTIP_VIEW_OPTIONS_MS}
      trigger={
        <span
          className="history-ref"
          style={{
            background: `color-mix(in srgb, ${color} 7%, transparent)`,
            color: `color-mix(in srgb, ${color} 90%, transparent)`,
          }}
        >
          <Icon name={icon} size={10} style={{ color: `color-mix(in srgb, ${color} 78%, transparent)` }} />
          <span className="history-ref-label">{reference.label}</span>
        </span>
      }
    />
  );
}

function refBadgeColor(reference: GitHistoryRef): string {
  switch (reference.kind) {
    case "branch":
      return "var(--rb-accent)";
    case "remote":
      return "var(--rb-activity)";
    case "tag":
      return "var(--rb-warning)";
  }
}

function RefTooltip({ descriptions }: { descriptions: readonly string[] }): ReactNode {
  return (
    <span className="history-ref-tooltip">
      {descriptions.map((description) => (
        <span key={description} className="mono">
          {description}
        </span>
      ))}
    </span>
  );
}

/** `render_author_cell` (§3.11) — avatar circle or the name. */
function AuthorCell({
  width,
  display,
  name,
  avatar,
}: {
  width: number;
  display: "avatar" | "name";
  name: string;
  avatar: string | null;
}) {
  if (display === "name") {
    return (
      <span className="history-author history-author-name" style={{ width }}>
        {name}
      </span>
    );
  }
  return (
    <span className="history-author" style={{ width }}>
      <Tooltip label={name} delay={AUTHOR_TOOLTIP_MS}
        trigger={
          <span className="history-avatar">
            {avatar !== null ? (
              <img className="history-avatar-image" src={avatar} alt="" draggable={false} />
            ) : (
              <span className="history-avatar-initial">{historyAuthorInitial(name)}</span>
            )}
          </span>
        }
      />
    </span>
  );
}

/** `render_date_cell` (§3.11). */
function DateCell({ width, authoredAt }: { width: number; authoredAt: string }) {
  return (
    <span className="history-date" style={{ width }}>
      {formatDate(authoredAt)}
    </span>
  );
}

/** `render_sha_cell` (§3.11) — the clickable 7-char pill / "Copied". */
function ShaCell({
  width,
  sha,
  copied,
  onCopy,
}: {
  width: number;
  sha: string;
  copied: boolean;
  onCopy: (sha: string) => void;
}) {
  return (
    <span className="history-sha" style={{ width }}>
      <button
        type="button"
        className={`history-sha-pill mono ${copied ? "history-sha-copied" : ""}`}
        aria-label={`Copy commit sha ${sha}`}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          void navigator.clipboard?.writeText(sha).catch(() => {});
          onCopy(sha);
        }}
      >
        {copied ? "Copied" : sha.slice(0, 7)}
      </button>
    </span>
  );
}
