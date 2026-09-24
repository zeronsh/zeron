import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { Icon } from "@zeron/icons";
import { highlightCode, splitTokenLines, type SyntaxToken } from "../../lib/syntax";
import { isMarkdownPath } from "../../lib/files";
import {
  EDITOR_COMMENT_DRAFT_HEIGHT,
  editorCommentOverlayHorizontal,
  editorCommentOverlayTop,
  cardHeight,
  type ReviewComment,
} from "../../lib/review-comments";
import { HorizontalScrollbar, MenuScrollbar } from "../ui/Scrollbar";
import { editorTextSize, previewLineHeight, previewTextSize } from "../../lib/typography";
import { EditorCommentCard } from "../review-comments/editor-comment-card";
import { EditorCommentDraft } from "../review-comments/editor-comment-draft";

/**
 * `CodeView` — the file viewer's text layer (crates/ui/src/files/preview.rs:
 * `render_document_body` / `ensure_editor` / `render_preview_line`). One
 * component renders both the read-only preview and the editable buffer,
 * exactly like the desktop's single text state: gutter + line numbers +
 * syntax-highlighted rows, with only the input layer toggled. Editing rides
 * a transparent `<textarea>` laid over the highlight layer (the research's
 * sanctioned shape — no CodeMirror/Monaco); both layers share the same
 * font metrics, paddings, and wrap mode so the caret sits exactly on the
 * colored text.
 *
 * Layout ports (preview.rs:3108-3189):
 * - gutter 48px, right-aligned mono 10px `text_faint` at 0.7, right border
 *   at 0.55.
 * - read-only rows: 20px line height, mono 11.5px, base color `text` at
 *   0.93, code cell padding 12/18, `nowrap` rows scroll horizontally.
 * - editable rows: the `editorFontSize` setting drives size and
 *   `line-height = max(size + 8.5, 20)`; selection washes `accent` at 0.22,
 *   the caret is `--rb-caret`, the active line `ink` at 0.025.
 * - word wrap: rows stretch and wrap; off: nowrap rows (the scroller gives
 *   native horizontal scroll with a working vertical wheel).
 *
 * Ticket 23 seam: gutter cells render through `renderGutterCell` — a later
 * per-row gutter affordance slots in there without restructuring the row
 * loop.
 */

/** `PREVIEW_LINE_HEIGHT` (preview.rs:34) — the editor row-height floor. */
const PREVIEW_LINE_HEIGHT = 20;
/** The highlight debounce (preview.rs request_editor_highlight: 120ms). */
const HIGHLIGHT_DEBOUNCE_MS = 120;

export interface CodeViewProps {
  readonly text: string;
  readonly path: string;
  /** Whether the input layer accepts keystrokes. */
  readonly editable: boolean;
  readonly onChange: (text: string) => void;
  /**
   * `codeFontSize` — the shared code setting (typography.rs): the editable
   * editor scales off its 13px baseline, the read-only preview off its
   * 11.5px one (`editor_text_size` / `preview_text_size`).
   */
  readonly codeFontSize: number;
  readonly wordWrap: boolean;
  /** Focus the input layer once mounted (the markdown toggle's off-ramp). */
  readonly autoFocus?: boolean;
  /** The host's handle to the input layer (the context menu's target). */
  readonly inputRef?: RefObject<HTMLTextAreaElement | null>;
  /**
   * Ticket 23's editor-side comments (preview.rs::render_editor_comment_
   * overlays, :2784-3060): the File-sourced staged set for THIS path, the
   * open card/draft, and their actions. Null on read-only documents (the
   * desktop renders the overlay only over a live editor).
   */
  readonly review?: CodeReviewWiring | null;
}

/** The editor-side comment wiring `CodeView` hosts (preview.rs:2794-2893). */
export interface CodeReviewWiring {
  /** The File-sourced staged comments for this path. */
  readonly comments: readonly ReviewComment[];
  /** The open editor card's comment id (`preview.active_comment`). */
  readonly activeId: string | null;
  /** The open editor draft, when it belongs to this path. */
  readonly draft: { readonly path: string; readonly line: number; readonly body: string; readonly editingId: string | null } | null;
  readonly onOpenDraft: (line: number) => void;
  readonly onToggleActive: (id: string) => void;
  readonly onCardEdit: (id: string) => void;
  readonly onCardRemove: (id: string) => void;
  readonly onDraftBody: (body: string) => void;
  readonly onDraftCancel: () => void;
  readonly onDraftCommit: () => void;
}

/** The tokenizer's language hint: the file's own extension. */
function languageForPath(path: string): string | null {
  const name = path.split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  if (dot <= 0) {
    return null;
  }
  return name.slice(dot + 1).toLowerCase();
}

export function CodeView({ text, path, editable, onChange, codeFontSize, wordWrap, autoFocus, inputRef, review }: CodeViewProps) {
  const language = useMemo(() => languageForPath(path), [path]);
  // Markdown files highlight as markdown; everything else keys off its
  // extension (`lib/syntax.ts` resolves aliases).
  const effectiveLanguage = isMarkdownPath(path) ? "markdown" : language;
  const lines = useMemo(() => text.split("\n"), [text]);
  const [tokenLines, setTokenLines] = useState<SyntaxToken[][]>(() =>
    splitTokenLines(highlightCode(text, effectiveLanguage)),
  );
  const [highlightedSource, setHighlightedSource] = useState(text);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [activeLine, setActiveLine] = useState(0);

  // ── Ticket 23: the editor-side comment overlay ──────────────────────────
  // The card/draft floats at the anchor row's bottom edge, clamped into the
  // scroll viewport (`editor_comment_overlay_top`, preview.rs:3301-3309);
  // scrolling repositions it with the row, exactly like the desktop's
  // window-space bounds math recomputed per render.
  const [scrollTick, setScrollTick] = useState(0);
  useEffect(() => {
    const el = scrollRef.current;
    if (el === null) {
      return;
    }
    const onScroll = (): void => setScrollTick((tick) => tick + 1);
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);
  const activeComment =
    review?.activeId === null || review === null || review === undefined || review.activeId === null
      ? null
      : review.comments.find((comment) => comment.id === review.activeId) ?? null;
  const overlayDraft = review !== null && review !== undefined && review.draft !== null ? review.draft : null;
  const overlayAnchorLine = overlayDraft !== null ? overlayDraft.line : activeComment !== null ? activeComment.line : null;
  const overlayHeight = overlayDraft !== null ? EDITOR_COMMENT_DRAFT_HEIGHT : activeComment !== null ? cardHeight(activeComment.body) : 0;
  const [overlay, setOverlay] = useState<{ left: number; top: number; width: number } | null>(null);
  useLayoutEffect(() => {
    const scroller = scrollRef.current;
    if (scroller === null || overlayAnchorLine === null) {
      setOverlay(null);
      return;
    }
    const row = scroller.querySelector<HTMLElement>(`[data-line="${overlayAnchorLine}"]`);
    if (row === null) {
      setOverlay(null);
      return;
    }
    const viewport = scroller.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    const rowTop = rowRect.top - viewport.top;
    const gutter = row.querySelector<HTMLElement>(".files-code-gutter");
    const gutterPx = Math.min(Math.max(gutter?.getBoundingClientRect().width ?? 36, 24), 64);
    const horizontal = editorCommentOverlayHorizontal(gutterPx, scroller.clientWidth);
    const top = editorCommentOverlayTop(rowTop, rowRect.height, overlayHeight, scroller.clientHeight);
    setOverlay(top === null ? null : { left: horizontal.left, top, width: horizontal.width });
  }, [overlayAnchorLine, overlayHeight, scrollTick, lines.length, text, codeFontSize, wordWrap, editable, review?.activeId, overlayDraft]);

  // Keep the host's handle (the editor context menu's target) live.
  useEffect(() => {
    if (inputRef !== undefined) {
      inputRef.current = textareaRef.current;
    }
  });

  useEffect(() => {
    if (autoFocus && editable) {
      textareaRef.current?.focus();
    }
  }, [autoFocus, editable]);

  // Read-only views tokenize synchronously; editable ones debounce at the
  // desktop's 120ms so long files keep typing smooth — the plain rows hold
  // the current text while tokens catch up.
  useEffect(() => {
    if (!editable) {
      setTokenLines(splitTokenLines(highlightCode(text, effectiveLanguage)));
      setHighlightedSource(text);
      return;
    }
    if (text === highlightedSource) {
      return;
    }
    const timer = setTimeout(() => {
      setTokenLines(splitTokenLines(highlightCode(text, effectiveLanguage)));
      setHighlightedSource(text);
    }, HIGHLIGHT_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [text, effectiveLanguage, editable, highlightedSource]);

  const measureCaretLine = useCallback((): void => {
    const textarea = textareaRef.current;
    if (textarea === null) {
      return;
    }
    setActiveLine(text.slice(0, textarea.selectionStart).split("\n").length - 1);
  }, [text]);

  // The active-line wash follows the caret (editor_adapter.rs's
  // active_line highlight).
  useEffect(() => {
    if (!editable) {
      return;
    }
    document.addEventListener("selectionchange", measureCaretLine);
    return () => {
      document.removeEventListener("selectionchange", measureCaretLine);
    };
  }, [editable, measureCaretLine]);

  return (
    <div
      className={[
        "files-code",
        editable ? "files-code-editable" : "",
        wordWrap ? "files-code-wrap" : "",
      ]
        .filter((part) => part.length > 0)
        .join(" ")}
      style={
        editable
          ? {
              fontSize: `${editorTextSize(codeFontSize)}px`,
              lineHeight: `${Math.max(editorTextSize(codeFontSize) + 8.5, PREVIEW_LINE_HEIGHT)}px`,
            }
          : {
              fontSize: `${previewTextSize(codeFontSize)}px`,
              lineHeight: `${previewLineHeight(codeFontSize)}px`,
            }
      }
    >
      <div ref={scrollRef} className="files-code-scroll">
        <div className="files-code-content">
          {lines.map((line, index) => (
            <div
              key={index}
              data-line={index + 1}
              className={`files-code-row${editable && index === activeLine ? " files-code-row-active" : ""}`}
            >
              {renderGutterCell(index, review)}
              <span className="files-code-line">
                {renderTokens(line, index, tokenLines)}
              </span>
            </div>
          ))}
          {editable && (
            <textarea
              ref={textareaRef}
              className="files-code-input"
              value={text}
              wrap={wordWrap ? "soft" : "off"}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              aria-label={`Edit ${path.split("/").pop() ?? path}`}
              onChange={(event) => {
                onChange(event.target.value);
                measureCaretLine();
              }}
              onKeyUp={measureCaretLine}
              onClick={measureCaretLine}
              onSelect={measureCaretLine}
            />
          )}
        </div>
      </div>
      {/* Ticket 23: the floating comment card/draft overlays
          (render_editor_comment_overlays, preview.rs:2860-2893) — absolute
          children of the code view, above the scroll layer, at the anchor
          math's clamped left/top/width. */}
      {review !== null && review !== undefined && overlay !== null && activeComment !== null ? (
        <EditorCommentCard
          comment={activeComment}
          left={overlay.left}
          top={overlay.top}
          width={overlay.width}
          onEdit={review.onCardEdit}
          onRemove={review.onCardRemove}
        />
      ) : null}
      {review !== null && review !== undefined && overlay !== null && overlayDraft !== null ? (
        <EditorCommentDraft
          left={overlay.left}
          top={overlay.top}
          width={overlay.width}
          body={overlayDraft.body}
          editing={overlayDraft.editingId !== null}
          placeholder={isMarkdownPath(path) ? "Request a change…" : "Add a comment…"}
          onBody={review.onDraftBody}
          onCancel={review.onDraftCancel}
          onCommit={review.onDraftCommit}
        />
      ) : null}
      {/* The floating rails replace the native scrollbars (popover.rs's
          scrollbar pair — the code plane's share of the 21/25 debt). */}
      <MenuScrollbar scrollRef={scrollRef} />
      <HorizontalScrollbar scrollRef={scrollRef} />
    </div>
  );
}

/**
 * Ticket 23's gutter cell (preview.rs:2801-2857): the line number, with the
 * comment affordance overlaying it — a `chatRoundLine` icon on lines that
 * carry a staged File comment (the cell gets the card plate + hover wash,
 * click toggles the card), or a hover-revealed solid `+` (16px, 11px plus)
 * on lines without one, click opens the draft.
 */
function renderGutterCell(lineIndex: number, review: CodeReviewWiring | null | undefined): ReactNode {
  const line = lineIndex + 1;
  const comment = review?.comments.find((candidate) => candidate.line === line);
  const affordance =
    comment !== undefined ? (
      <button
        type="button"
        className="files-gutter-comment"
        aria-label={`Open comment on line ${line}`}
        onClick={() => review!.onToggleActive(comment.id)}
      >
        <Icon name="chatRoundLine" size={10.5} />
      </button>
    ) : review !== null && review !== undefined ? (
      <button
        type="button"
        className="files-gutter-add"
        aria-label={`Comment on line ${line}`}
        onClick={() => review.onOpenDraft(line)}
      >
        <span className="files-gutter-add-button">
          <Icon name="plus" size={11} />
        </span>
      </button>
    ) : null;
  return (
    <span className={`files-code-gutter${comment !== undefined ? " files-code-gutter-commented" : ""}`}>
      {affordance}
      <span className="files-code-gutter-number">{line}</span>
    </span>
  );
}

function renderTokens(line: string, index: number, tokenLines: readonly (readonly SyntaxToken[])[]): ReactNode[] {
  const tokens = index < tokenLines.length ? tokenLines[index] : undefined;
  if (tokens === undefined || tokens.length === 0) {
    // An empty line still holds its row height (white-space: pre collapses
    // an empty line box without a placeholder).
    return ["\u00a0"];
  }
  return tokens.map((token, tokenIndex) => (
    <span key={tokenIndex} className={token.role === null ? undefined : `tk-${token.role}`}>
      {token.text}
    </span>
  ));
}
