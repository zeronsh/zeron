/**
 * The titlebar identity's freeze (ticket 63) — the boundary-work rider on
 * 57a's sidebar tween. The desktop keeps the titlebar row's free space
 * INVARIANT through a collapse: one `sidebar_now(t)` scalar drives the row
 * inset, the pane band and the pane column per frame, so their deltas
 * cancel (shell.rs:1946-1962, :3814-3826; tabs.rs:179-253). The web's
 * CSS-managed tween unified the CLOCK — one flip commit, the browser
 * interpolates, zero React frames — but not the INPUTS:
 * `--rb-titlebar-row-left` and `--rb-pane-band` are endpoint-computed from
 * the FLIPPED sidebar (app-shell.tsx), and while both transitions ride the
 * same resize curve, their endpoint deltas do not cancel when the pane is
 * width-clamped (`resolvePaneWidth` loosens by up to the sidebar's width;
 * the row inset retreats by the sidebar less the content-start floor). The
 * row's free space slides through the 200ms, and the identity — the row's
 * only shrinkable child (`.titlebar-identity`, min-width 0) — re-truncates
 * its title/folder frame by frame.
 *
 * The freeze pins the box instead of porting the per-frame clock (which 57a
 * deliberately removed): at ARM — the flip commit's layout effects, while
 * the row's transitions still sit at their pre-tween values — the
 * identity's laid-out width is captured and written as a `max-width`
 * clamp; at SETTLE the clamp lifts and the truncation re-evaluates exactly
 * once, the snap the ticket sanctions. The window is 57a's own signal
 * (`sidebarTweenSignal.subscribe`) — no forked predicate — and the dock
 * glide never moves the row (its channels are the conversation column's
 * opacity/rise/width set), so the sidebar tween is the only window. One
 * read and one write per edge, zero work per frame.
 *
 * The gate is pure (no DOM) so the unit tests drive a whole window with a
 * recording stand-in for the element; `Titlebar` installs the wiring at
 * its mount with the identity element's ref.
 */

/** The stylesheet hooks (app.css's `.titlebar-identity` block): the flag and the captured clamp. */
export const IDENTITY_FREEZE_ATTR = "data-rb-identity-frozen";
export const IDENTITY_FREEZE_WIDTH_VAR = "--rb-identity-freeze-width";

/**
 * The freeze's gate — the window's EDGES are its only inputs, so the clamp
 * cannot move between them (nothing re-reads the box mid-tween; that is
 * the invariant). `arm` captures the box as laid out at the flip; a
 * mid-glide reversal re-arms and re-captures from the painted box — the
 * signal's own retarget semantics — which is the same value, because the
 * box is pinned. `settle` lifts the clamp and the truncation re-evaluates
 * once.
 */
export class IdentityFreezeGate {
  #clamp: number | null = null;

  /** The tween armed: capture the box as it is laid out NOW (pre-tween). */
  arm(renderedWidth: number): void {
    this.#clamp = Math.max(0, renderedWidth);
  }

  /** The tween settled: lift the clamp. */
  settle(): void {
    this.#clamp = null;
  }

  /** The clamped max-width while the window is open; null when shut. */
  maxWidth(): number | null {
    return this.#clamp;
  }
}

/**
 * The DOM half's target — satisfied structurally by the identity element
 * (and by the tests' recording stand-in): the custom-property pair that
 * carries the clamp, the attribute pair that carries the flag, and the box
 * read the capture performs exactly once per window.
 */
export interface IdentityFreezeTarget {
  readonly style: {
    setProperty(name: string, value: string): void;
    removeProperty(name: string): void;
  };
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
  getBoundingClientRect(): { readonly width: number };
}

/**
 * Pin the target's box at `maxWidth` (the flag and the clamp together), or
 * unpin on null. The stylesheet turns the pair into `flex: none` plus the
 * captured `max-width`, which reproduces the pre-tween box exactly —
 * truncated or not — and opts the identity out of the row's shrink
 * distribution for the window.
 */
export function applyIdentityFreeze(
  target: IdentityFreezeTarget,
  maxWidth: number | null,
): void {
  if (maxWidth === null) {
    target.style.removeProperty(IDENTITY_FREEZE_WIDTH_VAR);
    target.removeAttribute(IDENTITY_FREEZE_ATTR);
    return;
  }
  target.style.setProperty(IDENTITY_FREEZE_WIDTH_VAR, `${maxWidth}px`);
  target.setAttribute(IDENTITY_FREEZE_ATTR, "");
}

/**
 * Wire the freeze to 57a's signal: the subscription is the ONLY
 * integration point, because every settle path — `transitionend`, the
 * settle cap, the seam-drag and reduce disarms — funnels through
 * `sidebarTweenSignal.settle()`, so the freeze releases with whichever
 * lands first. Arm captures and pins; settle lifts. `find` re-resolves the
 * element per edge (the identity unmounts while a takeover owns the row);
 * a missing element is a no-op. Returns the unsubscribe.
 */
export function installIdentityFreeze(
  signal: { subscribe(listener: (active: boolean) => void): () => void },
  find: () => IdentityFreezeTarget | null,
): () => void {
  const gate = new IdentityFreezeGate();
  return signal.subscribe((active) => {
    const target = find();
    if (!active) {
      gate.settle();
      if (target !== null) {
        applyIdentityFreeze(target, null);
      }
      return;
    }
    if (target === null) {
      return;
    }
    gate.arm(target.getBoundingClientRect().width);
    applyIdentityFreeze(target, gate.maxWidth());
  });
}
