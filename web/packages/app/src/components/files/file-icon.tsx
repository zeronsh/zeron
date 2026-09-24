import type { CSSProperties } from "react";
import type { Appearance } from "@zeron/theme";
import { resolveDirectoryIcon, resolveFileIcon } from "../../lib/file-icons";

/**
 * `FileIcon` — the polychrome file-type icon, rendered as an `<img>` of the
 * resolved manifest asset (desktop `file_icons::icon`, a `gpui::img`).
 *
 * The img form is the point: these are authored multi-color VS Code-derived
 * SVGs whose fills must NOT be tinted — unlike the monochrome `@zeron/icons`
 * `Icon`, which is a `currentColor` control glyph. Dark appearance is a
 * different asset path (`file-icons/dark/…`), so a theme switch forces a
 * fresh image load by construction. (file_icons.rs §icon / tree.rs:184-186)
 */

export type FileIconKind = "file" | "directory" | "symlink";

export interface FileIconProps {
  /** The entry kind; symlinks resolve by filename like files (no target kind). */
  readonly kind: FileIconKind;
  /** The name or path to resolve — basenames win, extensions follow. */
  readonly name: string;
  /** Threaded for the caller's chevron glyph; the folder image ignores it. */
  readonly expanded?: boolean;
  readonly appearance: Appearance;
  /** Default 14 — `tree.rs:184-186` / `search.rs` rows / drag ghosts. */
  readonly size?: number;
  readonly className?: string;
  readonly style?: CSSProperties;
}

export function FileIcon({ kind, name, expanded, appearance, size = 14, className, style }: FileIconProps) {
  const src =
    kind === "directory"
      ? resolveDirectoryIcon(name, appearance, expanded)
      : resolveFileIcon(name, appearance);
  return (
    <img
      className={className}
      style={style}
      src={src}
      alt=""
      width={size}
      height={size}
      draggable={false}
      loading="lazy"
    />
  );
}
