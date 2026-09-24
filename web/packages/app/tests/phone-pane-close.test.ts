import { describe, expect, it } from "vitest";
import {
  armsExpandedCloseHold,
  releasesExpandedCloseHold,
  type PhonePaneSnapshot,
} from "../src/components/right-pane";
import { RightPaneStore } from "../src/state/right-pane";

/**
 * Ticket 72 — the phone drawer's expanded-close width hold (research
 * §3.72/§4.72/§5.72): an expanded close must slide away at the 100vw the
 * user was viewing while the logical flags reset immediately.
 *
 * Like the app-shell-drawer suite, this runs in vitest's node environment
 * with no DOM and no mount harness (nothing in the repo renders components
 * in tests), so the coverage lands on the hold's pure decision core —
 * `armsExpandedCloseHold` / `releasesExpandedCloseHold`, the functions
 * `usePhoneExpandedCloseHold`'s commit step is made of — with `snap(...)`
 * standing in for the rendered commit. The wiring around them (the
 * `right-pane-closing-expanded` class on the aside, the guarded
 * `transitionend`/`transitioncancel` listener, the reduced-motion flip
 * listener and the MENU_IN + 100ms fallback clock) is typechecked by
 * `tsc --noEmit` in `pnpm -r build`; actual transition frames are the
 * ticket §2.5 runtime matrix.
 */

/** A rendered commit's snapshot: phone portrait, chat-1's pane, open normal. */
function snap(partial: Partial<PhonePaneSnapshot>): PhonePaneSnapshot {
  return { owner: "chat-1", phone: true, open: true, expanded: false, ...partial };
}

/**
 * The hook's commit step, folded: arm on the expanded-open → closed edge,
 * release on reopen / owner switch / breakpoint flip. `null` is no hold;
 * otherwise the hold carries its pane owner. (Settle — transitionend,
 * transitioncancel, the reduced-motion flip or the fallback clock — is the
 * component clearing the hold outright; there is no decision in it.)
 */
function drive(
  steps: readonly { was: PhonePaneSnapshot; now: PhonePaneSnapshot; reducedMotion?: boolean }[],
): string | null {
  let hold: string | null = null;
  for (const { was, now, reducedMotion = false } of steps) {
    if (hold !== null && releasesExpandedCloseHold(hold, now)) {
      hold = null;
    }
    if (hold === null && armsExpandedCloseHold(was, now, reducedMotion)) {
      hold = now.owner;
    }
  }
  return hold;
}

describe("expanded close retains presentation width while resetting logical mode", () => {
  it("the store flags clear in the close commit while the hold arms on the same edge", () => {
    // The REAL store drives the logical half: open expanded, then close.
    const store = new RightPaneStore(null);
    store.setSurfacesOpen("chat-1", true);
    store.toggleExpanded("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: true, expanded: true });

    const was = snap({ open: true, expanded: true });
    store.close("chat-1");
    const now = snap({ open: false, expanded: false });

    // Logical mode resets immediately (the presentation hold must not
    // delay it — §5's first rule), independently asserted from…
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, expanded: false });
    // …the presentation half: this same commit arms the width hold, keyed
    // to the pane's owner, so the drawer keeps its expanded width class
    // through the slide.
    expect(armsExpandedCloseHold(was, now, false)).toBe(true);
    expect(now.owner).toBe("chat-1");
    // And while the close runs uninterrupted — same owner, still phone,
    // still closed — no commit releases it early; only the settle does.
    expect(releasesExpandedCloseHold("chat-1", now)).toBe(false);
    expect(releasesExpandedCloseHold("chat-1", snap({ open: false }))).toBe(false);
  });

  it("a normal-width close never arms the hold", () => {
    expect(drive([{ was: snap({}), now: snap({ open: false }) }])).toBeNull();
  });

  it("a desktop close never arms the hold — the width glide owns continuity there", () => {
    const was = snap({ phone: false, expanded: true });
    const now = snap({ phone: false, open: false });
    expect(armsExpandedCloseHold(was, now, false)).toBe(false);
    // And a mixed-layer edge (the flip commit itself) cannot arm either.
    expect(armsExpandedCloseHold(snap({ expanded: true }), now, false)).toBe(false);
    expect(armsExpandedCloseHold(was, snap({ open: false }), false)).toBe(false);
  });

  it("closed→closed and open→open commits never arm — the hold has no self-renewing edge", () => {
    expect(drive([{ was: snap({ open: false }), now: snap({ open: false }) }])).toBeNull();
    // A takeover flip while OPEN is not a close edge.
    expect(drive([{ was: snap({}), now: snap({ expanded: true }) }])).toBeNull();
    expect(drive([{ was: snap({ expanded: true }), now: snap({}) }])).toBeNull();
  });
});

describe("reopen owner change breakpoint and reduced motion cancel stale close completion", () => {
  const close = { was: snap({ open: true, expanded: true }), now: snap({ open: false }) };

  it("reopening mid-close releases the hold and the reopen never re-arms it", () => {
    // Close arms…
    expect(drive([close])).toBe("chat-1");
    // …and the reopen commit (normal mode — the flags were already reset)
    // releases it: the drawer retargets to the open presentation, no stale
    // completion narrows or hides it.
    const reopen = { was: snap({ open: false }), now: snap({}) };
    expect(drive([close, reopen])).toBeNull();
    expect(releasesExpandedCloseHold("chat-1", reopen.now)).toBe(true);
    expect(armsExpandedCloseHold(reopen.was, reopen.now, false)).toBe(false);
  });

  it("an owner switch cancels the old hold and honors the destination's state", () => {
    // The close was chat-1's; the next commit renders chat-2's pane
    // (closed, normal) — chat-1's hold must not follow the drawer across.
    const handoff = { was: close.now, now: snap({ owner: "chat-2", open: false }) };
    expect(drive([close, handoff])).toBeNull();
    expect(releasesExpandedCloseHold("chat-1", handoff.now)).toBe(true);
    // A close edge straddling two owners is not an edge at all.
    expect(
      armsExpandedCloseHold(snap({ owner: "chat-1", expanded: true }), snap({ owner: "chat-2", open: false }), false),
    ).toBe(false);
    // The destination's own destination state is untouched: chat-2 closed,
    // no hold.
    expect(armsExpandedCloseHold(handoff.was, handoff.now, false)).toBe(false);
  });

  it("crossing the 768px breakpoint mid-close releases the phone hold", () => {
    const flip = { was: close.now, now: snap({ phone: false, open: false }) };
    expect(drive([close, flip])).toBeNull();
    expect(releasesExpandedCloseHold("chat-1", flip.now)).toBe(true);
  });

  it("reduced motion arms nothing and a mid-close flip settles immediately", () => {
    // Preference on BEFORE the close: the CSS snaps the transform
    // (`transition: none` under reduce), so there are no frames to hold
    // through — arming would only leave a stale 100vw closed state.
    expect(drive([{ ...close, reducedMotion: true }])).toBeNull();
    // Flipped on MID-close the component's matchMedia listener clears the
    // hold outright (the same clear as a settle — the reduced-motion kill
    // ends the transition without an end event); afterwards a still-closed
    // commit re-arms nothing, so no deferred hold can reappear.
    expect(
      drive([
        { ...close, reducedMotion: true },
        { was: close.now, now: snap({ open: false }) },
      ]),
    ).toBeNull();
  });

  it("the full interruption set leaves no permanent hold: every path settles to null", () => {
    // close → settle (the component clears) is modeled by the absence of a
    // hold going into the next step; each interruption path below ends
    // with the release rule satisfied.
    for (const next of [
      snap({}), // reopen
      snap({ owner: "chat-2", open: false }), // owner switch
      snap({ phone: false, open: false }), // breakpoint flip
    ]) {
      expect(releasesExpandedCloseHold("chat-1", next)).toBe(true);
      expect(drive([close, { was: close.now, now: next }])).toBeNull();
    }
  });
});
