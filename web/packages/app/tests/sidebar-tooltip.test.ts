import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createViewOptionsTooltip } from "../src/components/space-filter";
import { TOOLTIP_VIEW_OPTIONS_MS } from "../src/components/ui/Tooltip";

/**
 * The "Sidebar view options" label's show/hide controller — the web port
 * of gpui's `.tooltip(…)` + `.tooltip_show_delay(350ms)` contract
 * (spaces.rs:1150-1151): show after the delay while hovered, dismiss when
 * the pointer leaves; the delay is the shared `TOOLTIP_VIEW_OPTIONS_MS`
 * (spaces.rs:960-966), never inlined. Driven entirely through the pure
 * factory — the component only wires DOM events onto it. The recorded
 * history is the label's visible-state timeline: `true` appears only
 * from a hover arm firing, never from focus.
 */

/** A `setVisible` double recording the label's visible-state history. */
function recordVisible(): { calls: boolean[]; setVisible: (next: boolean) => void } {
  const calls: boolean[] = [];
  return { calls, setVisible: (next: boolean) => { calls.push(next); } };
}

describe("createViewOptionsTooltip", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("viewOptionsTooltipAppearsAfterDelayAndDismissesOnLeave", () => {
    const { calls, setVisible } = recordVisible();
    const control = createViewOptionsTooltip(setVisible);

    // Pending: under the 350ms delay, nothing shows (a quick pass).
    control.enter();
    vi.advanceTimersByTime(TOOLTIP_VIEW_OPTIONS_MS - 1);
    expect(calls).toEqual([]);

    // At the delay, the label shows — once.
    vi.advanceTimersByTime(1);
    expect(calls).toEqual([true]);

    // Leave after firing: the label dismisses and no timer survives.
    control.leave();
    expect(calls).toEqual([true, false]);
    expect(vi.getTimerCount()).toBe(0);

    // Leave BEFORE firing: the pending arm is cleared, so the full delay
    // elapsing afterward never leaks a visible label — the only call is
    // the leave's own dismissal.
    calls.length = 0;
    control.enter();
    control.leave();
    vi.advanceTimersByTime(10 * TOOLTIP_VIEW_OPTIONS_MS);
    expect(calls).toEqual([false]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("viewOptionsTooltipReArmDoesNotStackTimers", () => {
    const { calls, setVisible } = recordVisible();
    const control = createViewOptionsTooltip(setVisible);

    control.enter();
    control.leave();
    control.enter();
    // Exactly one pending arm — the first was cleared on leave and again
    // on re-arm, so no zombie 350ms firing can follow the leave.
    expect(vi.getTimerCount()).toBe(1);

    // The first arm's firing window elapses without ever showing the
    // label (only the leave's dismissal is on the tape)…
    vi.advanceTimersByTime(TOOLTIP_VIEW_OPTIONS_MS - 1);
    expect(calls).toEqual([false]);
    // …and the SECOND arm shows on its own schedule, exactly once.
    vi.advanceTimersByTime(1);
    expect(calls).toEqual([false, true]);
  });

  it("viewOptionsTooltipNeverArmsOnFocus", () => {
    const { calls, setVisible } = recordVisible();
    const control = createViewOptionsTooltip(setVisible);

    // The controller has no focus-arm path at all — the desktop's label
    // is hover-only (spaces.rs:1150-1151) and the trigger's aria-label
    // serves assistive tech — and blur, its defensive counterpart,
    // clears and hides without ever arming.
    control.blur();
    vi.advanceTimersByTime(10 * TOOLTIP_VIEW_OPTIONS_MS);
    expect(calls).toEqual([false]);

    // Even a pending hover arm does not survive a blur.
    control.enter();
    control.blur();
    vi.advanceTimersByTime(10 * TOOLTIP_VIEW_OPTIONS_MS);
    expect(calls).toEqual([false, false]);
    expect(vi.getTimerCount()).toBe(0);

    // Escape and unmount hold the same defensive line: dismiss, never arm.
    control.enter();
    control.escape();
    vi.advanceTimersByTime(TOOLTIP_VIEW_OPTIONS_MS);
    control.enter();
    control.dispose();
    vi.advanceTimersByTime(TOOLTIP_VIEW_OPTIONS_MS);
    expect(calls).toEqual([false, false, false, false]);
  });
});
