import { useCallback, useEffect, useRef } from "react";
import { Icon } from "@zeron/icons";
import { SidebarFadedLabel } from "./sidebar-faded-label";

/**
 * The sidebar's shared disclosure section — the desktop's
 * `sidebar_disclosure_header` / `render_sidebar_disclosure_body` /
 * `sidebar_disclosure_chevron` (shell/spaces.rs), used by BOTH the ByDevice
 * groups and the archived shelf.
 *
 * The height tween is interruptible with epochs, exactly like
 * `SidebarDisclosureMotion`: a second click mid-flight captures the current
 * interpolated height as the new `from`, so a rapid double-click reverses
 * smoothly instead of snapping. Height, opacity (`0.35 + 0.65 * reveal`),
 * the 3px upward creep, and the chevron's 0→90° rotation all ride the same
 * 180ms ease-out progress curve; `animating()` stays true for a 120ms grace
 * past the duration to absorb frame lag.
 */

/** `motion::COLLAPSE` — 180ms. */
export const SIDEBAR_DISCLOSURE_MS = 180;
/** `SIDEBAR_DISCLOSURE_TWEEN_GRACE` — render-thread lag absorption. */
export const SIDEBAR_DISCLOSURE_TWEEN_GRACE_MS = 120;
/** `motion::EASE_OUT` = cubic-bezier(0, 0, 0.58, 1). */
const EASE_OUT_CURVE: readonly [number, number, number, number] = [0, 0, 0.58, 1];

export interface DisclosureMotion {
  readonly epoch: number;
  readonly from: number;
  readonly to: number;
  /** `performance.now()` when the tween began. */
  readonly startedAt: number;
}

/*
 * In-flight tweens, keyed by disclosure key. Module-scoped like the
 * desktop's Shell field: the collapse flags are in-memory-only state, and a
 * section unmounting mid-flight (space filter narrowed) must not strand a
 * stale `from` for when it comes back.
 */
const motions = new Map<string, DisclosureMotion>();

const clamp01 = (value: number): number => Math.min(1, Math.max(0, value));

const lerp = (from: number, to: number, t: number): number => from + (to - from) * t;

/** Solve one point of a CSS cubic-bezier (as browsers evaluate transitions). */
function cubicBezier(
  curve: readonly [number, number, number, number],
  progress: number,
): number {
  const [x1, y1, x2, y2] = curve;
  if (progress <= 0) {
    return 0;
  }
  if (progress >= 1) {
    return 1;
  }
  // Newton on x(u) = progress, then y(u); falls back to bisection on stall.
  const x = (u: number): number => 3 * x1 * (1 - u) * (1 - u) * u + 3 * x2 * (1 - u) * u * u + u ** 3;
  const dx = (u: number): number =>
    3 * x1 * (1 - u) * (1 - 3 * u) + 3 * x2 * u * (2 - 3 * u) + 3 * u * u;
  const y = (u: number): number => 3 * y1 * (1 - u) * (1 - u) * u + 3 * y2 * (1 - u) * u * u + u ** 3;
  let u = progress;
  for (let i = 0; i < 8; i += 1) {
    const err = x(u) - progress;
    if (Math.abs(err) < 1e-6) {
      return y(u);
    }
    const slope = dx(u);
    if (Math.abs(slope) < 1e-6) {
      break;
    }
    u -= err / slope;
  }
  let low = 0;
  let high = 1;
  u = progress;
  for (let i = 0; i < 24; i += 1) {
    const err = x(u) - progress;
    if (Math.abs(err) < 1e-6) {
      break;
    }
    if (err > 0) {
      high = u;
    } else {
      low = u;
    }
    u = (low + high) / 2;
  }
  return y(u);
}

/** Eased progress of a tween at `now` (clamped, curve applied). Pure. */
function easedProgress(motion: DisclosureMotion, now: number): number {
  const raw = SIDEBAR_DISCLOSURE_MS > 0 ? (now - motion.startedAt) / SIDEBAR_DISCLOSURE_MS : 1;
  return cubicBezier(EASE_OUT_CURVE, clamp01(raw));
}

/**
 * The tween's current interpolated height — always lands exactly on `to`,
 * never overshoots (an over-aged tween reads as finished). Pure.
 */
export function disclosureCurrent(motion: DisclosureMotion, now: number): number {
  return lerp(motion.from, motion.to, easedProgress(motion, now));
}

/** True while the tween should still drive rendering (duration + grace). Pure. */
export function disclosureAnimating(motion: DisclosureMotion, now: number): boolean {
  return now - motion.startedAt < SIDEBAR_DISCLOSURE_MS + SIDEBAR_DISCLOSURE_TWEEN_GRACE_MS;
}

function prefersReducedMotion(): boolean {
  const query = (globalThis as { matchMedia?: (query: string) => { matches: boolean } }).matchMedia;
  if (query === undefined) {
    return false;
  }
  return query("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Owns one disclosure's open/close tween. Call `toggle()` from the header's
 * click BEFORE flipping `open` — it captures the in-flight height as the
 * new `from` and begins the tween toward the opposite end; the returned
 * refs carry the per-frame height/opacity/offset (body) and rotation
 * (chevron).
 */
export function useSidebarDisclosure(motionKey: string, open: boolean, fullHeight: number) {
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const chevronRef = useRef<HTMLSpanElement | null>(null);

  const toggle = useCallback((): boolean => {
    const now = performance.now();
    const previous = motions.get(motionKey);
    // `begin_sidebar_disclosure_motion`: capture the in-flight value, not
    // the resting height, so a rapid double-click reverses smoothly.
    const from =
      previous !== undefined && disclosureAnimating(previous, now)
        ? disclosureCurrent(previous, now)
        : open
          ? fullHeight
          : 0;
    const to = open ? 0 : fullHeight;
    if (prefersReducedMotion()) {
      // Duration snaps to 0: no tween, the resting render is the target.
      motions.delete(motionKey);
      return !open;
    }
    motions.set(motionKey, {
      epoch: previous === undefined ? 1 : previous.epoch + 1,
      from,
      to,
      startedAt: now,
    });
    return !open;
  }, [motionKey, open, fullHeight]);

  useEffect(() => {
    const body = bodyRef.current;
    const chevron = chevronRef.current;
    if (body === null) {
      return;
    }
    const rest = (): void => {
      motions.delete(motionKey);
      body.style.height = `${open ? fullHeight : 0}px`;
      body.style.opacity = "1";
      body.style.top = "0px";
      if (chevron !== null) {
        chevron.style.transform = `rotate(${open ? 90 : 0}deg)`;
      }
    };
    const motion = motions.get(motionKey);
    const now = performance.now();
    if (motion === undefined || !disclosureAnimating(motion, now)) {
      rest();
      return;
    }
    let raf = 0;
    const tick = (): void => {
      const frame = performance.now();
      const live = motions.get(motionKey);
      const frameBody = bodyRef.current;
      if (frameBody === null || live === undefined || !disclosureAnimating(live, frame)) {
        if (frameBody !== null) {
          rest();
        }
        return;
      }
      const height = disclosureCurrent(live, frame);
      const reveal = fullHeight > 0 ? clamp01(height / fullHeight) : 1;
      frameBody.style.height = `${height}px`;
      // Never fully opaque/transparent mid-slide (render_sidebar_disclosure_body).
      frameBody.style.opacity = `${0.35 + 0.65 * reveal}`;
      // A 3px upward creep as it opens, settling to 0 at full reveal.
      frameBody.style.top = `${-3 * (1 - reveal)}px`;
      if (chevronRef.current !== null) {
        // The chevron rides the same tween, normalized like the desktop's
        // denominator = max(from, to, 1) so mid-flight reversals stay smooth.
        const denominator = Math.max(live.from, live.to, 1);
        const from = clamp01(live.from / denominator);
        const to = clamp01(live.to / denominator);
        const t = easedProgress(live, frame);
        chevronRef.current.style.transform = `rotate(${lerp(from, to, t) * 90}deg)`;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [motionKey, open, fullHeight]);

  return { bodyRef, chevronRef, toggle };
}

/**
 * `render_pinned_divider` (38a8f013): a sibling of the disclosure body whose
 * whole frame — hairline box plus its top gap — rides the SAME tween as the
 * body, so neither the line nor an empty gap survives a collapse. The reveal
 * is the body's normalized progress (current height / full height), and the
 * resting render is the open/closed height pair.
 */
export function useSidebarDisclosureDivider(
  motionKey: string,
  open: boolean,
  frameHeight: number,
  fullHeight: number,
) {
  const dividerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const divider = dividerRef.current;
    if (divider === null) {
      return;
    }
    const rest = (): void => {
      divider.style.height = `${open ? frameHeight : 0}px`;
      divider.style.opacity = open ? "1" : "0";
    };
    const motion = motions.get(motionKey);
    const now = performance.now();
    if (motion === undefined || !disclosureAnimating(motion, now)) {
      rest();
      return;
    }
    let raf = 0;
    const tick = (): void => {
      const frame = performance.now();
      const live = motions.get(motionKey);
      const el = dividerRef.current;
      if (el === null || live === undefined || !disclosureAnimating(live, frame)) {
        if (el !== null) {
          rest();
        }
        return;
      }
      const height = disclosureCurrent(live, frame);
      const reveal = fullHeight > 0 ? clamp01(height / fullHeight) : 1;
      el.style.height = `${frameHeight * reveal}px`;
      el.style.opacity = `${reveal}`;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [motionKey, open, frameHeight, fullHeight]);

  return { dividerRef };
}

/**
 * `sidebar_disclosure_header`: muted 12px MEDIUM label, the hairline rule
 * (device groups only — `with_rule`; Pinned and Archived go bare, matching
 * the desktop's 38a8f013/adc290e3 polish) filling the middle, the chevron
 * at the end. 28px tall, 8px inline padding.
 */
export function SidebarDisclosureHeader({
  id,
  label,
  open,
  withRule = true,
  chevronRef,
  onToggle,
}: {
  id?: string;
  label: string;
  open: boolean;
  /** Whether the hairline rule fills the middle (adc290e3: device groups only). */
  withRule?: boolean;
  chevronRef: React.RefObject<HTMLSpanElement | null>;
  onToggle: () => void;
}) {
  return (
    <button type="button" id={id} className="sidebar-disclosure-header" aria-expanded={open} onClick={onToggle}>
      <SidebarFadedLabel className="sidebar-disclosure-label">{label}</SidebarFadedLabel>
      {withRule ? (
        <span className="sidebar-disclosure-rule" />
      ) : (
        // No rule: an invisible spring keeps the chevron right-aligned
        // (7c7b574b — the rule did that job on the device-group headers).
        <span className="sidebar-disclosure-spacer" />
      )}
      <span ref={chevronRef} className="sidebar-disclosure-chevron">
        <Icon name="altArrowRight" size={12} />
      </span>
    </button>
  );
}

/**
 * `render_sidebar_disclosure_body`: the content stays mounted (the desktop
 * renders it always, height 0 clipping it closed); only the wrapper's
 * height/opacity/offset animate.
 */
export function SidebarDisclosureBody({
  bodyRef,
  children,
}: {
  bodyRef: React.RefObject<HTMLDivElement | null>;
  children: React.ReactNode;
}) {
  return (
    <div ref={bodyRef} className="sidebar-disclosure-body">
      {children}
    </div>
  );
}
