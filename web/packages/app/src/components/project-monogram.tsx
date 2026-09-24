import { useState } from "react";
import { Tooltip } from "./ui/Tooltip";
import { TOOLTIP_VIEW_OPTIONS_MS } from "./ui/Tooltip";
import { monogramCss, monogramLetter, monogramTone } from "../lib/monogram";
import { useProjectIcon } from "../state/project-icons";
import { useResolvedAppearance } from "../state/appearance";

/**
 * Project monograms — the web peer of `crates/ui/src/shell/project_icon.rs`
 * (upstream 1ec74e40 → 378a1945): a project's initial on a small frosted
 * tile, tone from the curated palette (`lib/monogram.ts`), with the
 * pull-request-badge-style tooltip card below replaced by the app's label
 * tooltip naming the project and (from b58af627/378a1945) its owning
 * device. Repository artwork — the web port of the desktop's client-side
 * `ICON_PATHS` probe (`lib/project-icons.ts`) — replaces the monogram when
 * it resolves; the monogram stays the loading and fallback state.
 */

/**
 * One monogram tile. `active` is the row's selected state: the active row
 * wears the hover tint permanently (upstream 378a1945).
 */
export function ProjectMonogram({
  name,
  seed,
  active = false,
}: {
  readonly name: string;
  readonly seed: string;
  readonly active?: boolean;
}) {
  const appearance = useResolvedAppearance();
  const tone = monogramTone(seed);
  const variant: "light" | "dark" = appearance === "dark" ? "dark" : "light";
  // 8% plate / 85% letter at rest; 24% plate / full letter on the active
  // row — emitted as CSS variables so the row's hover/selected states can
  // strengthen the tint without a re-render (app.css's `.project-monogram`
  // rules pick the pair).
  const style = {
    "--rb-mono-plate": monogramCss(tone, variant, 0.08),
    "--rb-mono-plate-active": monogramCss(tone, variant, 0.24),
    "--rb-mono-ink": monogramCss(tone, variant, 0.85),
    "--rb-mono-ink-active": monogramCss(tone, variant, 1),
  } as React.CSSProperties;
  return (
    <span className="project-monogram" data-active={active ? "1" : undefined} style={style}>
      {monogramLetter(name)}
    </span>
  );
}

/**
 * One artwork tile: the fetched bytes as an image, contain-fit like the
 * desktop's `ObjectFit::Contain`. A file that fails to decode in the
 * browser (corrupt or truncated artwork) falls back to the monogram — the
 * web peer of `decode_project_icon(...).ok()`, which prefers the default
 * over showing unrelated lower-priority art.
 */
function ProjectIconArt({
  src,
  name,
  seed,
  active = false,
}: {
  readonly src: string;
  readonly name: string;
  readonly seed: string;
  readonly active?: boolean;
}) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return <ProjectMonogram name={name} seed={seed} active={active} />;
  }
  return (
    <img
      className="project-icon-art"
      src={src}
      alt=""
      draggable={false}
      onError={() => setFailed(true)}
    />
  );
}

/**
 * The project icon frame (project_icon.rs's `project_icon_frame`): the
 * monogram with a 350ms tooltip naming the project and its device — the
 * b58af627/378a1945 device tooltip, web-shaped as the label tooltip. The
 * space's probed artwork replaces the monogram when it has landed
 * (`useProjectIcon`; loading, miss, and no-space rows keep the monogram).
 */
export function ProjectIconMark({
  name,
  seed,
  device,
  active = false,
  spaceId = null,
}: {
  readonly name: string;
  readonly seed: string;
  readonly device: string;
  readonly active?: boolean;
  /** The row's scoped space id; null (project-less) never probes. */
  readonly spaceId?: string | null;
}) {
  const icon = useProjectIcon(spaceId);
  return (
    <Tooltip
      label={`${name} — ${device}`}
      delay={TOOLTIP_VIEW_OPTIONS_MS}
      trigger={
        <span className="project-icon-mark">
          {icon !== null ? (
            <ProjectIconArt key={icon} src={icon} name={name} seed={seed} active={active} />
          ) : (
            <ProjectMonogram name={name} seed={seed} active={active} />
          )}
        </span>
      }
    />
  );
}
