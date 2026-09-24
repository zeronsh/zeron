import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  buildHeadingAnchors,
  markdownLinkTarget,
  relativeTarget,
  type MdBlock,
  type MdInline,
  type TaskMarker,
} from "../../lib/markdown-doc";
import { CodeBlock } from "../markdown";
import { ImageView } from "./image-view";

/**
 * The files markdown preview (crates/ui/src/files/markdown_preview.rs):
 * the shared block renderer's output at the files-specific metrics —
 * 900px max content width (not the transcript's 736px), `py(16)`, 12px
 * block gap, centered column with 24px side padding — rendering the LIVE
 * editor buffer. Code fences ride the shared `CodeBlock` (highlighting +
 * copy + the 1200ms "Copied" reset); task checkboxes toggle back into the
 * buffer through a single edit; links resolve workspace-relative via
 * `relative_target`; images load through the workspace RPC under the
 * per-document media limits and open the lightbox (an `ImageView` at
 * natural size) on click.
 *
 * Mermaid fences render as their source code block (a JS Mermaid renderer
 * is out of scope for this ticket — noted in its Comments).
 */

/** `MAX_PREVIEW_CONTENT_WIDTH` (markdown_preview.rs). */
const MAX_PREVIEW_CONTENT_PX = 900;
/** `MD_BLOCK_GAP` (markdown_preview.rs). */
const MD_BLOCK_GAP_PX = 12;
/** `MAX_MEDIA_ENTRIES` — distinct sources per document (markdown_media.rs). */
const MAX_MEDIA_ENTRIES = 32;
/** `MAX_MEDIA_BYTES` — combined decoded budget per document. */
const MAX_MEDIA_BYTES = 64 * 1024 * 1024;
/** Rendered media height cap (markdown_preview.rs). */
const MEDIA_MAX_HEIGHT_PX = 480;

export interface WorkspaceImageLoad {
  readonly url: string;
  readonly width: number;
  readonly height: number;
  /** Decoded size estimate (w × h × 4) for the document budget. */
  readonly decodedBytes: number;
}

export interface MarkdownViewProps {
  readonly blocks: readonly MdBlock[];
  readonly documentPath: string;
  readonly truncated: boolean;
  readonly editable: boolean;
  readonly onToggleTask: (marker: TaskMarker, next: boolean) => void;
  readonly onOpenPath: (path: string) => void;
  /** Resolves and loads a workspace-relative image, or null when offline. */
  readonly loadImage: ((path: string) => Promise<WorkspaceImageLoad>) | null;
}

type MediaEntry =
  | { readonly kind: "loading" }
  | { readonly kind: "loaded"; readonly load: WorkspaceImageLoad }
  | { readonly kind: "error"; readonly message: string };

interface LightboxState {
  readonly load: WorkspaceImageLoad;
  readonly alt: string;
  readonly restoreFocus: HTMLElement | null;
}

export function MarkdownView(props: MarkdownViewProps) {
  const [media, setMedia] = useState<ReadonlyMap<string, MediaEntry>>(new Map());
  const mediaRef = useRef(media);
  mediaRef.current = media;
  const budgetRef = useRef(0);
  const [lightbox, setLightbox] = useState<LightboxState | null>(null);
  const anchors = useMemo(() => buildHeadingAnchors(props.blocks), [props.blocks]);

  const entryFor = useCallback(
    (resolvedPath: string): MediaEntry => {
      const existing = media.get(resolvedPath);
      if (existing !== undefined) {
        return existing;
      }
      if (media.size >= MAX_MEDIA_ENTRIES) {
        return { kind: "error", message: "Document image preview limit reached" };
      }
      return { kind: "loading" };
    },
    [media],
  );

  // Media loads: the first 32 distinct sources register; each decode feeds
  // the document's 64 MiB combined budget. The media map is read through a
  // ref so an entry landing never cancels the other in-flight loads.
  useEffect(() => {
    if (props.loadImage === null) {
      return;
    }
    const targets = collectImageSources(props.blocks, props.documentPath);
    const pending = targets.filter(
      (path) => !mediaRef.current.has(path) && mediaRef.current.size < MAX_MEDIA_ENTRIES,
    );
    if (pending.length === 0) {
      return;
    }
    let cancelled = false;
    const loadImage = props.loadImage;
    const load = async (path: string): Promise<void> => {
      setMedia((current) => (current.has(path) ? current : new Map(current).set(path, { kind: "loading" })));
      try {
        const result = await loadImage(path);
        if (cancelled || budgetRef.current + result.decodedBytes > MAX_MEDIA_BYTES) {
          if (!cancelled) {
            setMedia((current) =>
              new Map(current).set(path, { kind: "error", message: "Document media preview memory limit reached" }),
            );
          }
          return;
        }
        budgetRef.current += result.decodedBytes;
        if (!cancelled) {
          setMedia((current) => new Map(current).set(path, { kind: "loaded", load: result }));
        }
      } catch (error) {
        if (!cancelled) {
          setMedia((current) =>
            new Map(current).set(path, {
              kind: "error",
              message: error instanceof Error ? error.message : String(error),
            }),
          );
        }
      }
    };
    for (const path of pending) {
      void load(path);
    }
    return () => {
      cancelled = true;
    };
  }, [props.blocks, props.documentPath, props.loadImage]);

  const openLightbox = useCallback(
    (load: WorkspaceImageLoad, alt: string): void => {
      setLightbox({ load, alt, restoreFocus: document.activeElement instanceof HTMLElement ? document.activeElement : null });
    },
    [],
  );

  const closeLightbox = useCallback((): void => {
    setLightbox((current) => {
      current?.restoreFocus?.focus();
      return null;
    });
  }, []);

  return (
    <div className="files-markdown">
      {props.truncated && (
        <p className="files-markdown-truncated" role="status">Large file preview is truncated and read-only.</p>
      )}
      {props.blocks.length === 0 && <p className="files-markdown-empty">Loading preview…</p>}
      {props.blocks.map((block, index) => (
        <MarkdownRow key={index}>
          <MarkdownBlock
            block={block}
            anchor={anchors.get(block) ?? null}
            documentPath={props.documentPath}
            editable={props.editable}
            onToggleTask={props.onToggleTask}
            onOpenPath={props.onOpenPath}
            media={media}
            entryFor={entryFor}
            openLightbox={openLightbox}
            onAnchorScroll={scrollToAnchor}
          />
        </MarkdownRow>
      ))}
      {lightbox !== null &&
        createPortal(
          <MarkdownLightbox state={lightbox} onClose={closeLightbox} onOpenImageLink={() => props.onOpenPath(props.documentPath)} />,
          document.body,
        )}
    </div>
  );
}

/** One block row: the centered 900px column with the 12px block gap. */
function MarkdownRow({ children }: { children: ReactNode }) {
  return (
    <div className="files-markdown-row" style={{ paddingBottom: MD_BLOCK_GAP_PX }}>
      <div className="files-markdown-content" style={{ maxWidth: MAX_PREVIEW_CONTENT_PX }}>
        {children}
      </div>
    </div>
  );
}

function MarkdownBlock(props: {
  readonly block: MdBlock;
  readonly anchor: string | null;
  readonly documentPath: string;
  readonly editable: boolean;
  readonly onToggleTask: (marker: TaskMarker, next: boolean) => void;
  readonly onOpenPath: (path: string) => void;
  readonly media: ReadonlyMap<string, MediaEntry>;
  readonly entryFor: (path: string) => MediaEntry;
  readonly openLightbox: (load: WorkspaceImageLoad, alt: string) => void;
  readonly onAnchorScroll: (anchor: string) => void;
}): ReactNode {
  const { block } = props;
  switch (block.kind) {
    case "heading": {
      const Tag = (`h${Math.min(6, Math.max(1, block.level))}`) as "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
      const className = `md-h md-h${Math.min(6, Math.max(1, block.level))}`;
      return (
        <Tag className={className} id={props.anchor ?? undefined}>
          <Inlines inlines={block.inlines} {...inlineProps(props)} />
        </Tag>
      );
    }
    case "paragraph":
      return (
        <p className="md-p">
          <Inlines inlines={block.inlines} {...inlineProps(props)} />
        </p>
      );
    case "code":
      // Mermaid fences render as their source (see the file header).
      return <CodeBlock code={block.text} language={block.language} />;
    case "quote":
      return (
        <blockquote className="md-quote">
          {block.blocks.map((child, index) => (
            <MarkdownBlock key={index} {...props} block={child} anchor={null} />
          ))}
        </blockquote>
      );
    case "list": {
      const Tag = block.ordered ? "ol" : "ul";
      return (
        <Tag className="md-list">
          {block.items.map((item, index) => (
            <li key={index} className={item.task !== null ? "md-task" : undefined}>
              {item.task !== null && (
                <TaskCheckbox marker={item.task} editable={props.editable} onToggle={props.onToggleTask} />
              )}
              {item.blocks.map((child, childIndex) => (
                <MarkdownBlock key={childIndex} {...props} block={child} anchor={null} />
              ))}
            </li>
          ))}
        </Tag>
      );
    }
    case "table":
      return (
        <div className="md-table-wrap">
          <table className="md-table">
            <thead>
              <tr>
                {block.header.map((cell, index) => (
                  <th key={index}>
                    <Inlines inlines={cell} {...inlineProps(props)} />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, index) => (
                <tr key={index}>
                  {row.map((cell, cellIndex) => (
                    <td key={cellIndex}>
                      <Inlines inlines={cell} {...inlineProps(props)} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "rule":
      return <hr className="md-rule" />;
  }
}

function inlineProps(props: Parameters<typeof MarkdownBlock>[0]) {
  return {
    documentPath: props.documentPath,
    onOpenPath: props.onOpenPath,
    onAnchorScroll: props.onAnchorScroll,
    media: props.media,
    entryFor: props.entryFor,
    openLightbox: props.openLightbox,
  };
}

/** GFM task checkbox — the files preview's version is interactive. */
function TaskCheckbox({
  marker,
  editable,
  onToggle,
}: {
  readonly marker: TaskMarker;
  readonly editable: boolean;
  readonly onToggle: (marker: TaskMarker, next: boolean) => void;
}) {
  return (
    <button
      type="button"
      className={`md-checkbox files-task${marker.checked ? " md-checkbox-done" : ""}`}
      disabled={!editable}
      aria-pressed={marker.checked}
      aria-label={marker.checked ? "Mark as not done" : "Mark as done"}
      onClick={() => onToggle(marker, !marker.checked)}
    >
      {marker.checked ? "☑" : "☐"}
    </button>
  );
}

function Inlines(props: {
  readonly inlines: readonly MdInline[];
  readonly documentPath: string;
  readonly onOpenPath: (path: string) => void;
  readonly onAnchorScroll: (anchor: string) => void;
  readonly media: ReadonlyMap<string, MediaEntry>;
  readonly entryFor: (path: string) => MediaEntry;
  readonly openLightbox: (load: WorkspaceImageLoad, alt: string) => void;
}): ReactNode {
  return props.inlines.map((inline, index) => {
    switch (inline.kind) {
      case "text":
        return <span key={index}>{inline.text}</span>;
      case "code":
        return <code key={index} className="md-code">{inline.text}</code>;
      case "bold":
        return (
          <strong key={index}>
            <Inlines {...props} inlines={inline.children} />
          </strong>
        );
      case "italic":
        return (
          <em key={index}>
            <Inlines {...props} inlines={inline.children} />
          </em>
        );
      case "strike":
        return (
          <s key={index}>
            <Inlines {...props} inlines={inline.children} />
          </s>
        );
      case "image":
        return <MdImage key={index} src={inline.src} alt={inline.alt} {...props} />;
      case "link":
        return <MdLink key={index} href={inline.href} documentPath={props.documentPath} onOpenPath={props.onOpenPath} onAnchorScroll={props.onAnchorScroll}>
          <Inlines {...props} inlines={inline.children} />
        </MdLink>;
    }
  });
}

function MdLink({
  href,
  documentPath,
  onOpenPath,
  onAnchorScroll,
  children,
}: {
  readonly href: string;
  readonly documentPath: string;
  readonly onOpenPath: (path: string) => void;
  readonly onAnchorScroll: (anchor: string) => void;
  readonly children: ReactNode;
}) {
  const target = markdownLinkTarget(href);
  if (target.kind === "external") {
    return (
      <a className="md-link" href={target.href} target="_blank" rel="noreferrer noopener">
        {children}
      </a>
    );
  }
  if (target.kind === "text") {
    return <>{children}</>;
  }
  const resolved = relativeTarget(documentPath, target.path);
  if (resolved === null) {
    return <>{children}</>;
  }
  const sameFile = resolved.path === documentPath;
  const anchor = resolved.anchor;
  return (
    <a
      className="md-link"
      href="#"
      onClick={(event) => {
        event.preventDefault();
        if (sameFile) {
          if (anchor !== null) {
            onAnchorScroll(anchor);
          }
          return;
        }
        onOpenPath(resolved.path);
      }}
    >
      {children}
    </a>
  );
}

function MdImage(props: {
  readonly src: string;
  readonly alt: string;
  readonly documentPath: string;
  readonly media: ReadonlyMap<string, MediaEntry>;
  readonly entryFor: (path: string) => MediaEntry;
  readonly openLightbox: (load: WorkspaceImageLoad, alt: string) => void;
}): ReactNode {
  if (/^https?:\/\//i.test(props.src)) {
    return (
      <img
        className="markdown-image files-markdown-image"
        src={props.src}
        alt={props.alt}
        style={{ maxWidth: MAX_PREVIEW_CONTENT_PX, maxHeight: MEDIA_MAX_HEIGHT_PX }}
      />
    );
  }
  const resolved = relativeTarget(props.documentPath, props.src);
  if (resolved === null) {
    return <span className="files-markdown-media-error">{props.alt} — Image path is outside the workspace</span>;
  }
  const entry = props.entryFor(resolved.path);
  if (entry.kind === "loading") {
    return <span className="files-markdown-media-note">Loading image…</span>;
  }
  if (entry.kind === "error") {
    return <span className="files-markdown-media-error">{props.alt} — {entry.message}</span>;
  }
  return (
    <button
      type="button"
      className="files-markdown-image-button"
      aria-label="Enlarge image"
      onClick={() => props.openLightbox(entry.load, props.alt)}
    >
      <img
        className="markdown-image files-markdown-image"
        src={entry.load.url}
        alt={props.alt}
        style={{ maxWidth: MAX_PREVIEW_CONTENT_PX, maxHeight: MEDIA_MAX_HEIGHT_PX }}
      />
    </button>
  );
}

function MarkdownLightbox({
  state,
  onClose,
  onOpenImageLink,
}: {
  readonly state: LightboxState;
  readonly onClose: () => void;
  readonly onOpenImageLink: () => void;
}) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [onClose]);

  return (
    <div className="files-lightbox-scrim" role="dialog" aria-modal="true" aria-label="Image preview" onClick={onClose}>
      <div className="files-lightbox-card" onClick={(event) => event.stopPropagation()}>
        <ImageView
          src={state.load.url}
          natural={{ width: state.load.width, height: state.load.height }}
          alt={state.alt}
          onImageClick={onClose}
        />
        <button type="button" className="files-lightbox-open" onClick={onOpenImageLink}>
          Open image link
        </button>
      </div>
    </div>
  );
}

/** Same-file anchor scroll: the slug id the heading carries. */
function scrollToAnchor(anchor: string): void {
  document.getElementById(anchor)?.scrollIntoView({ block: "start", behavior: "instant" });
}

/** Every distinct workspace-relative image source in the tree, in order. */
function collectImageSources(blocks: readonly MdBlock[], documentPath: string): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  const walkInlines = (inlines: readonly MdInline[]): void => {
    for (const inline of inlines) {
      if (inline.kind === "image" && !/^https?:\/\//i.test(inline.src)) {
        const resolved = relativeTarget(documentPath, inline.src);
        if (resolved !== null && !seen.has(resolved.path)) {
          seen.add(resolved.path);
          out.push(resolved.path);
        }
      } else if (inline.kind === "bold" || inline.kind === "italic" || inline.kind === "strike") {
        walkInlines(inline.children);
      } else if (inline.kind === "link") {
        walkInlines(inline.children);
      }
    }
  };
  const walk = (list: readonly MdBlock[]): void => {
    for (const block of list) {
      if (block.kind === "heading" || block.kind === "paragraph") {
        walkInlines(block.inlines);
      } else if (block.kind === "quote") {
        walk(block.blocks);
      } else if (block.kind === "list") {
        for (const item of block.items) {
          walk(item.blocks);
        }
      } else if (block.kind === "table") {
        for (const cell of block.header) {
          walkInlines(cell);
        }
        for (const row of block.rows) {
          for (const cell of row) {
            walkInlines(cell);
          }
        }
      }
    }
  };
  walk(blocks);
  return out;
}
