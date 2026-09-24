import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { Icon } from "@zeron/icons";
import type { Appearance } from "@zeron/theme";
import { useUiSettings } from "../state/ui-settings";
import { DIFF_LINE_BASELINE, DIFF_TEXT_BASELINE, diffLineHeight, diffTextSize } from "../lib/typography";
import {
  ACCENT_BAR_WIDTH,
  BODY_BOTTOM_PAD,
  commentAdderLeft,
  DIFF_LINE_HEIGHT,
  estimateRowHeight,
  FILE_HEADER_HEIGHT,
  fileNotices,
  flattenFiles,
  FOLD_TWEEN_MAX_PX,
  gutterWidth,
  HUNK_HEADER_HEIGHT,
  horizontalGeometry,
  MARKER_WIDTH,
  NOTICE_HEIGHT,
  parseKey,
  parsePatch,
  splitAdderLeft,
  SPLIT_CODE_PADDING_LEFT,
  splitContentWidth,
  splitPairsUpto,
  UNIFIED_CODE_PADDING_LEFT,
  unifiedContentWidth,
  type DiffDraftAnchor,
  type DiffLine,
  type DiffMode,
  type DiffRow,
  type FileDiff,
  type FileFold,
  type Hunk,
  type LineKind,
} from "../lib/diff";
import type { ReviewComment } from "../lib/review-comments";
import { highlightCode, splitTokenLines } from "../lib/syntax";
import { CommentCard } from "./review-comments/comment-card";
import { CommentDraft } from "./review-comments/comment-draft";
import { FileIcon } from "./files/file-icon";

/**
 * The diff viewer — file headers + hunks + per-line rows, virtualized at
 * line granularity. The desktop's right-pane Changes tab is the reference
 * (`crates/ui/src/changes.rs`): rows are the desktop's `DiffRow` model
 * (`flattenFiles`), each +/− line carries a 3px accent bar at 0.55, dual
 * line-number gutters at the analytic `gutterWidth`, a 28px (unified) / 18px
 * (split) marker column, and a code plane that scrolls horizontally
 * INDEPENDENTLY of the row chrome, per file.
 *
 * Fold behavior is the desktop's too: a toggle swaps a file's body rows for
 * ONE height-animated stand-in (`foldingBody`) whose content is bounded to
 * what its clip can reveal (`FileBodyUpto`, capped at
 * `FOLD_TWEEN_MAX_PX`), and a settle sweep in the surface store swaps in the
 * steady rows once the 180ms COLLAPSE tween's window elapses.
 *
 * Row components (`FileHeaderRow`, `HunkHeaderRow`, `NoticeRow`, the unified
 * and split line rows) are prop-driven and free of the virtualizer and the
 * fold model, so ticket 19 can compose the same rows into the transcript's
 * stacked, unvirtualized tool-diff blocks.
 *
 * The viewer takes parsed files (the host parses once — see `useParsedDiff`
 * — so the surface store can measure fold heights analytically). It stays
 * free of wire I/O.
 */

const LINE_HEIGHT = DIFF_LINE_HEIGHT;
const SPLIT_MARKER_WIDTH = 18;

export type DiffLayout = DiffMode;

/** The hover anchor ticket 23's comment adder attaches to. */
export interface LineHoverInfo {
  readonly path: string;
  readonly side: "old" | "new";
  readonly lineNo: number;
}

export type LineHoverHandler = (info: LineHoverInfo | null) => void;

/**
 * Ticket 23's comment wiring: the staged set the row list interleaves
 * comment cards from (the file's own diff-sourced slice, the comment being
 * edited excluded by the caller), the open draft (with its live body), and
 * the card/draft action callbacks. `null`/undefined keeps the viewer
 * comment-free — the tool-diff mounts compose it that way.
 */
export interface DiffReviewWiring {
  /** The staged diff-sourced comments for this diff, in staged order. */
  readonly comments: readonly ReviewComment[];
  /** The open diff-side draft: its anchor, rename context, and live body. */
  readonly draft: (DiffDraftAnchor & { readonly body: string; readonly oldPath: string | null }) | null;
  readonly onDraftBody: (body: string) => void;
  readonly onDraftCancel: () => void;
  readonly onDraftCommit: () => void;
  readonly onCardEdit: (id: string) => void;
  readonly onCardRemove: (id: string) => void;
}

export interface DiffViewProps {
  /** The parsed files — `useParsedDiff` on the host side. */
  readonly files: readonly FileDiff[];
  /**
   * The resolved appearance — file headers pick their polychrome icons
   * from the `dark/` tree when dark (the desktop's `file_icons::icon`).
   */
  readonly appearance: Appearance;
  readonly layout?: DiffLayout;
  readonly wrap?: boolean;
  /** Per-file fold state, keyed by path (the surface store's snapshot). */
  readonly folds?: ReadonlyMap<string, FileFold>;
  readonly onToggleFold?: (path: string) => void;
  /**
   * Bumped by the surface store whenever the horizontal-extent inputs
   * change (scope / layout / wrap) — resets every file's code-plane offset.
   */
  readonly scrollEpoch?: number;
  /** Fires as the pointer enters/leaves a comment-adder anchor row. */
  readonly onLineHover?: LineHoverHandler;
  /**
   * Ticket 23's slot: renders the adder control inside the hovered row, at
   * the anchor the desktop computes (`commentAdderLeft`). Passing it also
   * arms the hover tracking that reveals the adder.
   */
  readonly renderAdder?: (info: LineHoverInfo) => ReactNode;
  /** Ticket 23's comment rows + actions. */
  readonly review?: DiffReviewWiring | null;
}

/**
 * Parse a fetched diff once per (checkout, checksum) — the memo the host
 * calls so both the viewer and the surface store's fold math share one
 * parse (the desktop's parse cache keyed on `parse_key`). `null` (no diff
 * resolved yet) parses to nothing, so callers need no conditional hook.
 */
export function useParsedDiff(
  diff: { readonly checkoutId: string; readonly checksum: string; readonly patch: string } | null,
): FileDiff[] {
  const cacheRef = useRef(new Map<string, FileDiff[]>());
  const cache = cacheRef.current;
  const key = diff === null ? "none" : parseKey(diff.checkoutId, diff.checksum, "workingTree", null);
  let files = cache.get(key);
  if (files === undefined) {
    files = diff === null ? [] : parsePatch(diff.patch);
    cache.set(key, files);
    if (cache.size > 8) {
      cache.delete(cache.keys().next().value as string);
    }
  }
  return files;
}

export function DiffView({
  files,
  appearance,
  layout = "unified",
  wrap = false,
  folds,
  onToggleFold,
  scrollEpoch,
  onLineHover,
  renderAdder,
  review,
}: DiffViewProps) {
  const scrollRef = useRef(new FilePlaneScroll());
  const scroll = scrollRef.current;

  useEffect(() => {
    scroll.reset();
    // `scrollEpoch` changes exactly when the extent inputs do.
  }, [scrollEpoch, scroll]);

  return (
    <DiffSurface
      files={files}
      appearance={appearance}
      layout={layout}
      wrap={wrap}
      folds={folds}
      onToggleFold={onToggleFold}
      scroll={scroll}
      onLineHover={onLineHover}
      renderAdder={renderAdder}
      review={review}
    />
  );
}

interface DiffSurfaceProps {
  readonly files: readonly FileDiff[];
  readonly appearance: Appearance;
  readonly layout: DiffLayout;
  readonly wrap: boolean;
  readonly folds?: ReadonlyMap<string, FileFold>;
  readonly onToggleFold?: (path: string) => void;
  readonly scroll: FilePlaneScroll;
  readonly onLineHover?: LineHoverHandler;
  readonly renderAdder?: (info: LineHoverInfo) => ReactNode;
  readonly review?: DiffReviewWiring | null;
}

function DiffSurface({ files, appearance, layout, wrap, folds, onToggleFold, scroll, onLineHover, renderAdder, review }: DiffSurfaceProps) {
  const emptyFolds = useRef(EMPTY_FOLDS).current;
  const foldMap = folds ?? emptyFolds;
  // The code font size drives the diff's text and row geometry together
  // (`diff_text_size` / `diff_line_height`, changes.rs): the rows paint it
  // through CSS vars and the virtualizer walks it.
  const codeFontSize = useUiSettings().codeFontSize;
  const lineHeight = diffLineHeight(codeFontSize);
  const textSize = diffTextSize(codeFontSize);
  const rows: DiffRow[] = useMemo(
    () => flattenFiles(files, layout, foldMap, review?.comments, review?.draft ?? null),
    [files, layout, foldMap, review?.comments, review?.draft],
  );

  // The hover anchor is only tracked while a consumer renders an adder;
  // `onLineHover` still fires for observers either way.
  const [hover, setHover] = useState<LineHoverInfo | null>(null);
  const handleHover: LineHoverHandler = (info) => {
    onLineHover?.(info);
    if (renderAdder !== undefined) {
      setHover((current) => (sameAnchor(current, info) ? current : info));
    }
  };

  return (
    <DiffScroller
      rows={rows}
      appearance={appearance}
      layout={layout}
      wrap={wrap}
      onToggleFold={onToggleFold}
      scroll={scroll}
      hover={renderAdder === undefined ? null : hover}
      onHover={handleHover}
      renderAdder={renderAdder}
      review={review}
      lineHeight={lineHeight}
      textSize={textSize}
    />
  );
}

const EMPTY_FOLDS: ReadonlyMap<string, FileFold> = new Map();

function sameAnchor(a: LineHoverInfo | null, b: LineHoverInfo | null): boolean {
  if (a === null || b === null) {
    return a === b;
  }
  return a.path === b.path && a.side === b.side && a.lineNo === b.lineNo;
}

/**
 * The per-file horizontal code-plane state — the desktop's
 * `FileHorizontalState`/`DiffCodeScroll` observable behavior: gutters,
 * markers, and the accent bar stay fixed while the code plane scrolls, every
 * row of one file shares one offset, and a reset (scope/layout/wrap change)
 * returns every file to origin.
 */
export class FilePlaneScroll {
  readonly #offsets = new Map<string, number>();
  readonly #nodes = new Map<string, Set<HTMLElement>>();

  offset(path: string): number {
    return this.#offsets.get(path) ?? 0;
  }

  register(path: string, el: HTMLElement): () => void {
    let set = this.#nodes.get(path);
    if (set === undefined) {
      set = new Set();
      this.#nodes.set(path, set);
    }
    set.add(el);
    const offset = this.offset(path);
    if (el.scrollLeft !== offset) {
      el.scrollLeft = offset;
    }
    return () => {
      set!.delete(el);
      if (set!.size === 0) {
        this.#nodes.delete(path);
      }
    };
  }

  onScroll(path: string, el: HTMLElement): void {
    const next = el.scrollLeft;
    if (next === this.#offsets.get(path)) {
      return;
    }
    this.#offsets.set(path, next);
    for (const node of this.#nodes.get(path) ?? []) {
      if (node !== el && node.scrollLeft !== next) {
        node.scrollLeft = next;
      }
    }
  }

  reset(): void {
    this.#offsets.clear();
    for (const nodes of this.#nodes.values()) {
      for (const node of nodes) {
        if (node.scrollLeft !== 0) {
          node.scrollLeft = 0;
        }
      }
    }
  }
}

interface ScrollerProps {
  readonly rows: readonly DiffRow[];
  readonly appearance: Appearance;
  readonly layout: DiffLayout;
  readonly wrap: boolean;
  readonly onToggleFold?: (path: string) => void;
  readonly scroll: FilePlaneScroll;
  readonly hover: LineHoverInfo | null;
  readonly onHover: LineHoverHandler;
  readonly renderAdder?: (info: LineHoverInfo) => ReactNode;
  readonly review?: DiffReviewWiring | null;
  /** The code-size-scaled diff row (`diff_line_height`); rows read it via CSS var. */
  readonly lineHeight: number;
  /** The code-size-scaled diff text size (`diff_text_size`). */
  readonly textSize: number;
}

function DiffScroller({ rows, appearance, layout, wrap, onToggleFold, scroll, hover, onHover, renderAdder, review, lineHeight, textSize }: ScrollerProps) {
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const heightsRef = useRef(new Map<string, number>());
  const positionsRef = useRef<readonly number[]>([]);
  const [view, setView] = useState({ top: 0, height: 0 });
  const [, bumpMeasure] = useState(0);

  const heights = heightsRef.current;
  if (heights.size > rows.length + 256) {
    const live = new Set(rows.map((row) => row.id));
    for (const id of heights.keys()) {
      if (!live.has(id)) {
        heights.delete(id);
      }
    }
  }

  // A code-font change re-sizes every line row through the CSS var: cached
  // measurements were taken against the old size, so drop them and let the
  // (exact) estimate stand in until rows re-measure on remount.
  useEffect(() => {
    heightsRef.current.clear();
    bumpMeasure((n) => n + 1);
  }, [lineHeight]);

  const positions: number[] = new Array(rows.length + 1);
  positions[0] = 0;
  for (let ix = 0; ix < rows.length; ix += 1) {
    const row = rows[ix]!;
    const measured = heights.get(row.id);
    const height = measured !== undefined ? measured : estimateRowHeight(row, lineHeight);
    positions[ix + 1] = positions[ix]! + height;
  }
  positionsRef.current = positions;

  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller === null) {
      return;
    }
    const update = (): void => {
      setView({ top: scroller.scrollTop, height: scroller.clientHeight });
    };
    update();
    const onScroll = (): void => update();
    scroller.addEventListener("scroll", onScroll, { passive: true });
    const observer = new ResizeObserver(() => update());
    observer.observe(scroller);
    return () => {
      scroller.removeEventListener("scroll", onScroll);
      observer.disconnect();
    };
  }, []);

  useEffect(() => {
    const total = positions[positions.length - 1] ?? 0;
    const scroller = scrollerRef.current;
    if (scroller === null) {
      return;
    }
    const overflow = total - scroller.clientHeight;
    if (overflow > 0 && scroller.scrollTop > overflow) {
      scroller.scrollTop = overflow;
    }
  }, [positions]);

  if (rows.length === 0) {
    return null;
  }

  const first = findRowAt(rows, positions, view.top);
  const last = findRowAt(rows, positions, view.top + view.height);
  const total = positions[positions.length - 1] ?? 0;
  const padTop = first > 0 ? positions[first]! : 0;
  const padBottom = last + 1 < rows.length ? (positions[positions.length - 1] ?? 0) - positions[last + 1]! : 0;

  return (
    <div
      className={`diff-view ${layout === "split" ? "diff-view-split" : "diff-view-unified"} ${wrap ? "diff-view-wrap" : ""}`}
      ref={scrollerRef}
      style={{
        ["--rb-diff-line-height" as string]: `${lineHeight}px`,
        ["--rb-diff-text-size" as string]: `${textSize}px`,
      }}
    >
      <div className="diff-spacer" style={{ height: total }} aria-hidden>
        <div className="diff-window" style={{ transform: `translateY(${padTop}px)` }}>
          {rows.slice(first, last + 1).map((row) => {
            const id = row.id;
            const measured = heights.get(id);
            const height = measured !== undefined ? measured : estimateRowHeight(row, lineHeight);
            return (
              <div
                key={id}
                data-row-id={id}
                ref={(el) => {
                  if (el === null) {
                    return;
                  }
                  if (row.kind === "foldingBody") {
                    // The tween animates its height after mount; observe it
                    // so positions track the animation, not just the seed.
                    const observer = new ResizeObserver(() => {
                      const next = el.getBoundingClientRect().height;
                      if (Math.abs((heights.get(id) ?? -1) - next) > 0.5) {
                        heights.set(id, next);
                        bumpMeasure((n) => n + 1);
                      }
                    });
                    observer.observe(el);
                    heights.set(id, el.getBoundingClientRect().height);
                    return () => {
                      observer.disconnect();
                      heights.delete(id);
                    };
                  }
                  const next = el.getBoundingClientRect().height;
                  if (Math.abs((heights.get(id) ?? -1) - next) > 0.5) {
                    heights.set(id, next);
                    bumpMeasure((n) => n + 1);
                  }
                }}
                style={{ minHeight: height }}
              >
                <RowContent
                  row={row}
                  appearance={appearance}
                  layout={layout}
                  wrap={wrap}
                  onToggleFold={onToggleFold}
                  scroll={scroll}
                  hover={hover}
                  onHover={onHover}
                  renderAdder={renderAdder}
                  review={review}
                  lineHeight={lineHeight}
                />
              </div>
            );
          })}
        </div>
        <div style={{ height: padBottom }} aria-hidden />
      </div>
    </div>
  );
}

function findRowAt(rows: readonly DiffRow[], positions: readonly number[], offset: number): number {
  let lo = 0;
  let hi = rows.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >>> 1;
    if ((positions[mid] ?? 0) <= offset) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  return lo;
}

interface RowContentProps {
  readonly row: DiffRow;
  readonly appearance: Appearance;
  readonly layout: DiffLayout;
  readonly wrap: boolean;
  readonly onToggleFold?: (path: string) => void;
  readonly scroll: FilePlaneScroll;
  readonly hover: LineHoverInfo | null;
  readonly onHover: LineHoverHandler;
  readonly renderAdder?: (info: LineHoverInfo) => ReactNode;
  readonly review?: DiffReviewWiring | null;
  readonly lineHeight: number;
}

function RowContent({ row, appearance, layout, wrap, onToggleFold, scroll, hover, onHover, renderAdder, review, lineHeight }: RowContentProps) {
  switch (row.kind) {
    case "fileHeader":
      return (
        <FileHeaderRow
          file={row.file}
          appearance={appearance}
          expanded={row.expanded}
          animating={row.animating}
          onToggle={() => onToggleFold?.(row.file.path)}
        />
      );
    case "hunkHeader":
      return <HunkHeaderRow file={row.file} hunkIx={row.hunkIx} />;
    case "notice":
      return <NoticeRow file={row.file} messageIx={row.messageIx} />;
    case "commentCard":
      // A comment row without the wiring never lands in the list (the
      // flattener only interleaves when comments are passed), but the
      // guard keeps the viewer independently mountable.
      return review === null || review === undefined ? null : (
        <CommentCard
          comment={row.comment}
          onEdit={review.onCardEdit}
          onRemove={review.onCardRemove}
        />
      );
    case "commentDraft":
      return review === null || review === undefined || review.draft === null ? null : (
        <CommentDraft
          // `draft_cite_path` (changes.rs:3291-3294): the header cites the
          // same path the staged card and the prompt bullet will — the
          // pre-rename path on the Old side. (The ticket's §2.3 says RAW
          // path; the desktop source wins — noted in the ticket Comments.)
          path={row.side === "old" && review.draft.oldPath !== null ? review.draft.oldPath : row.path}
          line={row.line}
          body={review.draft.body}
          editing={row.editingId !== null}
          onBody={review.onDraftBody}
          onCancel={review.onDraftCancel}
          onCommit={review.onDraftCommit}
        />
      );
    case "line":
      return (
        <LineRow
          file={row.file}
          hunkIx={row.hunkIx}
          lineIx={row.lineIx}
          layout={layout}
          wrap={wrap}
          scroll={scroll}
          hover={hover}
          onHover={onHover}
          renderAdder={renderAdder}
        />
      );
    case "splitLine":
      return (
        <SplitPairRow
          file={row.file}
          hunkIx={row.hunkIx}
          left={row.left}
          right={row.right}
          wrap={wrap}
          scroll={scroll}
          hover={hover}
          onHover={onHover}
          renderAdder={renderAdder}
        />
      );
    case "bodyPad":
      return <div className="diff-body-pad" aria-hidden />;
    case "foldingBody":
      return <FoldingBodyRow row={row} layout={layout} scroll={scroll} lineHeight={lineHeight} />;
  }
}

// ---------------------------------------------------------------------------
// Row components — prop-driven, free of the virtualizer and fold model so
// ticket 19 can compose them into the transcript's stacked tool diffs.
// ---------------------------------------------------------------------------

/**
 * The file header: chevron, file-type icon, path, `BIN`, `+N`/`−N`. No
 * status word and no index chip — the desktop has neither; a file's
 * added/deleted/renamed state is carried by the notice row alone
 * (`render_file_header`, changes.rs:3348-3482).
 */
function FileHeaderRow({ file, appearance, expanded, animating, onToggle }: { file: FileDiff; appearance: Appearance; expanded: boolean; animating: boolean; onToggle: () => void }) {
  return (
    <div className={`diff-file-header ${expanded ? "diff-file-expanded" : "diff-file-collapsed"}`}>
      <button type="button" className="diff-file-button" onClick={onToggle} aria-expanded={expanded}>
        <span className={`diff-chevron ${animating ? "diff-chevron-anim" : ""}`}>
          <Icon name={expanded ? "altArrowDown" : "altArrowRight"} size={13} />
        </span>
        <FileIcon kind="file" name={file.path} appearance={appearance} size={14} className="diff-file-icon" />
        <span className="diff-file-path mono">
          {file.oldPath !== null ? <span className="diff-file-rename">{file.oldPath} → </span> : null}
          {file.path}
        </span>
        {file.binary ? <span className="diff-file-bin">BIN</span> : null}
        {file.additions > 0 || !file.binary ? <span className="diff-file-add mono">+{file.additions}</span> : null}
        {file.deletions > 0 || !file.binary ? <span className="diff-file-del mono">−{file.deletions}</span> : null}
      </button>
    </div>
  );
}

function HunkHeaderRow({ file, hunkIx }: { file: FileDiff; hunkIx: number }) {
  const hunk = file.hunks[hunkIx] as Hunk | undefined;
  if (hunk === undefined) {
    return null;
  }
  return (
    <div className="diff-hunk-header mono" aria-hidden>
      {hunk.header}
    </div>
  );
}

function NoticeRow({ file, messageIx }: { file: FileDiff; messageIx: number }) {
  const message = fileNotices(file)[messageIx];
  if (message === undefined) {
    return null;
  }
  return (
    <div className="diff-notice">
      {message}
    </div>
  );
}

function LineRow({
  file,
  hunkIx,
  lineIx,
  layout,
  wrap,
  scroll,
  hover,
  onHover,
  renderAdder,
}: {
  file: FileDiff;
  hunkIx: number;
  lineIx: number;
  layout: DiffLayout;
  wrap: boolean;
  scroll: FilePlaneScroll;
  hover: LineHoverInfo | null;
  onHover: LineHoverHandler;
  renderAdder?: (info: LineHoverInfo) => ReactNode;
}) {
  const hunk = file.hunks[hunkIx];
  const line = hunk?.lines[lineIx];
  if (hunk === undefined || line === undefined) {
    return null;
  }
  if (layout === "split") {
    return null;
  }
  return (
    <UnifiedLineRow
      file={file}
      line={line}
      wrap={wrap}
      scroll={scroll}
      hover={hover}
      onHover={onHover}
      renderAdder={renderAdder}
    />
  );
}

const EMPTY_TOKENS: { lines: { text: string; role: string | null }[][] } = { lines: [] };

/** The anchor a row can host the comment adder on (ticket 23's hook). */
function lineAnchor(line: DiffLine, path: string): LineHoverInfo | null {
  if (line.newNo !== null) {
    return { path, side: "new", lineNo: line.newNo };
  }
  if (line.oldNo !== null) {
    return { path, side: "old", lineNo: line.oldNo };
  }
  return null;
}

/**
 * One unified +/−/context/meta line: accent bar, dual gutters, marker
 * column, code plane (`diff_line_row`, changes.rs:4233-4368).
 */
function UnifiedLineRow({
  file,
  line,
  wrap,
  scroll,
  hover,
  onHover,
  renderAdder,
}: {
  file: FileDiff;
  line: DiffLine;
  wrap: boolean;
  scroll: FilePlaneScroll;
  hover: LineHoverInfo | null;
  onHover: LineHoverHandler;
  renderAdder?: (info: LineHoverInfo) => ReactNode;
}) {
  const gutter = gutterWidth(file);
  const tokens = useMemo(() => tokenize(file.path, line.text), [file.path, line]);
  if (line.kind === "meta") {
    // `\ No newline at end of file` — a note, not code: indented past all
    // four columns, never tinted, italic 10.5.
    return (
      <div className="diff-line diff-line-meta mono" style={{ paddingLeft: ACCENT_BAR_WIDTH + 2 * gutter + MARKER_WIDTH + UNIFIED_CODE_PADDING_LEFT }}>
        {line.text}
      </div>
    );
  }
  const anchor = lineAnchor(line, file.path);
  // Every non-meta line is a comment-adder anchor: the hover state marks
  // it (ticket 23 renders the button; the callback is the seam).
  const canAnchor = anchor !== null;
  return (
    <div
      className={`diff-line diff-line-${line.kind} ${canAnchor ? "diff-line-can-add" : ""}`}
      onMouseEnter={anchor === null ? undefined : () => onHover(anchor)}
      onMouseLeave={anchor === null ? undefined : () => onHover(null)}
    >
      <span className={`diff-accent diff-accent-${line.kind}`} aria-hidden />
      <span
        className={`diff-line-old mono ${line.kind === "del" ? "diff-line-num-own-del" : ""}`}
        style={{ width: gutter }}
      >
        {formatLineNo(line.oldNo)}
      </span>
      <span
        className={`diff-line-new mono ${line.kind === "add" ? "diff-line-num-own-add" : ""}`}
        style={{ width: gutter }}
      >
        {formatLineNo(line.newNo)}
      </span>
      <span className={`diff-line-marker diff-line-marker-${line.kind}`}>{markerFor(line.kind)}</span>
      <CodePlane file={file} side="unified" wrap={wrap} scroll={scroll}>
        <LineText tokens={tokens} />
      </CodePlane>
      {anchor !== null && renderAdder !== undefined && hover !== null && sameAnchor(hover, anchor) ? (
        <span className="diff-adder-slot" style={{ left: commentAdderLeft(anchor.side, gutter) }}>
          {renderAdder!(anchor)}
        </span>
      ) : null}
    </div>
  );
}

/**
 * One split row: two halves of accent bar / gutter / marker / code plus the
 * centre hairline; a one-sided row's empty half is the flat filler wash
 * (`split_row`/`split_line_cell`, changes.rs:4408-4544). Meta markers span
 * both halves.
 */
function SplitPairRow({
  file,
  hunkIx,
  left,
  right,
  wrap,
  scroll,
  hover,
  onHover,
  renderAdder,
}: {
  file: FileDiff;
  hunkIx: number;
  left: number | null;
  right: number | null;
  wrap: boolean;
  scroll: FilePlaneScroll;
  hover: LineHoverInfo | null;
  onHover: LineHoverHandler;
  renderAdder?: (info: LineHoverInfo) => ReactNode;
}) {
  const hunk = file.hunks[hunkIx] as Hunk | undefined;
  if (hunk === undefined) {
    return null;
  }
  const leftLine = left !== null ? hunk.lines[left] ?? null : null;
  const rightLine = right !== null ? hunk.lines[right] ?? null : null;
  const markerLine =
    leftLine !== null && leftLine.kind === "meta"
      ? leftLine
      : rightLine !== null && rightLine.kind === "meta"
        ? rightLine
        : null;
  if (markerLine !== null) {
    return (
      <div className="diff-line diff-line-meta mono" style={{ paddingLeft: 2 * (ACCENT_BAR_WIDTH + gutterWidth(file)) }}>
        {markerLine.text}
      </div>
    );
  }
  return (
    <div className="diff-line diff-line-split">
      <SplitHalf file={file} line={leftLine} old wrap={wrap} scroll={scroll} hover={null} onHover={() => {}} />
      <span className="diff-split-divider" aria-hidden />
      <SplitHalf
        file={file}
        line={rightLine}
        old={false}
        wrap={wrap}
        scroll={scroll}
        hover={hover}
        onHover={onHover}
        renderAdder={renderAdder}
      />
    </div>
  );
}

function SplitHalf({
  file,
  line,
  old,
  wrap,
  scroll,
  hover,
  onHover,
  renderAdder,
}: {
  file: FileDiff;
  line: DiffLine | null;
  old: boolean;
  wrap: boolean;
  scroll: FilePlaneScroll;
  hover: LineHoverInfo | null;
  onHover: LineHoverHandler;
  renderAdder?: (info: LineHoverInfo) => ReactNode;
}) {
  const gutter = gutterWidth(file);
  const tokens = useMemo(
    () => (line !== null ? tokenize(file.path, line.text) : EMPTY_TOKENS),
    [file.path, line],
  );
  if (line === null) {
    // The empty half of a one-sided row — a flat wash quieter than either
    // tint, reading as "nothing here" without competing with the code.
    return <span className="diff-split-side diff-split-filler" aria-hidden />;
  }
  // Only the RIGHT column ever offers a `+` (a deletion cannot be edited);
  // the left column is inert, but its row still shows staged cards
  // (ticket 23) — this is why the halves never collapse.
  const anchor = !old && line.newNo !== null ? ({ path: file.path, side: "new", lineNo: line.newNo } as const) : null;
  // The right half's hover state marks the adder anchor even before ticket
  // 23 renders the button — the bare hook the ticket's screenshot (b) shows.
  const canAnchor = anchor !== null;
  return (
    <span
      className={`diff-split-side diff-split-${old ? "old" : "new"} diff-split-${line.kind} ${canAnchor ? "diff-line-can-add" : ""}`}
      onMouseEnter={anchor === null ? undefined : () => onHover(anchor)}
      onMouseLeave={anchor === null ? undefined : () => onHover(null)}
    >
      <span className={`diff-accent diff-accent-${line.kind}`} aria-hidden />
      <span
        className={`diff-line-old mono ${line.kind === "del" ? "diff-line-num-own-del" : line.kind === "add" ? "diff-line-num-own-add" : ""}`}
        style={{ width: gutter }}
      >
        {formatLineNo(old ? line.oldNo : line.newNo)}
      </span>
      <span className={`diff-split-marker diff-line-marker-${line.kind}`}>{markerFor(line.kind)}</span>
      <CodePlane file={file} side={old ? "split-old" : "split-new"} wrap={wrap} scroll={scroll}>
        <LineText tokens={tokens} />
      </CodePlane>
      {anchor !== null && renderAdder !== undefined && hover !== null && sameAnchor(hover, anchor) ? (
        <span className="diff-adder-slot" style={{ left: splitAdderLeft(gutter) }}>
          {renderAdder!(anchor)}
        </span>
      ) : null}
    </span>
  );
}

/**
 * The code plane — the only horizontally scrollable part of a row
 * (`code_text_viewport`, changes.rs:4177-4228). The content's intrinsic
 * width is the analytic estimate (columns × mono advance + paddings + the
 * gutter compensation that keeps every file's scroll extent identical), so
 * a tab-containing line widens the plane instead of clipping. When wrap is
 * on there is no intrinsic width and no scroll — the plane grows down.
 */
function CodePlane({
  file,
  side,
  wrap,
  scroll,
  children,
}: {
  file: FileDiff;
  side: "unified" | "split-old" | "split-new";
  wrap: boolean;
  scroll: FilePlaneScroll;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const geometry = useMemo(() => horizontalGeometry(file), [file]);
  const gutter = gutterWidth(file);
  const paddingSum = side === "unified" ? UNIFIED_CODE_PADDING_LEFT : SPLIT_CODE_PADDING_LEFT;

  useLayoutEffect(() => {
    const el = ref.current;
    if (el === null || wrap) {
      return;
    }
    return scroll.register(file.path, el);
  }, [scroll, file.path, wrap]);

  if (wrap) {
    return (
      <span className="diff-code-viewport diff-code-viewport-wrap">
        <span className="diff-code-content diff-code-content-wrap" style={{ paddingLeft: paddingSum }}>
          {children}
        </span>
      </span>
    );
  }
  const columns = geometry.maxCodeColumns;
  // The analytic extent with a zero text width is exactly the paddings plus
  // the gutter compensation; the text width rides in as `ch` units (the
  // mono advance — the web's `ch_advance`).
  const extentPx =
    side === "unified"
      ? unifiedContentWidth(0, geometry.maxGutterWidth, gutter)
      : splitContentWidth(0, geometry.maxGutterWidth, gutter);
  const style: CSSProperties = {
    minWidth: `calc(${columns}ch + ${extentPx}px)`,
    paddingLeft: paddingSum,
  };
  return (
    <span
      className="diff-code-viewport"
      ref={ref}
      onScroll={(event) => scroll.onScroll(file.path, event.currentTarget)}
    >
      <span className="diff-code-content" style={style}>
        {children}
      </span>
    </span>
  );
}

/**
 * The fold tween's stand-in: ONE clipped, height-animated row standing in
 * for the whole body. Only the slice the clip can reveal is built
 * (`FileBodyUpto`, capped at `FOLD_TWEEN_MAX_PX`), so a 50k-line file's
 * fold never builds more than ~2400px of content mid-animation.
 */
function FoldingBodyRow({ row, layout, scroll, lineHeight }: { row: Extract<DiffRow, { kind: "foldingBody" }>; layout: DiffLayout; scroll: FilePlaneScroll; lineHeight: number }) {
  const { file, from, to, epoch } = row;
  const ref = useRef<HTMLDivElement | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (el === null) {
      return;
    }
    el.style.height = `${from}px`;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      // The store already wrote steady rows; if motion flipped mid-flight,
      // land on the endpoint rather than tween.
      el.style.height = `${to}px`;
      return;
    }
    const raf = requestAnimationFrame(() => {
      el.style.height = `${to}px`;
    });
    return () => cancelAnimationFrame(raf);
  }, [epoch, from, to]);
  const capped = Math.min(Math.max(from, to), FOLD_TWEEN_MAX_PX);
  return (
    <div ref={ref} className="diff-folding" style={{ height: from }}>
      <FileBodyUpto file={file} maxPx={capped} layout={layout} scroll={scroll} lineHeight={lineHeight} />
    </div>
  );
}

/**
 * Build only rows that start above `maxPx` (`render_file_body_upto`,
 * changes.rs:4643-4757) — the body behind the fold tween's clip. Analytic
 * heights, no measurement, no virtualization: the walk stops the instant
 * the running height reaches the budget, and the split arm pairs only what
 * the clip can still reveal.
 *
 * Exported for ticket 19: an inline tool diff mounts this as its whole body
 * (the desktop's `render_file_body_with_syntax` call in
 * `transcript.rs::detail_body`), with `maxPx = Infinity` for the full
 * stack. The trailing `BODY_BOTTOM_PAD` is the caller's (`bodyHeight`
 * counts it; `bodyPad` renders it).
 */
export function FileBodyUpto({ file, maxPx, layout, scroll, lineHeight = DIFF_LINE_HEIGHT }: { file: FileDiff; maxPx: number; layout: DiffLayout; scroll: FilePlaneScroll; lineHeight?: number }) {
  const rows: ReactNode[] = [];
  const notices = fileNotices(file);
  let y = 0;
  build: {
    for (let ix = 0; ix < notices.length; ix += 1) {
      if (y >= maxPx) {
        break build;
      }
      rows.push(<NoticeRow key={`n${ix}`} file={file} messageIx={ix} />);
      y += NOTICE_HEIGHT;
    }
    for (let hunkIx = 0; hunkIx < file.hunks.length; hunkIx += 1) {
      if (y >= maxPx) {
        break build;
      }
      rows.push(<HunkHeaderRow key={`k${hunkIx}`} file={file} hunkIx={hunkIx} />);
      y += HUNK_HEADER_HEIGHT;
      const hunk = file.hunks[hunkIx]!;
      if (layout === "split") {
        const budget = Math.max(0, Math.ceil((maxPx - y) / lineHeight));
        const pairs = splitPairsUpto(hunk.lines, budget);
        for (let pairIx = 0; pairIx < pairs.length; pairIx += 1) {
          if (y >= maxPx) {
            break build;
          }
          rows.push(
            <SplitPairRow
              key={`s${hunkIx}.${pairIx}`}
              file={file}
              hunkIx={hunkIx}
              left={pairs[pairIx]![0]}
              right={pairs[pairIx]![1]}
              wrap={false}
              scroll={scroll}
              hover={null}
              onHover={() => {}}
            />,
          );
          y += lineHeight;
        }
      } else {
        for (let lineIx = 0; lineIx < hunk.lines.length; lineIx += 1) {
          if (y >= maxPx) {
            break build;
          }
          rows.push(
            <UnifiedLineRow
              key={`l${hunkIx}.${lineIx}`}
              file={file}
              line={hunk.lines[lineIx]!}
              wrap={false}
              scroll={scroll}
              hover={null}
              onHover={() => {}}
            />,
          );
          y += lineHeight;
        }
      }
    }
  }
  // The code-size-scaled row/text reach the rows through CSS vars (the
  // analytic `y` walk above uses the same value).
  return (
    <div
      className="diff-body-scaled"
      style={{
        ["--rb-diff-line-height" as string]: `${lineHeight}px`,
        ["--rb-diff-text-size" as string]: `${(lineHeight / DIFF_LINE_BASELINE) * DIFF_TEXT_BASELINE}px`,
        display: "contents",
      }}
    >
      {rows}
    </div>
  );
}

function LineText({ tokens }: { tokens: { lines: { text: string; role: string | null }[][] } }) {
  const lines = tokens.lines;
  return (
    <>
      {lines.map((tokenLine, ix) => (
        <span key={ix} className="diff-line-row">
          {tokenLine.map((token, tokenIx) => (
            <span key={tokenIx} className={token.role !== null ? `tk-${token.role}` : undefined}>
              {token.text}
            </span>
          ))}
          {ix === lines.length - 1 ? null : "\n"}
        </span>
      ))}
    </>
  );
}

function markerFor(kind: LineKind): string {
  switch (kind) {
    case "add":
      return "+";
    case "del":
      return "−";
    case "context":
      return "·";
    case "meta":
      return "\\";
  }
}

function formatLineNo(no: number | null): string {
  return no === null ? "" : String(no);
}

function tokenize(path: string, text: string): { lines: { text: string; role: string | null }[][] } {
  const language = languageFor(path);
  const tokens = highlightCode(text, language);
  return { lines: splitTokenLines(tokens) };
}

/** Map a file path to the syntax language id used by `lib/syntax.ts`. */
export function languageFor(path: string): string | null {
  const slash = path.lastIndexOf("/");
  const base = path.slice(slash + 1);
  const dot = base.lastIndexOf(".");
  if (dot < 0 || dot === base.length - 1) {
    return null;
  }
  return base.slice(dot + 1).toLowerCase();
}

export const DIFF_METRICS = {
  FILE_HEADER_HEIGHT,
  HUNK_HEADER_HEIGHT,
  LINE_HEIGHT,
  NOTICE_HEIGHT,
  BODY_BOTTOM_PAD,
  SPLIT_MARKER_WIDTH,
  MARKER_WIDTH,
  ACCENT_BAR_WIDTH,
  SPLIT_CODE_PADDING_LEFT,
  UNIFIED_CODE_PADDING_LEFT,
  TEXT_SIZE: 12,
} as const;
