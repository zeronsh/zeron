import { useCallback, useEffect, useRef, useState } from "react";
import { RESIZE_EDGE_BOUNCE_MS, resizeBounceOffset, resizeDragSample, type ResizeEdge } from "../state/layout";

/**
 * A column resize seam — the desktop's `shell.rs::resize_handle`.
 *
 * Geometry is the desktop's: a 20px transparent hit target centred on the
 * seam, running from the titlebar's bottom edge to the window's, so the strip
 * never steals clicks from the titlebar controls above it. The visual divider
 * stays the adjacent pane's own 1px border; hovering adds a stronger highlight
 * that fades toward both ends, and it goes solid while dragging — EXCEPT
 * while dragging at a clamped edge, where it hides instead (the seam goes dark
 * to say "you've hit the limit", `shell.rs:5726-5735`).
 *
 * With `bounds`, a drag runs through `motion.rs::resize_drag_sample`: the
 * width clamps, the edge is detected, and hitting a NEW edge under a pointer
 * that has not latched it arms the 220ms 5px edge bounce (§3.23), driven by a
 * rAF loop that writes the offset into a CSS variable the column's width
 * adds. The latch is what makes a HELD pointer produce exactly one nudge —
 * leaving the edge rearms it. Under `prefers-reduced-motion: reduce` no
 * bounce ever starts (`eval_resize_edge_bounce` returns 0 there).
 *
 * Double-click restores the column's default width, as on the desktop.
 */
export interface PaneSeamProps {
  readonly label: string;
  /** Map a pointer x (client coords) to the requested column width. */
  readonly widthAt: (clientX: number) => number;
  readonly onWidth: (width: number) => void;
  readonly onReset: () => void;
  readonly className?: string;
  /**
   * The desktop's clamp bounds for this seam. When `max < min` (a window too
   * narrow for the pane's floor beside the chat's), `on_right_pane_drag` pins
   * to `max` with no edge and no bounce — the chat wins, the pane yields.
   */
  readonly bounds?: { readonly min: number; readonly max: number };
  /** The CSS variable the bounce pulse is written to (e.g. `--rb-pane-edge-offset`). */
  readonly bounceVar?: string;
}

function prefersReducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function PaneSeam({ label, widthAt, onWidth, onReset, className, bounds, bounceVar }: PaneSeamProps) {
  const [dragging, setDragging] = useState(false);
  /** The edge this press is currently latched at, or null when mid-range. */
  const latchedRef = useRef<ResizeEdge>(null);
  /** The live edge (drives the constrained look while the press is held). */
  const [edge, setEdge] = useState<ResizeEdge>(null);
  const rafRef = useRef<number | null>(null);
  // The handlers outlive a render, so read the live callbacks through a ref
  // rather than re-binding the window listeners on every parent render.
  const latest = useRef({ widthAt, onWidth, bounds, bounceVar });
  latest.current = { widthAt, onWidth, bounds, bounceVar };

  const stopBounce = useCallback(() => {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    const { bounceVar: varName } = latest.current;
    if (varName !== undefined) {
      document.documentElement.style.setProperty(varName, "0px");
    }
  }, []);

  /** The 220ms out-and-back pulse, written straight into the column's width var. */
  const startBounce = useCallback((hit: Exclude<ResizeEdge, null>) => {
    if (prefersReducedMotion()) {
      return;
    }
    stopBounce();
    const started = performance.now();
    const { bounceVar: varName } = latest.current;
    if (varName === undefined) {
      return;
    }
    const tick = (now: number): void => {
      const elapsed = now - started;
      const offset = resizeBounceOffset(hit, elapsed);
      document.documentElement.style.setProperty(varName, `${offset}px`);
      if (elapsed < RESIZE_EDGE_BOUNCE_MS) {
        rafRef.current = requestAnimationFrame(tick);
      } else {
        rafRef.current = null;
        document.documentElement.style.setProperty(varName, "0px");
      }
    };
    rafRef.current = requestAnimationFrame(tick);
  }, [stopBounce]);

  useEffect(() => stopBounce, [stopBounce]);

  const onPointerDown = useCallback((event: React.PointerEvent) => {
    // Suppress the text selection a horizontal drag would otherwise paint
    // across the transcript.
    event.preventDefault();
    // A fresh press rearms the latch cycle (`finish_pane_resize` clears it).
    latchedRef.current = null;
    setEdge(null);
    setDragging(true);
  }, []);

  useEffect(() => {
    if (!dragging) {
      return;
    }
    const onMove = (event: PointerEvent): void => {
      const { widthAt: map, onWidth: commit, bounds: limits } = latest.current;
      const requested = map(event.clientX);
      if (limits === undefined) {
        commit(requested);
        return;
      }
      let sample;
      if (limits.max < limits.min) {
        // `on_right_pane_drag`'s narrow-window branch: pin, no edge, no bounce.
        sample = { width: limits.max, edge: null, startsBounce: false };
      } else {
        sample = resizeDragSample(requested, limits.min, limits.max, latchedRef.current, prefersReducedMotion());
      }
      commit(sample.width);
      if (sample.edge === null) {
        // Leaving the edge rearms the latch for the next hit.
        latchedRef.current = null;
      } else if (sample.startsBounce) {
        latchedRef.current = sample.edge;
        startBounce(sample.edge);
      }
      setEdge(sample.edge);
    };
    const onUp = (): void => {
      setDragging(false);
      setEdge(null);
      latchedRef.current = null;
      stopBounce();
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    // A drag owns the cursor even when the pointer outruns the 20px strip.
    const previous = document.body.style.cursor;
    document.body.style.cursor = "col-resize";
    // Columns glide on toggles but must track the pointer exactly on a drag —
    // the desktop drops the tween for the duration (`right_tween = None`).
    document.documentElement.setAttribute("data-rb-resizing", "");
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      document.body.style.cursor = previous;
      document.documentElement.removeAttribute("data-rb-resizing");
      stopBounce();
    };
  }, [dragging, startBounce, stopBounce]);

  return (
    <div
      className={[
        "pane-seam",
        dragging ? "pane-seam-dragging" : "",
        // `pane_resize_active` cleared at an edge ⇒ constrained: the line
        // hides while the pointer is held at a clamp bound.
        dragging && edge !== null ? "pane-seam-constrained" : "",
        className ?? "",
      ]
        .filter((part) => part.length > 0)
        .join(" ")}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      onPointerDown={onPointerDown}
      onDoubleClick={onReset}
    >
      <div className="pane-seam-line" />
    </div>
  );
}
