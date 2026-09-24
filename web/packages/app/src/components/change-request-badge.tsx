import { Icon } from "@zeron/icons";
import type { ChangeRequestState, ChangeRequestSummary } from "@zeron/proto";

/**
 * The change-request badge — the web peer of `crates/ui/src/change_requests.rs`
 * (`pull_request_badge`, 117-169). A pill that shows the PR glyph (composer
 * size only) and the mono `#N`; the state word never appears in the badge
 * itself — the tooltip carries it ("PR #N · State"). Click opens the change
 * request's own URL. Tone by state: Open → success, Merged → code text (the
 * accent), Closed → danger.
 *
 * Two size presets, exactly the desktop's: sidebar (h16, gap 0, px 4,
 * radius 4, 10px, no glyph) and composer (h20, gap 5, px 7, radius 6, 11px,
 * an 11px PULL_REQUEST glyph). The tooltip is a CSS-hover card — the
 * research's sanctioned native substitute for the desktop's 350ms tooltip —
 * delayed 350ms to match.
 *
 * Used at the desktop's two call sites: the sidebar chat row and the
 * composer footer. (The Changes pane's own CR card is a documented,
 * intentional web-only addition, not a port of this.)
 */

export type BadgeTone = "open" | "merged" | "closed";

export function toneFor(state: ChangeRequestState): BadgeTone {
  switch (state) {
    case "open":
      return "open";
    case "merged":
      return "merged";
    case "closed":
      return "closed";
  }
}

export const TONE_LABEL: Readonly<Record<BadgeTone, string>> = {
  open: "Open",
  merged: "Merged",
  closed: "Closed",
};

export interface ChangeRequestBadgeProps {
  readonly summary: ChangeRequestSummary;
  readonly size?: "composer" | "sidebar";
}

export function ChangeRequestBadge({ summary, size = "sidebar" }: ChangeRequestBadgeProps) {
  const tone = toneFor(summary.state);
  const label = TONE_LABEL[tone];
  const number = `#${summary.number}`;
  const title = summary.title.replace(/[\r\n]+/g, " ");
  const tooltipId = `cr-${summary.provider}-${summary.number}`;
  const composer = size === "composer";

  return (
    <a
      className={`cr-badge cr-badge-${tone} ${composer ? "cr-badge-composer" : "cr-badge-sidebar"}`}
      href={summary.url}
      target="_blank"
      rel="noreferrer noopener"
      aria-describedby={tooltipId}
      onClick={(event) => event.stopPropagation()}
    >
      {composer ? <Icon name="pullRequest" size={11} className="cr-badge-glyph" aria-hidden /> : null}
      <span className="cr-badge-number mono">{number}</span>
      <span className="cr-tooltip" role="tooltip" id={tooltipId}>
        <span className={`cr-tooltip-line cr-tooltip-line-${tone}`}>{`PR ${number} · ${label}`}</span>
        <span className="cr-tooltip-title">{title}</span>
      </span>
    </a>
  );
}
