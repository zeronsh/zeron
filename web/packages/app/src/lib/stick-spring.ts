/**
 * The transcript's stick-to-bottom spring — a line-for-line port of the
 * desktop's `StickSpring` (`crates/ui/src/transcript.rs`, mugen §1e) running
 * on the shared constants from `@zeron/theme`'s motion catalog (generated
 * from `zeron_proto::motion`). The DOM driver lives in
 * `../components/transcript.tsx`; everything here is pure.
 */

import { motion } from "@zeron/theme";

const spring = motion.stickSpring;

export const SPRING_DAMPING = spring.damping;
export const SPRING_STIFFNESS = spring.stiffness;
export const SPRING_MASS = spring.mass;
export const SPRING_FRAME_MS = spring.frameMs;
export const SPRING_MAX_CATCHUP_FRAMES = spring.maxCatchupFrames;
export const SPRING_GROWTH_EMA = spring.growthEma;
export const SPRING_CHASE_MAX_LEAD = spring.chaseMaxLeadPx;
export const AT_BOTTOM_PX = spring.atBottomPx;
export const STICK_THRESHOLD_PX = spring.stickThresholdPx;
export const SPRING_SETTLE_GRACE_MS = spring.settleGraceMs;
export const GLIDE_MAX_VIEWPORTS = spring.glideMaxViewports;

/** List overdraw beyond the viewport (transcript.rs OVERDRAW_PX). */
export const OVERDRAW_PX = 320;
/** Show the scroll-to-bottom button beyond this distance from the end. */
export const SCROLL_BUTTON_THRESHOLD_PX = 320;

/**
 * Jump-button hysteresis: once offered, keep the control until close to the
 * end — a single 320px threshold made it disappear halfway through a downward
 * scroll gesture (transcript.rs `jump_visibility`).
 */
export function jumpVisibility(wasShown: boolean, distance: number): boolean {
  return distance > (wasShown ? AT_BOTTOM_PX : SCROLL_BUTTON_THRESHOLD_PX);
}

/**
 * `jump_button_shown` (transcript.rs:3198, :3743-3745): the pill follows
 * `jumpVisibility`'s hysteresis, hidden while pinned (so a settling bottom
 * spring never flashes it) and while an own-turn hold is live
 * (transcript.rs:3174-3175 — the scroll handler's own-turn branch carries
 * that half inline).
 */
export function jumpButtonShown(
  jumpShown: boolean,
  distance: number,
  pinned: boolean,
  ownTurnHeld: boolean,
): boolean {
  return jumpVisibility(jumpShown, distance) && !pinned && !ownTurnHeld;
}

/**
 * Direction-aware re-stick: scrolling away from the bottom never re-sticks,
 * even inside the band (a 20px wheel notch from the pinned bottom must break
 * the pin); returning toward it re-engages once inside the 70px band.
 */
export function shouldRestick(distance: number, previousDistance: number): boolean {
  return distance <= STICK_THRESHOLD_PX && distance < previousDistance;
}

/**
 * Whether a not-ours scroll moving the viewport breaks the pin — the
 * desktop's escape rule (transcript.rs:3182-3189): a scroll whose distance
 * grew by more than 1px while already past the stick band is user input
 * moving away from the bottom. Content growth never fires the handler on the
 * desktop, so `previousDistance` is always a USER-scroll baseline there; the
 * web controller must refresh the baseline on every content kick to keep
 * that meaning (growth between two user scrolls would otherwise read as the
 * second scroll's intent — the trap the desktop's own-turn stepper documents
 * at transcript.rs:3534-3540).
 */
export function shouldBreakPin(distance: number, previousDistance: number): boolean {
  return distance > previousDistance + 1 && distance > AT_BOTTOM_PX;
}

/**
 * A live stream already resting at the end keeps that end anchored as its
 * measured height grows — deliberately narrower than `pinned`: users gliding
 * back toward the bottom keep the normal spring behavior.
 */
export function shouldAnchorLiveStream(pinned: boolean, distanceFromBottom: number, streaming: boolean): boolean {
  return pinned && streaming && distanceFromBottom <= AT_BOTTOM_PX;
}

/**
 * Pure stick-to-bottom spring stepper — the mugen `tick()` integration:
 * velocity relaxes toward `(damping·v + stiffness·diff)/mass` per 60fps
 * sub-frame, position advances by `v + targetVel` where `targetVel` is a
 * feed-forward EMA of target growth px/frame, and the chase point sits up to
 * `SPRING_CHASE_MAX_LEAD` px above the true bottom proportional to growth.
 */
export class StickSpring {
  /** Spring velocity, px per 60fps frame. */
  #velocity = 0;
  /** Feed-forward: smoothed target growth, px per 60fps frame. */
  #targetVel = 0;
  /** Target observed at the previous tick (null = fresh/parked). */
  #lastTarget: number | null = null;

  /** Park the spring (drops all state; the next tick starts cold). */
  reset(): void {
    this.#velocity = 0;
    this.#targetVel = 0;
    this.#lastTarget = null;
  }

  /** Residual motion below mugen's settle thresholds (`v < .05 && targetVel < .05`)? */
  isIdle(): boolean {
    return this.#velocity < 0.05 && this.#targetVel < 0.05;
  }

  static needsFrame(distance: number): boolean {
    // The spring is clamped to the target. Residual velocity cannot move a
    // viewport already there; virtual-list height estimates can keep that
    // velocity nonzero indefinitely even after a turn completes.
    return distance > 0.5;
  }

  /** Test-only view of the feed-forward estimate. */
  targetVel(): number {
    return this.#targetVel;
  }

  /**
   * Advance one tick. `pos`/`target` are scroll offsets in px (larger =
   * closer to the bottom); `frames` is elapsed time in 60fps frames (clamped
   * by the caller to `SPRING_MAX_CATCHUP_FRAMES`). Returns the new position:
   * never overshoots `target`, monotone while approaching, snaps exactly once
   * within 0.5px.
   */
  step(pos: number, target: number, frames: number): number {
    let remaining = frames;
    const grew = this.#lastTarget === null ? 0 : target - this.#lastTarget;
    this.#lastTarget = target;
    if (grew < -1) {
      // Target shrank (row collapse/removal) — growth estimate is stale.
      this.#targetVel = 0;
    } else {
      const observed = Math.max(0, grew) / Math.max(remaining, 0.25);
      this.#targetVel += SPRING_GROWTH_EMA * (observed - this.#targetVel);
    }
    const chase = target - Math.min(this.#targetVel * 9, SPRING_CHASE_MAX_LEAD);
    let v = this.#velocity;
    let position = pos;
    while (remaining > 0) {
      const h = Math.min(remaining, 1);
      remaining -= h;
      const diff = Math.max(chase - position, 0);
      v += h * ((SPRING_DAMPING * v + SPRING_STIFFNESS * diff) / SPRING_MASS - v);
      position = Math.min(position + (v + this.#targetVel) * h, target);
    }
    this.#velocity = v;
    return target - position <= 0.5 ? target : position;
  }
}
