import type { CSSProperties, ReactElement } from "react";
import type { ThemeVariant } from "@zeron/theme";

/**
 * The Appearance page's two live theme previews, ports of
 * `crates/ui/src/settings/appearance.rs`'s `palette_preview` (`:734-747`)
 * and `miniature`/`miniature_split` (`:624-725`). Both re-derive their
 * colors from a variant object — NOT the document's CSS variables — because
 * each card previews a different variant/accent than the one currently
 * installed (the Light card previews the light variant while the app runs
 * dark, the import dialog previews a candidate family). Inline
 * `color-mix`/hex here comes from that variant data, so the "no literal
 * hex" token rule does not apply — these ARE token values, just someone
 * else's.
 */

/** `bar(fraction, tone)` (appearance.rs:502-508): 5px, radius 3. */
function bar(fraction: number, tone: string, key: string): ReactElement {
  return <span key={key} className="miniature-bar" style={{ width: `${fraction * 100}%`, background: tone }} />;
}

/**
 * `palette_preview` (appearance.rs:734-747): the 30×18 three-way swatch —
 * surface / background / accent — that identifies a variant in the
 * theme-family menus and the import rows.
 */
export function PalettePreview(props: { readonly variant: ThemeVariant }) {
  const accent = props.variant.accent.primary;
  const style: CSSProperties = {
    background: props.variant.colors.shell,
  };
  return (
    <span className="palette-preview" aria-hidden="true">
      <span className="palette-preview-third" style={style} />
      <span className="palette-preview-third" style={{ background: props.variant.colors.background }} />
      <span className="palette-preview-third" style={{ background: accent }} />
    </span>
  );
}

/**
 * `miniature` (appearance.rs:635-676): the 44px "sidebar" column plus the
 * bordered "content" pane, all bars in the theme's text at 22% (line) and
 * 34% (strong). `corners` rounds only the half the System card's split
 * clips (appearance.rs:627-634) — the preview rounds ITSELF to the option
 * card's 6px radius (widgets.rs:71-76: gpui's clip is square, and so is
 * CSS `overflow: hidden` on the frame).
 */
export function ThemeMiniature(props: {
  readonly variant: ThemeVariant;
  readonly corners: "all" | "left" | "right";
}) {
  const { colors } = props.variant;
  const line = `color-mix(in srgb, ${colors.text} 22%, transparent)`;
  const strong = `color-mix(in srgb, ${colors.text} 34%, transparent)`;
  const sidebarBars = [
    bar(0.7, strong, "sb-1"),
    bar(1, line, "sb-2"),
    bar(0.85, line, "sb-3"),
    bar(1, line, "sb-4"),
  ];
  const contentBars = [
    bar(0.62, strong, "ct-1"),
    bar(0.88, line, "ct-2"),
    bar(0.76, line, "ct-3"),
    bar(0.52, line, "ct-4"),
  ];
  return (
    <div className={`miniature miniature-${props.corners}`} style={{ background: colors.shell }}>
      <div className="miniature-sidebar">{sidebarBars}</div>
      <div className="miniature-content" style={{ borderColor: colors.border, background: colors.background }}>
        {contentBars}
      </div>
    </div>
  );
}

/**
 * `preview` (appearance.rs:680-725): the artwork an Appearance mode card
 * carries — the System card's split (light left, dark right, each half
 * clipped to its own corners), or one full miniature for Light/Dark.
 */
export function ThemeModePreview(props: {
  readonly mode: "system" | "light" | "dark";
  readonly lightVariant: ThemeVariant;
  readonly darkVariant: ThemeVariant;
}) {
  if (props.mode === "system") {
    return (
      <div className="miniature-split">
        <div className="miniature-half">
          <ThemeMiniature variant={props.lightVariant} corners="left" />
        </div>
        <div className="miniature-half">
          <ThemeMiniature variant={props.darkVariant} corners="right" />
        </div>
      </div>
    );
  }
  return (
    <ThemeMiniature variant={props.mode === "light" ? props.lightVariant : props.darkVariant} corners="all" />
  );
}
