import { useState } from "react";
import type { ContextUsage } from "@zeron/proto";
import { PickerCard } from "./ui/PickerCard";
import { TOOLTIP_CONTEXT_METER_MS } from "./ui/Tooltip";

/**
 * Context occupancy — the desktop's `context_usage.rs`, read from the
 * replicated chat snapshot and never from local CLI state.
 *
 * A 16px ring: a faint full track with the used arc growing clockwise from 12
 * o'clock, and the percentage beside it. The color escalates with pressure —
 * muted below 75%, warning from 75%, danger from 90% — and an unknown window
 * reads as a faint em dash rather than a guess.
 *
 * The `Context window` card (context_usage.rs:71-137) opens on a 500ms
 * hover (gpui `DEFAULT_TOOLTIP_SHOW_DELAY`) through `PickerCard`'s
 * hover-open shape — a content card, not the label tooltip
 * (`base/tooltip.tsx`'s own routing rule). While open it live-updates: the
 * usage flows through the footer's props, and Base UI keeps the popup
 * mounted (a re-render, never a re-mount). The card carries no fixed width:
 * the lines break only at their own newlines and the card sizes from the
 * unwrapped text (context_usage.rs render — the old fixed 260px soft-wrapped
 * and clipped the last line). The native `title=` stand-in is
 * gone — both tooltips would otherwise show at once.
 */

/** `ContextUsage::fraction` — `None` unless both halves are present and sane. */
export function usageFraction(usage: ContextUsage | null): number | null {
  if (usage === null) {
    return null;
  }
  const { tokens, window } = usage;
  if (tokens === null || window === null || window <= 0) {
    return null;
  }
  return tokens / window;
}

/**
 * `with_separators` (context_usage.rs:85-101) — counts grouped by thousands,
 * so the tooltip reads like a token meter, not a wall of digits.
 */
export function withSeparators(count: number): string {
  const digits = String(count);
  let grouped = "";
  for (let index = 0; index < digits.length; index += 1) {
    if (index > 0 && (digits.length - index) % 3 === 0) {
      grouped += ",";
    }
    grouped += digits[index];
  }
  return grouped;
}

/**
 * `has_window` (context_usage.rs:105-111) — whether the indicator has
 * anything to measure against: harnesses that never report a window
 * (antigravity) get no indicator at all, rather than a permanently empty
 * ring.
 */
export function hasWindow(usage: ContextUsage | null): boolean {
  return usage?.window != null && usage.window > 0;
}

/**
 * `details` (context_usage.rs:85-108) — the card body's four verbatim cases.
 */
export function usageDetails(usage: ContextUsage | null): string {
  const tokens = usage?.tokens ?? null;
  const window = usage?.window ?? null;
  if (tokens !== null && window !== null && window > 0) {
    const remaining = Math.max(window - tokens, 0);
    return `${withSeparators(tokens)} / ${withSeparators(window)} tokens\n${withSeparators(remaining)} tokens remaining`;
  }
  if (tokens !== null) {
    return `${withSeparators(tokens)} tokens used\nContext limit not reported`;
  }
  if (window !== null && window > 0) {
    return `${withSeparators(window)} token capacity\nWaiting for context usage`;
  }
  return "Context usage not reported by this harness yet";
}

const RADIUS = 6;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

export function ContextUsageIndicator({ usage }: { usage: ContextUsage | null }) {
  const [open, setOpen] = useState(false);
  const fraction = usageFraction(usage);
  const tone =
    fraction === null ? "none" : fraction >= 0.9 ? "danger" : fraction >= 0.75 ? "warning" : "muted";
  const filled = Math.min(Math.max(fraction ?? 0, 0), 1);
  const label = fraction === null ? "—" : `${Math.round(fraction * 100)}%`;
  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement={{ side: "top", align: "center" }}
      cardClassName="popover-card context-usage-card"
      ariaLabel="Context window"
      openOnHover
      hoverDelayMs={TOOLTIP_CONTEXT_METER_MS}
      trigger={
        <div className="context-usage" data-tone={tone}>
          <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
            {/* Rotated so both arcs start at 12 o'clock, like the desktop's paths. */}
            <g transform="rotate(-90 8 8)" fill="none" strokeWidth="1.8">
              <circle cx="8" cy="8" r={RADIUS} className="context-usage-track" />
              <circle
                cx="8"
                cy="8"
                r={RADIUS}
                className="context-usage-arc"
                strokeDasharray={`${CIRCUMFERENCE * filled} ${CIRCUMFERENCE}`}
                strokeLinecap="butt"
              />
            </g>
          </svg>
          <span>{label}</span>
        </div>
      }
    >
      <ContextUsageTooltip usage={usage} />
    </PickerCard>
  );
}

/**
 * The card content (context_usage.rs:110-137): the 12px MEDIUM "Context
 * window" title over the four `details` strings, 12/19 in `text_muted`. The
 * live-update subscription is the footer's re-render — the usage flows
 * through the indicator's props, and Base UI keeps the popup mounted (a
 * re-render, never a re-mount).
 */
export function ContextUsageTooltip({ usage }: { usage: ContextUsage | null }) {
  return (
    <>
      <div className="context-usage-card-title">Context window</div>
      <div className="context-usage-card-body">{usageDetails(usage)}</div>
    </>
  );
}
