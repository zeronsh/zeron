import { describe, expect, it, afterEach } from "vitest";
import { readFileSync } from "node:fs";
import { sidebarTweenSignal } from "../src/lib/sidebar-tween";
import {
  IDENTITY_FREEZE_ATTR,
  IDENTITY_FREEZE_WIDTH_VAR,
  IdentityFreezeGate,
  applyIdentityFreeze,
  installIdentityFreeze,
  type IdentityFreezeTarget,
} from "../src/lib/identity-freeze";

/**
 * The identity freeze's contract (ticket 63), the same layers the titlebar
 * wires: the GATE (the arm→settle window's only inputs are its edges, so
 * the clamp cannot move between them), the DOM half (one read + one write
 * at each edge, the flag and the clamp applied together), and the wiring
 * against 57a's REAL signal — every settle path funnels through
 * `settle()`, so the freeze rides whichever lands first. The window
 * simulation is node-env-honest: a recording stand-in for the identity
 * element whose box read returns the slide the row would inflict per
 * frame, proving the freeze consumes exactly the FIRST (pre-tween) sample.
 */

afterEach(() => {
  sidebarTweenSignal.settle();
});

/**
 * A recording stand-in for the identity element (the DOM half's target).
 * Each `getBoundingClientRect` pops the next width — the sequence the
 * row's sliding free space would produce if the identity kept
 * re-participating in the shrink distribution.
 */
function recordingIdentity(widths: readonly number[]): {
  readonly target: IdentityFreezeTarget;
  readonly reads: () => number;
  readonly ops: string[];
} {
  let reads = 0;
  const ops: string[] = [];
  const target: IdentityFreezeTarget = {
    style: {
      setProperty: (name, value) => {
        ops.push(`${name}=${value}`);
      },
      removeProperty: (name) => {
        ops.push(`-${name}`);
      },
    },
    setAttribute: (name, value) => {
      ops.push(value === "" ? `@${name}` : `@${name}=${value}`);
    },
    removeAttribute: (name) => {
      ops.push(`@-${name}`);
    },
    getBoundingClientRect: () => {
      const width = widths[Math.min(reads, widths.length - 1)] ?? 0;
      reads += 1;
      return { width };
    },
  };
  return { target, reads: () => reads, ops };
}

describe("IdentityFreezeGate — the window's edges are the only inputs", () => {
  it("released by default; arm captures; settle lifts", () => {
    const gate = new IdentityFreezeGate();
    expect(gate.maxWidth()).toBe(null);
    gate.arm(431.25);
    expect(gate.maxWidth()).toBe(431.25);
    gate.settle();
    expect(gate.maxWidth()).toBe(null);
  });

  it("a mid-glide reversal re-arms and re-captures the painted box", () => {
    const gate = new IdentityFreezeGate();
    gate.arm(431.25);
    // The reversal's capture reads the PINNED box — the same value — which
    // is the signal's own retarget-from-painted semantics.
    gate.arm(431.25);
    expect(gate.maxWidth()).toBe(431.25);
    gate.settle();
    expect(gate.maxWidth()).toBe(null);
  });
});

describe("applyIdentityFreeze — the pin and its release", () => {
  it("writes the flag and the clamp together, clears them together", () => {
    const { target, ops } = recordingIdentity([500]);
    applyIdentityFreeze(target, 500);
    expect(ops).toEqual([`${IDENTITY_FREEZE_WIDTH_VAR}=500px`, `@${IDENTITY_FREEZE_ATTR}`]);
    applyIdentityFreeze(target, null);
    expect(ops).toEqual([
      `${IDENTITY_FREEZE_WIDTH_VAR}=500px`,
      `@${IDENTITY_FREEZE_ATTR}`,
      `-${IDENTITY_FREEZE_WIDTH_VAR}`,
      `@-${IDENTITY_FREEZE_ATTR}`,
    ]);
  });
});

describe("installIdentityFreeze — 57a's window gates the clamp", () => {
  it("arm captures the pre-tween width once; the window adds no reads or writes; settle clears", () => {
    // The slide the row would inflict per frame of the 200ms: the identity
    // would re-truncate through every one of these widths.
    const slide = [431.25, 421.5, 402.25, 380, 364.5, 356];
    const identity = recordingIdentity(slide);
    const uninstall = installIdentityFreeze(sidebarTweenSignal, () => identity.target);
    expect(identity.ops).toEqual([]);
    // The flip commit arms the tween; the capture lands inside its layout
    // effects, while the row's transitions still sit at their pre-tween
    // values — the FIRST sample.
    sidebarTweenSignal.arm();
    expect(identity.reads()).toBe(1);
    const written = identity.ops.filter((op) => op.startsWith(`${IDENTITY_FREEZE_WIDTH_VAR}=`));
    expect(written).toEqual([`${IDENTITY_FREEZE_WIDTH_VAR}=431.25px`]);
    expect(identity.ops).toContain(`@${IDENTITY_FREEZE_ATTR}`);
    // The whole window: the clamp holds the arm-time capture — no further
    // reads (the freeze never polls), no further writes (the max-width is
    // constant for the window's duration), so none of the later slide
    // values can reach the element.
    for (const width of slide.slice(1)) {
      expect(identity.ops).not.toContain(`${IDENTITY_FREEZE_WIDTH_VAR}=${width}px`);
    }
    // transitionend: exactly one release pair, appended after the arm pair.
    sidebarTweenSignal.settle();
    expect(identity.ops).toEqual([
      `${IDENTITY_FREEZE_WIDTH_VAR}=431.25px`,
      `@${IDENTITY_FREEZE_ATTR}`,
      `-${IDENTITY_FREEZE_WIDTH_VAR}`,
      `@-${IDENTITY_FREEZE_ATTR}`,
    ]);
    expect(identity.reads()).toBe(1);
    uninstall();
  });

  it("every settle path funnels through the signal — the freeze releases with whichever lands", () => {
    const identity = recordingIdentity([500]);
    const uninstall = installIdentityFreeze(sidebarTweenSignal, () => identity.target);
    sidebarTweenSignal.arm();
    // The cap, a drag disarm, a reduce flip: all call settle(); the
    // idempotent double-settle is the cap racing the event.
    sidebarTweenSignal.settle();
    sidebarTweenSignal.settle();
    expect(identity.ops[identity.ops.length - 1]).toBe(`@-${IDENTITY_FREEZE_ATTR}`);
    expect(identity.ops[identity.ops.length - 2]).toBe(`-${IDENTITY_FREEZE_WIDTH_VAR}`);
    uninstall();
  });

  it("a fresh window captures the SETTLED geometry, not the stale clamp", () => {
    const identity = recordingIdentity([431.25, 356]);
    const uninstall = installIdentityFreeze(sidebarTweenSignal, () => identity.target);
    sidebarTweenSignal.arm();
    sidebarTweenSignal.settle();
    // The next flip: the box re-measures at the (new) settled layout —
    // 356, not the previous window's 431.25.
    sidebarTweenSignal.arm();
    expect(identity.ops).toContain(`${IDENTITY_FREEZE_WIDTH_VAR}=356px`);
    sidebarTweenSignal.settle();
    uninstall();
  });

  it("a missing element (takeover unmounted the identity) is a no-op both ways", () => {
    const uninstall = installIdentityFreeze(sidebarTweenSignal, () => null);
    sidebarTweenSignal.arm();
    sidebarTweenSignal.settle();
    uninstall();
  });

  it("the unsubscribe detaches the rider from the window", () => {
    const identity = recordingIdentity([500]);
    const uninstall = installIdentityFreeze(sidebarTweenSignal, () => identity.target);
    uninstall();
    sidebarTweenSignal.arm();
    sidebarTweenSignal.settle();
    expect(identity.ops).toEqual([]);
    expect(identity.reads()).toBe(0);
  });
});

describe("the CSS artifact (the clamp the browser actually runs)", () => {
  const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");
  const source = readFileSync(new URL("../src/components/titlebar.tsx", import.meta.url), "utf8");

  it("the frozen identity opts out of the row's shrink and clamps at the captured width", () => {
    const block = css.match(/\.titlebar-identity\[data-rb-identity-frozen\]\s*\{[^}]*\}/)?.[0];
    expect(block).toBeDefined();
    expect(block).toMatch(/flex:\s*none;/);
    expect(block).toMatch(/max-width:\s*var\(--rb-identity-freeze-width/);
  });

  it("the titlebar installs the freeze on 57a's signal — no forked tween predicate", () => {
    expect(source).toMatch(/installIdentityFreeze\(sidebarTweenSignal/);
    expect(source).not.toMatch(/dockGlideSignal/);
  });
});
