import {
  createContext,
  memo,
  useContext,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { Icon } from "@zeron/icons";
import type { Block, BlockTree, InlineRun, TableAlign } from "../lib/markdown";
import { PENDING_LINK_URL, tableColumns } from "../lib/markdown";
import { graphemeBreaks, resolveWorkspaceFileLink, transcriptAddress } from "../lib/links";
import { hasSpecificFileIcon, wellBg } from "../lib/file-icons";
import { highlightCode, splitTokenLines, type SyntaxRole, type SyntaxToken } from "../lib/syntax";
import { sliceTokensForVeil } from "../lib/veil";
import { uiSettings, useUiSettings } from "../state/ui-settings";
import { codeBlockLineHeight, codeBlockTextSize } from "../lib/typography";
import { useResolvedAppearance } from "../state/appearance";
import { FileIcon } from "./files/file-icon";
import {
  RbContextMenu,
  RbContextMenuPopup,
  RbContextMenuPortal,
  RbContextMenuPositioner,
  RbContextMenuTrigger,
} from "./base/menu";
import { RbTooltip, RbTooltipTrigger } from "./base/tooltip";
import { MenuRow } from "./ui/MenuRows";
import { Tooltip } from "./ui/Tooltip";

/**
 * Renders the parsed markdown model (`../lib/markdown.ts`) — one React
 * element tree per top-level block, so transcript rows re-render only the
 * block whose bytes changed. Colors come from `--rb-*` custom properties
 * (`md-*`/`tk-*` classes in app.css); no color is hardcoded here.
 *
 * The interactive layer is the web port of the desktop markdown stack's host
 * hooks (`crates/ui/src/markdown/render.rs`): the code header with its
 * verbatim fence-info label, copy and global fit toggle
 * (`code_block_header` :1928-1966, `code_copy_button` :1881-1926,
 * `render_code_block_source_with_actions` :2015-2254), link validation +
 * the hover destination card + the right-click menu + the drag/selection
 * click guard (`browser/model.rs:37-106`, `link_destination.rs`,
 * `link_interaction.rs`), images as real media elements
 * (`text_element` :1593-1637), workspace-file links
 * (`workspace_links.rs`, `render.rs:1711-1793`), the desktop's own list
 * markers and task checkbox (:552-690), and the veil slicing for code
 * (`veil.rs` + `render.rs:2065-2070, 2141-2146`).
 */

// ---------------------------------------------------------------------------
// Surface context — the host hooks the desktop threads through RenderOptions
// ---------------------------------------------------------------------------

/**
 * What the transcript host provides to markdown blocks: the chat's workspace
 * root (`opts.workspace_root`, transcript.rs:5341-5349) and the internal
 * link action — opening a workspace file in the right pane (the desktop's
 * `LinkOutcome::Internal`). Null/absent (the subagent dialog) leaves
 * workspace links unresolved and inert.
 */
export interface MarkdownSurface {
  readonly workspaceRoot: string | null;
  readonly openWorkspaceFile: (path: string) => void;
}

const MarkdownSurfaceContext = createContext<MarkdownSurface | null>(null);

const INERT_SURFACE: MarkdownSurface = { workspaceRoot: null, openWorkspaceFile: () => {} };

export function MarkdownSurfaceProvider({
  value,
  children,
}: {
  value: MarkdownSurface;
  children: ReactNode;
}) {
  return <MarkdownSurfaceContext.Provider value={value}>{children}</MarkdownSurfaceContext.Provider>;
}

function useMarkdownSurface(): MarkdownSurface {
  return useContext(MarkdownSurfaceContext) ?? INERT_SURFACE;
}

/** One fading veil chunk over the block's flat text (see `../lib/veil.ts`). */
export interface VeilChunk {
  readonly key: string;
  readonly start: number;
  readonly end: number;
  readonly durationMs: number;
}

// ---------------------------------------------------------------------------
// Block dispatch
// ---------------------------------------------------------------------------

export const MarkdownBlockView = memo(function MarkdownBlockView({ block }: { block: Block }) {
  return <>{renderBlock(block)}</>;
});

/** A whole tree (assistant text part, thought detail) block by block. */
export function MarkdownTreeView({ tree }: { tree: BlockTree }) {
  return (
    <>
      {tree.blocks.map((top, ix) => (
        <MarkdownBlockView key={ix} block={top.block} />
      ))}
    </>
  );
}

function renderBlock(block: Block): ReactNode {
  switch (block.kind) {
    case "paragraph":
      return (
        <p className="md-p">
          <FileRefGuard runs={block.runs}>
            <RunsOrMedia runs={block.runs} />
          </FileRefGuard>
        </p>
      );
    case "heading": {
      const content = <RunsOrMedia runs={block.runs} />;
      switch (Math.min(6, Math.max(1, block.level))) {
        case 1:
          return <h1 className="md-h md-h1">{content}</h1>;
        case 2:
          return <h2 className="md-h md-h2">{content}</h2>;
        case 3:
          return <h3 className="md-h md-h3">{content}</h3>;
        case 4:
          return <h4 className="md-h md-h4">{content}</h4>;
        case 5:
          return <h5 className="md-h md-h5">{content}</h5>;
        default:
          return <h6 className="md-h md-h6">{content}</h6>;
      }
    }
    case "codeBlock":
      return <CodeBlock code={block.code} language={block.language} />;
    case "blockQuote":
      return (
        <blockquote className="md-quote">
          {block.children.map((child, ix) => (
            <MarkdownBlockView key={ix} block={child} />
          ))}
        </blockquote>
      );
    case "list": {
      const items = block.items.map((item, ix) => (
        <li key={ix} className="md-item">
          <span className="md-marker" aria-hidden={item.checked === null}>
            {item.checked !== null ? (
              <TaskCheckbox checked={item.checked} />
            ) : block.orderedStart !== null ? (
              `${block.orderedStart + ix}.`
            ) : (
              <span className="md-marker-dot" />
            )}
          </span>
          <div className="md-item-body">
            {item.blocks.map((child, childIx) => (
              <MarkdownBlockView key={childIx} block={child} />
            ))}
          </div>
        </li>
      ));
      return block.orderedStart !== null ? (
        <ol className="md-list" start={block.orderedStart}>
          {items}
        </ol>
      ) : (
        <ul className="md-list">{items}</ul>
      );
    }
    case "table":
      return <MarkdownTable block={block} />;
    case "rule":
      return <hr className="md-rule" />;
  }
}

function alignCss(align: TableAlign | undefined): "left" | "center" | "right" {
  return align ?? "left";
}

// ---------------------------------------------------------------------------
// Inline rendering — the ONE renderer (transcript.tsx's veiled rows and
// thought details share it; the old StyledRun copy is gone)
// ---------------------------------------------------------------------------

function InlineRuns({ runs }: { runs: readonly InlineRun[] }) {
  return (
    <>
      {runs.map((run, ix) => (
        <InlineRunView key={ix} run={run} />
      ))}
    </>
  );
}

/** One styled inline run — shared by every markdown surface. */
export function InlineRunView({ run }: { run: InlineRun }) {
  const style = run.style;
  let content: ReactNode = run.text;
  if (style.code) {
    content = <code className="md-code">{content}</code>;
  }
  if (style.bold) {
    content = <strong>{content}</strong>;
  }
  if (style.italic) {
    content = <em>{content}</em>;
  }
  if (style.strikethrough) {
    content = <s>{content}</s>;
  }
  if (style.link !== null && style.link !== undefined) {
    // A mended link whose URL is still streaming renders styled but inert —
    // the settling URL must not collapse the line (mend.rs PENDING_LINK_URL).
    if (style.link === PENDING_LINK_URL) {
      content = <span className="md-link md-link-pending">{content}</span>;
    } else if (style.image === true) {
      // An image run that reached the plain inline path (nested inside a
      // link's label, or a veiled piece) still renders as media.
      content = <MarkdownImage run={run} />;
    } else {
      content = <MarkdownLink href={style.link}>{content}</MarkdownLink>;
    }
  }
  return <>{content}</>;
}

// ---------------------------------------------------------------------------
// Images — `text_element`'s media split (render.rs:1593-1637)
// ---------------------------------------------------------------------------

/**
 * Inline content with image runs: surrounding text and images stack as a
 * `flex-col gap:8` column in original order ("several images with captions"
 * lay out as separate rows). Without images this is just the runs.
 */
function RunsOrMedia({ runs }: { runs: readonly InlineRun[] }) {
  if (!runs.some((run) => run.style.image === true)) {
    return <InlineRuns runs={runs} />;
  }
  const parts: ReactNode[] = [];
  let start = 0;
  runs.forEach((run, ix) => {
    if (run.style.image === true) {
      if (start < ix) {
        parts.push(
          <span key={`t${start}`} className="md-media-text">
            <InlineRuns runs={runs.slice(start, ix)} />
          </span>,
        );
      }
      parts.push(<MarkdownImage key={`i${ix}`} run={run} />);
      start = ix + 1;
    }
  });
  if (start < runs.length) {
    parts.push(
      <span key={`t${start}`} className="md-media-text">
        <InlineRuns runs={runs.slice(start)} />
      </span>,
    );
  }
  return <span className="md-media">{parts}</span>;
}

/** An image run as a real `<img>` — the src validates like any link. */
function MarkdownImage({ run }: { run: InlineRun }) {
  const source = run.style.link ?? "";
  const alt = run.text;
  const src = source.startsWith("data:") ? source : transcriptAddress(source);
  if (src === null) {
    // The desktop's unloaded-media branch: the alt text, link-styled, inert.
    return <span className="md-link md-link-pending">{alt}</span>;
  }
  return <img className="md-img" src={src} alt={alt} />;
}

// ---------------------------------------------------------------------------
// Links — validation, destination card, context menu, activation guard
// ---------------------------------------------------------------------------

/** The link menu's card width (link_interaction.rs:273-420). */
const LINK_MENU_WIDTH = 260;
/** The destination card's show delay — gpui's hoverable-tooltip default. */
const LINK_CARD_DELAY_MS = 650;

function MarkdownLink({ href, children }: { href: string; children: ReactNode }) {
  const surface = useMarkdownSurface();
  const address = useMemo(() => transcriptAddress(href), [href]);
  const workspace = useMemo(
    () =>
      address === null && surface.workspaceRoot !== null
        ? resolveWorkspaceFileLink(href, surface.workspaceRoot)
        : null,
    [address, href, surface.workspaceRoot],
  );

  if (address === null && workspace === null) {
    // A rejected destination renders as inert styled text — the same
    // treatment a still-streaming pending link gets (markdown.tsx:145-157).
    return <span className="md-link md-link-pending">{children}</span>;
  }
  return (
    <LinkChrome href={href} address={address} workspace={workspace} surface={surface}>
      {children}
    </LinkChrome>
  );
}

/**
 * A validated link's interactive chrome: the anchor (external) or button
 * (internal workspace file), the 650ms hover destination card, the
 * right-click menu, and the click-during-selection guard
 * (`link_interaction.rs`).
 */
function LinkChrome({
  href,
  address,
  workspace,
  surface,
  children,
}: {
  href: string;
  address: string | null;
  workspace: { path: string; line: number | null; column: number | null } | null;
  surface: MarkdownSurface;
  children: ReactNode;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const downRef = useRef<{ x: number; y: number } | null>(null);

  /** `click_is_activation` (link_interaction.rs:465-474): ≤4px and no selection. */
  const activationGuard = (event: { clientX: number; clientY: number; preventDefault(): void }): boolean => {
    const start = downRef.current;
    if (start !== null && Math.hypot(event.clientX - start.x, event.clientY - start.y) > 4) {
      event.preventDefault();
      return false;
    }
    const selection = window.getSelection?.();
    if (selection !== null && selection !== undefined && selection.toString().length > 0) {
      event.preventDefault();
      return false;
    }
    return true;
  };

  const openTarget = (): void => {
    if (address !== null) {
      window.open(address, "_blank", "noopener,noreferrer");
    } else if (workspace !== null) {
      surface.openWorkspaceFile(workspace.path);
    }
  };

  const copyAddress = (): void => {
    void navigator.clipboard?.writeText(address ?? href);
  };

  const onMouseDown = (event: { button: number; clientX: number; clientY: number }): void => {
    downRef.current = event.button === 0 ? { x: event.clientX, y: event.clientY } : null;
  };

  const target =
    address !== null ? (
      <a
        className="md-link"
        href={address}
        target="_blank"
        rel="noreferrer noopener"
        onMouseDown={onMouseDown}
        onClick={(event) => {
          // Guard only — the browser still owns ordinary navigation (and
          // every modified click: ctrl/middle opens a tab, shift a window).
          activationGuard(event);
        }}
      >
        {children}
      </a>
    ) : (
      <button
        type="button"
        className="md-link md-link-internal"
        onMouseDown={onMouseDown}
        onClick={(event) => {
          if (activationGuard(event)) {
            openTarget();
          }
        }}
      >
        {children}
      </button>
    );

  return (
    <RbContextMenu open={menuOpen} onOpenChange={setMenuOpen}>
      <RbTooltip
        label={<span className="md-link-card-text">{graphemeBreaks(address ?? href)}</span>}
        placement={{ side: "bottom", align: "start" }}
        popupClassName="md-link-card"
      >
        <RbTooltipTrigger
          delay={LINK_CARD_DELAY_MS}
          render={<RbContextMenuTrigger render={target} />}
        />
      </RbTooltip>
      <RbContextMenuPortal>
        <RbContextMenuPositioner>
          <RbContextMenuPopup
            className="rb-popover-popup popover-card"
            role="menu"
            aria-label="Link actions"
            style={{ width: LINK_MENU_WIDTH }}
          >
            <MenuRow
              fadeKey="md-link-open"
              onClick={() => {
                setMenuOpen(false);
                openTarget();
              }}
            >
              <Icon name="arrowUpRight" size={16} className="md-link-menu-icon" />
              <span className="menu-row-label">Open link</span>
            </MenuRow>
            <MenuRow
              fadeKey="md-link-copy"
              onClick={() => {
                setMenuOpen(false);
                copyAddress();
              }}
            >
              <Icon name="copy" size={16} className="md-link-menu-icon" />
              <span className="menu-row-label">Copy link address</span>
            </MenuRow>
          </RbContextMenuPopup>
        </RbContextMenuPositioner>
      </RbContextMenuPortal>
    </RbContextMenu>
  );
}

// ---------------------------------------------------------------------------
// Workspace file references (render.rs:1711-1793, workspace_links.rs)
// ---------------------------------------------------------------------------

/**
 * A file icon belongs beside a link only when the whole visible paragraph is
 * one safe workspace-file target (`sole_workspace_file_link` /
 * `sole_plain_file_reference`). Mixed prose keeps the ordinary layout.
 */
function FileRefGuard({ runs, children }: { runs: readonly InlineRun[]; children: ReactNode }) {
  const surface = useMarkdownSurface();
  const path = useMemo<
    | { readonly kind: "lines"; readonly lines: string[] }
    | { readonly kind: "sole"; readonly path: string }
    | null
  >(() => {
    const root = surface.workspaceRoot;
    if (root === null) {
      return null;
    }
    const lines = plainFileReferenceLines(runs, root);
    if (lines !== null) {
      return { kind: "lines", lines };
    }
    const sole = soleWorkspaceFileLink(runs, root) ?? solePlainFileReference(runs, root);
    return sole === null ? null : { kind: "sole", path: sole };
  }, [runs, surface.workspaceRoot]);

  if (path === null) {
    return <>{children}</>;
  }
  if (path.kind === "lines") {
    return (
      <span className="md-fileref-lines">
        {path.lines.map((line, ix) => (
          <FileRefWell key={ix} path={line}>
            <span className="md-media-text">{line}</span>
          </FileRefWell>
        ))}
      </span>
    );
  }
  return <FileRefWell path={path.path}>{children}</FileRefWell>;
}

/** The 20px icon well beside a sole file reference (render.rs:1670-1691). */
function FileRefWell({ path, children }: { path: string; children: ReactNode }) {
  const appearance = useResolvedAppearance();
  return (
    <span className="md-fileref">
      <span className="md-fileref-well" style={{ background: wellBg(appearance, false) }}>
        <FileIcon kind="file" name={path} appearance={appearance} size={14} />
      </span>
      <span className="md-fileref-body">{children}</span>
    </span>
  );
}

function soleWorkspaceFileLink(runs: readonly InlineRun[], workspaceRoot: string): string | null {
  let target: string | null = null;
  for (const run of runs) {
    if (run.text.length === 0) {
      continue;
    }
    if (run.style.image === true) {
      return null;
    }
    const link = run.style.link;
    if (link === null || link === undefined) {
      return null;
    }
    if (link === PENDING_LINK_URL) {
      return null;
    }
    if (target !== null && target !== link) {
      return null;
    }
    target = link;
  }
  if (target === null) {
    return null;
  }
  return resolveWorkspaceFileLink(target, workspaceRoot)?.path ?? null;
}

function solePlainFileReference(runs: readonly InlineRun[], workspaceRoot: string): string | null {
  let text = "";
  for (const run of runs) {
    if (run.text.length === 0) {
      continue;
    }
    if ((run.style.link !== null && run.style.link !== undefined) || run.style.image === true || run.style.code) {
      return null;
    }
    text += run.text;
  }
  const candidate = text.trim();
  if (candidate.length === 0 || /\s/.test(candidate)) {
    return null;
  }
  const path = resolveWorkspaceFileLink(candidate, workspaceRoot)?.path ?? null;
  return path !== null && hasSpecificFileIcon(path) ? path : null;
}

function plainFileReferenceLines(runs: readonly InlineRun[], workspaceRoot: string): string[] | null {
  if (runs.length !== 1) {
    return null;
  }
  const run = runs[0]!;
  if (
    (run.style.link !== null && run.style.link !== undefined) ||
    run.style.image === true ||
    run.style.code ||
    !run.text.includes("\n")
  ) {
    return null;
  }
  const lines: string[] = [];
  for (const raw of run.text.split("\n")) {
    const candidate = raw.trim();
    if (candidate.length === 0 || /\s/.test(candidate)) {
      return null;
    }
    const path = resolveWorkspaceFileLink(candidate, workspaceRoot)?.path ?? null;
    if (path === null || !hasSpecificFileIcon(path)) {
      return null;
    }
    lines.push(candidate);
  }
  return lines.length > 1 ? lines : null;
}

// ---------------------------------------------------------------------------
// Lists — markers and the task checkbox (render.rs:552-690)
// ---------------------------------------------------------------------------

/**
 * The task checkbox (render.rs:570-627): 16×16, radius 3, accent
 * border/fill + a 12px check when checked. The desktop's TRANSCRIPT wires no
 * toggle handler (`tasks: None`, transcript.rs:5459/5507 — only the editable
 * files preview passes one), so the parity-correct web box is disabled too:
 * opacity 0.5, no cursor. An `onToggle` prop is the seam a future
 * message-edit path plugs into.
 */
function TaskCheckbox({ checked, onToggle }: { checked: boolean; onToggle?: () => void }) {
  const interactive = onToggle !== undefined;
  return (
    <span
      className={`md-checkbox${checked ? " md-checkbox-done" : ""}`}
      role="checkbox"
      aria-checked={checked}
      aria-disabled={!interactive}
      tabIndex={interactive ? 0 : undefined}
      onClick={onToggle}
    >
      {checked && <Icon name="check" size={12} className="md-checkbox-check" />}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Tables — `table_columns` applied as per-column floors (render.rs:727-742)
// ---------------------------------------------------------------------------

function MarkdownTable({ block }: { block: Extract<Block, { kind: "table" }> }) {
  const tableRef = useRef<HTMLTableElement>(null);
  // Minimum column widths from the ported `table_columns`: content-proportional
  // columns (the browser's auto layout shares the desktop's flex algorithm)
  // floored at each column's minimum, so the table scrolls once the floors
  // exceed the viewport instead of crushing columns.
  const [minimums, setMinimums] = useState<readonly number[] | null>(null);

  useLayoutEffect(() => {
    const table = tableRef.current;
    if (table === null || typeof document === "undefined") {
      return;
    }
    const style = getComputedStyle(table);
    const content = columnContentWidths(block, style.font, style.fontFamily);
    if (content === null) {
      return;
    }
    setMinimums(tableColumns(content).minimums);
  }, [block]);

  const cellStyle = (ix: number, align: TableAlign | undefined): CSSProperties => ({
    textAlign: alignCss(align),
    minWidth: minimums === null ? undefined : `${minimums[ix] ?? 0}px`,
  });

  return (
    <div className="md-table-wrap">
      <table className="md-table" ref={tableRef}>
        <thead>
          <tr>
            {block.header.map((cell, ix) => (
              <th key={ix} style={cellStyle(ix, block.align[ix])}>
                <RunsOrMedia runs={cell} />
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {block.rows.map((row, rowIx) => (
            <tr key={rowIx}>
              {row.map((cell, cellIx) => (
                <td key={cellIx} style={cellStyle(cellIx, block.align[cellIx])}>
                  <RunsOrMedia runs={cell} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * Per-column max-content widths measured with a 2D canvas — the web
 * equivalent of shaping each cell's runs unwrapped. Headers measure bold.
 */
function columnContentWidths(
  block: Extract<Block, { kind: "table" }>,
  font: string,
  fontFamily: string,
): number[] | null {
  const ctx = measureCanvas();
  if (ctx === null) {
    return null;
  }
  const columns = block.align.length;
  const widths = new Array<number>(columns).fill(0);
  type TableRow = readonly (readonly InlineRun[])[];
  const rows: readonly TableRow[] = [block.header, ...block.rows];
  rows.forEach((row, rowIndex) => {
    row.forEach((cell, ix) => {
      if (ix >= columns) {
        return;
      }
      ctx.font = `${rowIndex === 0 ? 700 : 400} ${font || "14px"} ${fontFamily || "sans-serif"}`;
      const width = ctx.measureText(cell.map((run) => run.text).join("")).width;
      widths[ix] = Math.max(widths[ix] ?? 0, width);
    });
  });
  return widths;
}

let measureCtx: CanvasRenderingContext2D | null | undefined;

function measureCanvas(): CanvasRenderingContext2D | null {
  if (measureCtx === undefined) {
    measureCtx =
      typeof document === "undefined" ? null : document.createElement("canvas").getContext("2d");
  }
  return measureCtx;
}

// ---------------------------------------------------------------------------
// Code blocks — header, actions, veil (render.rs:1881-2254)
// ---------------------------------------------------------------------------

/**
 * A fenced code block: the 28px header (verbatim fence-info label, the
 * global fit toggle, copy), the 12.5/18 mono body, and — on live rows — the
 * per-line veil fade over freshly appended code.
 *
 * Mermaid fences render as their source, deliberately: the desktop's
 * diagram closure lives in the files preview, and shipping Mermaid.js
 * (megabytes of bundle) into the engine-embedded app for a diagram renderer
 * is judged too heavy — see the ticket's Comments.
 */
export function CodeBlock({
  code,
  language,
  chunks = null,
  onChunkEnd,
}: {
  code: string;
  language: string | null;
  /** Live rows only: fading chunk ranges over the flat code text. */
  chunks?: readonly VeilChunk[] | null;
  onChunkEnd?: (key: string) => void;
}) {
  const [copied, setCopied] = useState(false);
  const settings = useUiSettings();
  const fit = settings.codeFencesFitContent;
  // Code blocks scale 1:1 off the code font size (render.rs) — the same
  // value the transcript estimator's analytic code-row height reads.
  const codeTextPx = codeBlockTextSize(settings.codeFontSize);
  const codeLinePx = codeBlockLineHeight(settings.codeFontSize);
  const lines = useMemo(() => splitTokenLines(highlightCode(code, language)), [code, language]);

  const copy = (): void => {
    const clipboard = (navigator as Navigator | undefined)?.clipboard;
    if (clipboard === undefined) {
      return;
    }
    void clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };

  const toggleFit = (): void => {
    // A persisted, GLOBAL preference — every code fence flips together
    // (render.rs:267-271, settings.rs:626).
    uiSettings.updateImmediate({ codeFencesFitContent: !fit });
  };

  return (
    <div
      className={`md-codeblock${fit ? " md-codeblock-fit" : ""}`}
      style={{
        ["--rb-code-size-px" as string]: `${codeTextPx}px`,
        ["--rb-code-line-height" as string]: `${codeLinePx}px`,
      }}
    >
      <div className="md-codehead">
        <div className="md-codehead-lang">{language ?? ""}</div>
        <div className="md-codehead-actions">
          <Tooltip
            label={fit ? "Use horizontal scrolling" : "Fit content"}
            trigger={
              <button
                type="button"
                className={`md-action${fit ? " md-action-active" : ""}`}
                aria-label={fit ? "Use horizontal scrolling" : "Fit content"}
                aria-pressed={fit}
                onClick={toggleFit}
              >
                <Icon name="wrapText" size={13} />
              </button>
            }
          />
          <button
            type="button"
            className="md-action md-copy"
            aria-label={copied ? "Copied" : "Copy"}
            onClick={copy}
          >
            <Icon name={copied ? "check" : "copy"} size={12} />
            {copied && <span className="md-copy-label">Copied</span>}
          </button>
        </div>
      </div>
      <pre className="md-pre">
        <code>
          <CodeLines
            lines={lines}
            code={code}
            chunks={chunks}
            onChunkEnd={onChunkEnd ?? (() => {})}
          />
        </code>
      </pre>
    </div>
  );
}

function CodeLines({
  lines,
  code,
  chunks,
  onChunkEnd,
}: {
  lines: readonly SyntaxToken[][];
  code: string;
  chunks: readonly VeilChunk[] | null;
  onChunkEnd: (key: string) => void;
}) {
  // Line offsets over the flat code text (render.rs:2135-2140's scan).
  let offset = 0;
  return (
    <>
      {lines.map((line, ix) => {
        const start = offset;
        const text = line.map((token) => token.text).join("");
        offset = start + text.length + 1;
        return (
          <span key={ix} className="md-codeline">
            {chunks === null || chunks.length === 0 ? (
              line.map((token, tokenIx) => (
                <SyntaxTokenView key={tokenIx} text={token.text} role={token.role} />
              ))
            ) : (
              <VeiledCodeLine
                tokens={line}
                lineStart={start}
                lineEnd={start + text.length}
                chunks={chunks}
                onChunkEnd={onChunkEnd}
              />
            )}
            {"\n"}
          </span>
        );
      })}
    </>
  );
}

/** One code line with its tail chunks dissolving in (`slice_spans`). */
function VeiledCodeLine({
  tokens,
  lineStart,
  lineEnd,
  chunks,
  onChunkEnd,
}: {
  tokens: readonly SyntaxToken[];
  lineStart: number;
  lineEnd: number;
  chunks: readonly VeilChunk[];
  onChunkEnd: (key: string) => void;
}) {
  return (
    <>
      {sliceTokensForVeil(tokens, lineStart, lineEnd, chunks).map((piece, ix) =>
        piece.chunk === null ? (
          <SyntaxTokenView key={ix} text={piece.text} role={piece.token.role} />
        ) : (
          <span
            key={ix}
            className="veil-fade"
            style={{ animationDuration: `${piece.chunk.durationMs}ms` }}
            onAnimationEnd={() => onChunkEnd(piece.chunk!.key)}
          >
            <SyntaxTokenView text={piece.text} role={piece.token.role} />
          </span>
        ),
      )}
    </>
  );
}

function SyntaxTokenView({ text, role }: { text: string; role: SyntaxRole | null }) {
  if (role === null) {
    return <>{text}</>;
  }
  return <span className={`tk-${role}`}>{text}</span>;
}
