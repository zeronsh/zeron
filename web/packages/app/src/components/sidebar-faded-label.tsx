import { useEffect, useRef, type ReactNode } from "react";

/**
 * `sidebar_faded_label` (shell.rs, upstream 01b705fe): a nowrap label that
 * outgrows its slot fades its last band of pixels instead of cutting an
 * ellipsis — the glass sidebar has no paintable "behind the window" color to
 * overlay, so the fade has to come out of the content itself.
 *
 * The web's mechanism is a right-edge mask whose ramp width is
 * `--rb-label-fade-inset`: 0 while the label fits (fitting labels stay
 * intact), easing toward one 20px band as the overflow grows to a band —
 * the smooth onset of `label_fade_outset`, so a label a fraction too wide
 * does not suddenly dim its final characters. A shared module-level
 * ResizeObserver measures every mounted label and re-measures on resize,
 * the web peer of the desktop's per-label tracked scroll handle. Ancestor
 * masks compose with this one for free, so the sidebar's vertical scroll
 * fade keeps fading an overflowing label's rows — what gpui's nested
 * EdgeFade scopes needed `inherit_vertical_fade` for.
 */

/** The fade band (edge_fade.rs): 20px, and the onset spans one band. */
export const SIDEBAR_LABEL_FADE_BAND = 20;

/**
 * `label_fade_outset` (edge_fade.rs): how much of the band stays OUT of the
 * label. The visible ramp is the complement — 0 at no overflow, one full
 * band once the overflow reaches a band.
 */
export function labelFadeOutset(overflow: number, band = SIDEBAR_LABEL_FADE_BAND): number {
  const ramp = Math.max(band, 1);
  const progress = Math.min(Math.max(overflow / ramp, 0), 1);
  const eased = progress * progress * (3 - 2 * progress);
  return ramp * (1 - eased);
}

/** The visible ramp width — `band − label_fade_outset(overflow)`. */
export function labelFadeInset(overflow: number, band = SIDEBAR_LABEL_FADE_BAND): number {
  return Math.max(band, 1) - labelFadeOutset(overflow, band);
}

function applyLabelFade(el: HTMLElement): void {
  const overflow = el.scrollWidth - el.clientWidth;
  el.style.setProperty("--rb-label-fade-inset", `${labelFadeInset(overflow)}px`);
}

/*
 * One observer for every mounted label (the desktop's paint-time gate is
 * likewise shared): a sidebar's worth of rows costs one RO, and every entry
 * re-measures exactly the label that resized.
 */
let labelObserver: ResizeObserver | null = null;

function observeLabel(el: HTMLElement): () => void {
  if (labelObserver === null && typeof ResizeObserver !== "undefined") {
    labelObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        applyLabelFade(entry.target as HTMLElement);
      }
    });
  }
  labelObserver?.observe(el);
  applyLabelFade(el);
  return () => {
    labelObserver?.unobserve(el);
  };
}

/**
 * The faded label. `fill` gives the wrapper `flex: 1` (title/device slots
 * that own their row's remaining width); without it the wrapper shrinks but
 * never grows, matching the desktop's `flex_none` branch/device labels.
 * `className` carries the slot's own typography classes.
 */
export function SidebarFadedLabel({
  className,
  fill = false,
  children,
}: {
  className?: string;
  fill?: boolean;
  children: ReactNode;
}) {
  const ref = useRef<HTMLSpanElement | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (el === null) {
      return;
    }
    return observeLabel(el);
  }, []);
  return (
    <span
      ref={ref}
      className={`sidebar-label-fade${fill ? " sidebar-label-fill" : ""}${className === undefined ? "" : ` ${className}`}`}
    >
      <span className="sidebar-label-fade-inner">{children}</span>
    </span>
  );
}
