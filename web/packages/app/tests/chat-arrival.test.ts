import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import type { MessagePart, SessionMessageEntry, ToolCall } from "@zeron/proto";
import { parseMarkdown } from "../src/lib/markdown";
import { rowsForEntry, type TranscriptRow } from "../src/lib/transcript";
import { ToolGroupMotionStore } from "../src/lib/tool-motion";
import { ARRIVAL_HARD_CAP_MS, ARRIVAL_QUIESCE_MS, ChatArrivalWindow } from "../src/lib/chat-arrival";
import { StickController } from "../src/components/stick-controller";

// Behavior lives in the mounted outlet suite. Keep route wiring and the
// shared layout contract covered here; neither proves browser paint.
describe("live arrival outlet wiring", () => {
  const source = readFileSync(new URL("../src/routes/chat-page.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");

  it("passes the selected store to the outlet and leaves the departing veil outside", () => {
    expect(source).toMatch(/<ChatTranscriptOutlet store=\{liveTranscript\} departing=\{departing\}>[\s\S]*?<TranscriptView[\s\S]*?store=\{activeStore\}[\s\S]*?<\/ChatTranscriptOutlet>[\s\S]*?className="departing-veil"/);
    expect(source).not.toContain("lastPaintedTranscriptRef");
  });

  it("preserves flex sizing on both axes and transitions only the arrival wrapper", () => {
    const gate = [...css.matchAll(/\.chat-arrival-gate\s*\{[^}]*\}/g)].map(match => match[0]).join("\n");
    expect(gate).toBeDefined();
    expect(gate).toMatch(/flex:\s*1;/);
    expect(gate).toMatch(/min-width:\s*0;/);
    expect(gate).toMatch(/min-height:\s*0;/);
    expect(gate).toMatch(/display:\s*flex;/);
    expect(gate).toMatch(/flex-direction:\s*row;/);
    expect(gate).toMatch(/transition:\s*opacity 120ms ease-out;/);
    for (const body of css.matchAll(/\.chat-body\s*\{[^}]*\}/g)) {
      expect(body[0]).not.toMatch(/transition:/);
    }
    expect(css).toMatch(/@media\s*\(prefers-reduced-motion: reduce\)\s*\{\s*\.chat-arrival-gate\s*\{\s*transition: none;/);
  });
});

/**
 * Ticket 58 — the chat-switch arrival. The desktop's switch is atomic
 * (`select_chat`, state.rs:1740-1792: rows re-derived and the restored
 * viewport applied in ONE frame; shell.rs:1837-1862 snaps the pane tweens;
 * composer.rs:5849-5874 ROUTE_SNAPS the morph): a switch renders its
 * destination state, and motion belongs to live streams. The web's ONE
 * predicate (`ChatArrivalWindow`) carries that contract — armed at the
 * switch remount, cleared once the measurement cascade quiesces — and every
 * arrival gate consults it: the scroller's spring, the tool groups' fold
 * tweens, the shimmer arming.
 */

const parse = (_key: string, text: string, live: boolean) => parseMarkdown(text, live);

function entry(id: string, parts: MessagePart[], fields: Partial<SessionMessageEntry> = {}): SessionMessageEntry {
  return { id, role: "assistant", parts, createdAt: 1758000000000, deviceId: "dev", status: null, ...fields };
}

function toolPart(id: string, call: ToolCall): MessagePart {
  return { kind: "tool", id, call, isError: false, resolved: true };
}

function exec(command: string): ToolCall {
  return { kind: "exec", command };
}

/** One collapsible tool-group row, built through the real row model. */
function toolGroupRow(entryId: string, commands: string[]): TranscriptRow {
  const e = entry(entryId, commands.map((command, ix) => toolPart(`c${ix}`, exec(command))));
  const rows = rowsForEntry(e, { parse });
  const group = rows.find((row) => row.rowKind.kind === "toolGroup");
  if (group === undefined) {
    throw new Error("expected a tool group row");
  }
  return group;
}

/** A minimal scroller: only the fields the controller reads and writes. */
type MutableScroller = HTMLElement & {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  addEventListener: () => void;
  removeEventListener: () => void;
  parentElement: null;
  /** Deliver an event the controller subscribed (the user-scroll path). */
  fire(type: string): void;
};

function fakeScroller(scrollHeight: number, clientHeight: number): MutableScroller {
  const listeners = new Map<string, Set<() => void>>();
  const el = {
    scrollTop: 0,
    scrollHeight,
    clientHeight,
    addEventListener: (type: string, listener: () => void) => {
      const set = listeners.get(type) ?? new Set();
      set.add(listener);
      listeners.set(type, set);
    },
    removeEventListener: (type: string, listener: () => void) => {
      listeners.get(type)?.delete(listener);
    },
    parentElement: null,
    fire: (type: string) => {
      for (const listener of [...(listeners.get(type) ?? [])]) {
        listener();
      }
    },
  };
  return el as unknown as MutableScroller;
}

describe("ChatArrivalWindow (ticket 58 — the ONE arrival predicate)", () => {
  it("same-chat frames are not arrival; the switch arms the window", () => {
    const w = new ChatArrivalWindow();
    expect(w.isArrival(0)).toBe(false);
    expect(w.isArrival(999)).toBe(false);
    w.arm(1000);
    // Before the arm is an ordinary same-chat frame.
    expect(w.isArrival(999)).toBe(false);
    expect(w.isArrival(1000)).toBe(true);
    // Waiting for the first measure: the hard cap is the only bound.
    expect(w.isArrival(1000 + ARRIVAL_HARD_CAP_MS)).toBe(true);
    expect(w.isArrival(1000 + ARRIVAL_HARD_CAP_MS + 1)).toBe(false);
  });

  it("measurement batches extend the window; quiet frames close it", () => {
    const w = new ChatArrivalWindow();
    w.arm(1000);
    w.noteMeasure(1033);
    expect(w.isArrival(1033)).toBe(true);
    expect(w.isArrival(1033 + ARRIVAL_QUIESCE_MS)).toBe(true);
    expect(w.isArrival(1033 + ARRIVAL_QUIESCE_MS + 1)).toBe(false);
    // A later batch re-extends the quiesce deadline; the cap still bounds it.
    w.noteMeasure(1400);
    expect(w.isArrival(1400 + ARRIVAL_QUIESCE_MS)).toBe(true);
    expect(w.isArrival(1401 + ARRIVAL_HARD_CAP_MS)).toBe(false);
  });

  it("measures outside an armed window are inert; a later arm supersedes it", () => {
    const w = new ChatArrivalWindow();
    w.noteMeasure(500);
    expect(w.isArrival(500)).toBe(false);
    w.arm(1000);
    w.noteMeasure(1010);
    w.arm(2000);
    expect(w.isArrival(1999)).toBe(false);
    expect(w.isArrival(2000)).toBe(true);
  });
});

describe("StickController arrival (ticket 58 — no spring on a chat switch)", () => {
  it("chat_switch_arrival_restores_in_one_assignment_and_schedules_no_spring", () => {
    const raf = vi.fn(() => 1);
    vi.stubGlobal("requestAnimationFrame", raf);
    try {
      const arrival = new ChatArrivalWindow();
      const el = fakeScroller(4000, 600);
      const stick = new StickController({ onJumpVisibility: () => {}, arrival });
      stick.attach(el);

      // The switch lands: the first fill writes the end instantly — one
      // assignment, estimates and all.
      arrival.arm(performance.now());
      stick.snapToEnd();
      expect(el.scrollTop).toBe(3400);
      expect(raf).not.toHaveBeenCalled();

      // The settle cascade's measurement batches land; each per-commit kick
      // writes the CURRENT end directly — the viewport lands and stays, the
      // spring queue stays empty (no "scrolling down" glide).
      el.scrollHeight = 5200;
      stick.kick();
      expect(el.scrollTop).toBe(4600);
      el.scrollHeight = 6100;
      stick.kick();
      expect(el.scrollTop).toBe(5500);
      expect(raf).not.toHaveBeenCalled();

      // The window closed (cap passed): ordinary growth owns the spring again.
      arrival.arm(performance.now() - ARRIVAL_HARD_CAP_MS - 10);
      el.scrollHeight = 9000;
      stick.kick();
      expect(raf).toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("an anchored restore is ONE hard assignment — no poll frames, no spring", () => {
    const raf = vi.fn(() => 1);
    vi.stubGlobal("requestAnimationFrame", raf);
    try {
      const arrival = new ChatArrivalWindow();
      const el = fakeScroller(4000, 600);
      const stick = new StickController({ onJumpVisibility: () => {}, arrival });
      stick.attach(el);
      arrival.arm(performance.now());
      // The saved viewport is applied as a single clamped scrollTop write.
      stick.restoreViewport(1500, null, 1900);
      expect(el.scrollTop).toBe(1500);
      expect(stick.pinned).toBe(false);
      expect(raf).not.toHaveBeenCalled();
      // The post-measure correction is the per-commit anchor preserve — a
      // write, never a tween.
      stick.writePreserving(1525);
      expect(el.scrollTop).toBe(1525);
      expect(raf).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("a same-chat reset's kick neither re-engages an escaped pin nor moves the viewport", () => {
    const raf = vi.fn(() => 1);
    vi.stubGlobal("requestAnimationFrame", raf);
    try {
      const arrival = new ChatArrivalWindow();
      const el = fakeScroller(4000, 600);
      const stick = new StickController({ onJumpVisibility: () => {}, arrival });
      stick.attach(el);
      // The switch's restore landed; the user then escaped upward (a real
      // scroll event, not a controller write).
      arrival.arm(performance.now());
      stick.snapToEnd();
      expect(el.scrollTop).toBe(3400);
      el.scrollTop = 1000;
      el.fire("scroll");
      expect(stick.pinned).toBe(false);

      // Long past the hard cap, a same-chat reset swaps the content and the
      // commit kicks: the escaped viewport is NOT re-pinned and NOT moved —
      // the spring path is scheduled but owns nothing while unpinned.
      arrival.arm(performance.now() - ARRIVAL_HARD_CAP_MS - 10);
      el.scrollHeight = 5200;
      stick.kick();
      expect(raf).toHaveBeenCalled();
      expect(el.scrollTop).toBe(1000);
      expect(stick.pinned).toBe(false);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("tool-group arrival gates (ticket 58 — folds and shimmer)", () => {
  it("no fold tween and no shimmer on the arrival frame; live flips animate", () => {
    const arrival = new ChatArrivalWindow();
    const motion = new ToolGroupMotionStore(arrival);
    const row = toolGroupRow("tools", ["pwd"]);

    // The switch lands: the baseline sync re-seeds the reveals WITHOUT
    // arming the shimmer (an already-loaded transcript's first paint).
    arrival.arm(performance.now());
    motion.sync([row], true);
    expect(motion.revealOf(row.id)!.shimmerStartedAt).toBeNull();

    // The rendered-open flip mid-arrival (the streaming-status settle/desync
    // flap — the "try to close the opened group tabs" report) records its
    // endpoint WITHOUT seeding a close tween: the group renders its final
    // fold state.
    motion.noteRendered(row.id, true, 120);
    motion.noteRendered(row.id, false, 120);
    expect(motion.groupFold(row.id)).toBeNull();

    // The window closed — same chat, live: a rendered-open flip seeds the
    // tween again, and a live sync arms the shimmer for live content.
    arrival.arm(performance.now() - ARRIVAL_HARD_CAP_MS - 10);
    motion.noteRendered(row.id, true, 120);
    motion.noteRendered(row.id, false, 120);
    expect(motion.groupFold(row.id)?.toggledAt).not.toBeNull();
    motion.sync([row], false);
    expect(motion.revealOf(row.id)!.shimmerStartedAt).not.toBeNull();
  });

  it("a store without the window keeps ticket 40's semantics unchanged", () => {
    const motion = new ToolGroupMotionStore();
    const row = toolGroupRow("tools", ["pwd"]);
    motion.sync([row], true);
    expect(motion.revealOf(row.id)!.shimmerStartedAt).not.toBeNull();
    motion.noteRendered(row.id, false, 0);
    expect(motion.revealOf(row.id)!.renderedOpen).toBe(false);
    // A same-chat flip seeds the tween with no window to consult.
    motion.noteRendered(row.id, true, 34);
    expect(motion.groupFold(row.id)?.toggledAt).not.toBeNull();
  });

  it("a thought completion during the arrival window records, never animates (ticket 71)", () => {
    // The animated thought close (ticket 71 B) must never impersonate a
    // completion on the switch's settle frames: the store seeds its close
    // tween only for a LIVE unresolved→resolved flip, and the armed
    // arrival window suppresses the tween exactly like the group flip's
    // gate above.
    const arrival = new ChatArrivalWindow();
    const motion = new ToolGroupMotionStore(arrival);
    arrival.arm(performance.now());
    motion.sync([thoughtGroupRow("think", "thinking hard", true)], false);
    motion.noteDetailRendered("think#g0#d0", 312);
    // The completion lands while the window still covers the settle
    // cascade: the chip renders its closed endpoint without a tween.
    motion.sync([thoughtGroupRow("think", "thinking hard", false)], false);
    expect(motion.detailFold("think#g0#d0")).toBeNull();
    // The window closed — the same flip on a live frame seeds the animated
    // close (the completion is genuine now, not a restore artifact).
    arrival.arm(performance.now() - ARRIVAL_HARD_CAP_MS - 10);
    motion.sync([thoughtGroupRow("think2", "thinking again", true)], false);
    motion.noteDetailRendered("think2#g0#d0", 312);
    motion.sync([thoughtGroupRow("think2", "thinking again", false)], false);
    expect(motion.detailFold("think2#g0#d0")?.toggledAt).not.toBeNull();
  });
});

/** A single thought-chip group row through the real row model (ticket 71). */
function thoughtGroupRow(entryId: string, text: string, streaming: boolean): TranscriptRow {
  const e: SessionMessageEntry = {
    id: entryId,
    role: "assistant",
    parts: [{ kind: "reasoning", id: "r0", text }],
    createdAt: 1758000000000,
    deviceId: "dev",
    status: streaming ? "streaming" : null,
  };
  const rows = rowsForEntry(e, { parse });
  const group = rows.find((row) => row.rowKind.kind === "toolGroup");
  if (group === undefined) {
    throw new Error("expected a tool group row");
  }
  return group;
}
