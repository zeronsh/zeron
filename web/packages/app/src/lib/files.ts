import type {
  WorkspaceEntry,
  WorkspaceEntryKind,
  WorkspaceFileText,
  WorkspaceLineEnding,
  WorkspaceReadOnlyReason,
  WorkspaceTextEncoding,
  WorkspaceWritableEncoding,
  WorkspaceWritableLineEnding,
} from "@zeron/proto";

/**
 * Pure helpers for the files surface, ported from the desktop's files model
 * (crates/ui/src/files/) so the web tree reads identically: same entry order,
 * same path math on workspace-relative "/" paths, same read-only copy.
 */

/** Desktop `entry_rank` (model.rs): directories, then files, then symlinks. */
export function entryRank(kind: WorkspaceEntryKind): number {
  switch (kind) {
    case "directory":
      return 0;
    case "file":
      return 1;
    case "symlink":
      return 2;
  }
}

/** Desktop `compare_paths` (model.rs): rank, then case-insensitive name, then path. */
export function compareEntries(left: WorkspaceEntry, right: WorkspaceEntry): number {
  const byRank = entryRank(left.kind) - entryRank(right.kind);
  if (byRank !== 0) {
    return byRank;
  }
  const byName = left.name.toLowerCase().localeCompare(right.name.toLowerCase());
  if (byName !== 0) {
    return byName;
  }
  return left.path < right.path ? -1 : left.path > right.path ? 1 : 0;
}

/** Desktop `parent_path` (model.rs): workspace paths are "/" separated. */
export function parentPath(path: string): string | null {
  const slash = path.lastIndexOf("/");
  if (slash >= 0) {
    return path.slice(0, slash);
  }
  return path.length > 0 ? "" : null;
}

/** Desktop `is_direct_child` (model.rs). */
export function isDirectChild(candidate: string, directory: string): boolean {
  return parentPath(candidate) === directory;
}

/** The final path component. */
export function fileName(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash >= 0 ? path.slice(slash + 1) : path;
}

/** Desktop `is_image` (image_preview.rs): extensions that get an image preview. */
export function isImagePath(path: string): boolean {
  const extension = extensionOf(path);
  return (
    extension !== null &&
    ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "tiff"].includes(extension)
  );
}

/** Desktop `is_markdown` (markdown_preview.rs): extensions that get a markdown preview. */
export function isMarkdownPath(path: string): boolean {
  const extension = extensionOf(path);
  return extension === "md" || extension === "markdown";
}

function extensionOf(path: string): string | null {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  if (dot <= 0) {
    return null;
  }
  return name.slice(dot + 1).toLowerCase();
}

/** Desktop `read_only_message` (preview.rs), verbatim copy. */
export function readOnlyMessage(reason: WorkspaceReadOnlyReason | null): string {
  switch (reason) {
    case "binary":
      return "Binary files cannot be previewed.";
    case "unsupportedEncoding":
      return "This file encoding is not supported.";
    case "symlink":
      return "Symlink targets are read-only.";
    case "permissionDenied":
      return "Permission denied.";
    case "tooLarge":
      return "This file is too large to preview.";
    case "mixedLineEndings":
      return "Files with mixed line endings are read-only.";
    case "notRegularFile":
    case null:
      return "This file cannot be previewed.";
  }
}

/**
 * The truncated-preview banner (preview.rs:2766-2779): shown above the code
 * scroll when text came back but the read was clipped — reachable today via
 * the markdown client-side 2 MiB clip (the server's own truncation always
 * pairs with no text).
 */
export function truncatedMessage(file: WorkspaceFileText): string | null {
  return file.truncated && file.text != null ? "Large file preview is truncated and read-only." : null;
}

/** Desktop document.rs `writable_encoding`. */
export function writableEncoding(encoding: WorkspaceTextEncoding): WorkspaceWritableEncoding | null {
  switch (encoding) {
    case "utf8":
      return "utf8";
    case "utf8Bom":
      return "utf8Bom";
    case "binary":
    case "unsupported":
      return null;
  }
}

/** Desktop document.rs `writable_line_ending`. */
export function writableLineEnding(
  lineEnding: WorkspaceLineEnding | null | undefined,
): WorkspaceWritableLineEnding | null {
  switch (lineEnding) {
    case "lf":
    case "none":
      return "lf";
    case "crlf":
      return "crlf";
    case "mixed":
    case null:
    case undefined:
      return null;
  }
}

/**
 * Desktop document.rs `read_only_reason`: the engine's explicit reason wins;
 * anything undecodable as an editable snapshot (truncated, no text, no hash,
 * non-writable encoding or line endings) degrades to notRegularFile.
 */
export function fileReadOnlyReason(file: WorkspaceFileText): WorkspaceReadOnlyReason | null {
  if (file.readOnlyReason !== null && file.readOnlyReason !== undefined) {
    return file.readOnlyReason;
  }
  const undecodable =
    file.truncated ||
    file.text === null ||
    file.text === undefined ||
    file.contentHash === null ||
    file.contentHash === undefined ||
    writableEncoding(file.encoding) === null ||
    writableLineEnding(file.lineEnding) === null;
  return undecodable ? "notRegularFile" : null;
}

/** Compact byte size for the tree and the viewer header ("12 B", "4.2 KB"). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) {
    return "";
  }
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  const kib = bytes / 1024;
  if (kib < 1024) {
    return `${kib >= 100 ? Math.round(kib) : kib.toFixed(1)} KB`;
  }
  const mib = kib / 1024;
  return `${mib >= 100 ? Math.round(mib) : mib.toFixed(1)} MB`;
}
