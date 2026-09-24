/**
 * `@zeron/icons` — the desktop's control-icon set, in the browser.
 *
 * The glyphs under `./generated/` are produced by `scripts/generate.mjs` from
 * `crates/ui/assets/icons/*.svg`, the same assets `crates/ui/src/icons.rs`
 * embeds. Every paint in them is `currentColor`, so an icon tints with the
 * `color` of whatever it sits in — the web equivalent of gpui's
 * `icon(path).text_color(…)`.
 *
 * Sizes are px, like the desktop's `.size(px(14.0))`. `<Icon>` renders a bare
 * `<svg>`; sizing and color belong to the call site.
 */
import type { CSSProperties } from "react";

import { iconAssets, type IconAsset, type IconName } from "./generated/index";

export { iconAssets };
export type { IconAsset, IconName };

export interface IconProps {
  name: IconName;
  /** Edge length in px. The desktop's icon calls are all square. */
  size?: number;
  /** Extra classes; the glyph inherits `color` from its box either way. */
  className?: string;
  style?: CSSProperties;
  /**
   * Icons are decorative by default (the control around them carries the
   * label). Pass a title only when the glyph is the whole affordance.
   */
  title?: string;
}

export function Icon({ name, size = 16, className, style, title }: IconProps) {
  const asset = iconAssets[name];
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox={asset.viewBox}
      fill={asset.fill}
      style={style}
      aria-hidden={title === undefined ? true : undefined}
      role={title === undefined ? undefined : "img"}
      focusable="false"
      // The generated body is build-time output from files in this repo, not
      // user or engine content.
      dangerouslySetInnerHTML={{
        __html: (title === undefined ? "" : `<title>${title}</title>`) + asset.body,
      }}
    />
  );
}

/**
 * Anthropic's brand orange. gpui tints SVGs wholesale, so the desktop applies
 * this at the call site (`crates/ui/src/icons.rs::claude_brand`); the web does
 * the same through `color`.
 */
export const CLAUDE_BRAND = "#D97757";

/** The harness ids the desktop's `HarnessId` enum serializes to. */
export type HarnessId =
  | "claudeCode"
  | "codex"
  | "cursor"
  | "devin"
  | "grok"
  | "hermes"
  | "pi"
  | "opencode"
  | "antigravity"
  | "mock";

/**
 * Brand mark + optional fixed tint for a harness — the exact table in
 * `crates/ui/src/pickers.rs::harness_brand_icon`. A `null` tint means the
 * surface tints the monochrome mark itself.
 */
export function harnessBrandIcon(harness: string): { name: IconName; tint: string | null } {
  switch (harness) {
    // Both spellings: the wire's kebab-case "claude-code" (proto HarnessId
    // serializes kebab-case) and the desktop's enum spelling.
    case "claudeCode":
    case "claude-code":
    case "mock":
      return { name: "claudeMark", tint: CLAUDE_BRAND };
    case "codex":
      return { name: "openaiMark", tint: null };
    case "cursor":
      return { name: "cursorMark", tint: null };
    case "devin":
      return { name: "devinMark", tint: null };
    case "grok":
      return { name: "grokMark", tint: null };
    case "hermes":
      return { name: "hermesMark", tint: null };
    case "pi":
      return { name: "piMark", tint: null };
    case "opencode":
      return { name: "opencodeMark", tint: null };
    // Google's Antigravity mark, monochrome like every other agent mark.
    case "antigravity":
      return { name: "antigravityMark", tint: null };
    default:
      return { name: "bot", tint: null };
  }
}
