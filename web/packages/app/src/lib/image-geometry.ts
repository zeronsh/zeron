/**
 * Image viewer geometry and pointer-gesture state — a port of
 * `crates/ui/src/image_viewer.rs` (the `Geometry`/`ViewState` pair behind
 * every image surface: the files pane, the markdown preview's media, and
 * the lightbox). Pure math plus a small mutable state machine; the
 * components render from it and push DOM events into it.
 */

export interface ImageSize {
  readonly width: number;
  readonly height: number;
}

export interface ImagePoint {
  readonly x: number;
  readonly y: number;
}

export interface ImageGeometry {
  readonly natural: ImageSize;
  readonly viewport: ImageSize;
  readonly scale: number;
  readonly pan: ImagePoint;
  /** True while the view sits at `fitScale` with no pan (resizes re-fit). */
  readonly fitted: boolean;
}

export const DEFAULT_GEOMETRY: ImageGeometry = {
  natural: { width: 1, height: 1 },
  viewport: { width: 1, height: 1 },
  scale: 1,
  pan: { x: 0, y: 0 },
  fitted: true,
};

function isFiniteSize(size: ImageSize): boolean {
  return (
    Number.isFinite(size.width) &&
    Number.isFinite(size.height) &&
    size.width > 0 &&
    size.height > 0
  );
}

/** `fit_scale` — never upscale past 1:1. */
export function fitScale(geometry: ImageGeometry): number {
  return Math.min(
    geometry.viewport.width / geometry.natural.width,
    geometry.viewport.height / geometry.natural.height,
    1,
  );
}

/** `resize` — a fitted view re-fits; a manual zoom survives and re-clamps. */
export function resize(
  geometry: ImageGeometry,
  natural: ImageSize,
  viewport: ImageSize,
): ImageGeometry {
  if (!isFiniteSize(natural) || !isFiniteSize(viewport)) {
    return geometry;
  }
  const next: ImageGeometry = { ...geometry, natural, viewport };
  if (next.fitted) {
    return fit(next);
  }
  return clampPan(next);
}

/** `fit` — scale to fit, centered, back to the fitted state. */
export function fit(geometry: ImageGeometry): ImageGeometry {
  return { ...geometry, scale: fitScale(geometry), pan: { x: 0, y: 0 }, fitted: true };
}

/** `clamp_pan` — draggable only until the image edge reaches the viewport edge. */
export function clampPan(geometry: ImageGeometry): ImageGeometry {
  const limitX = Math.max((geometry.natural.width * geometry.scale - geometry.viewport.width) / 2, 0);
  const limitY = Math.max((geometry.natural.height * geometry.scale - geometry.viewport.height) / 2, 0);
  return {
    ...geometry,
    pan: {
      x: Math.min(Math.max(geometry.pan.x, -limitX), limitX),
      y: Math.min(Math.max(geometry.pan.y, -limitY), limitY),
    },
  };
}

/**
 * `zoom` — clamp to `[min(fitScale, 0.01), min(131072/maxSide, 32).max(fit)]`
 * and keep the image point under the anchor fixed. A zoom always un-fits.
 */
export function zoom(geometry: ImageGeometry, scale: number, anchor: ImagePoint): ImageGeometry {
  if (
    !Number.isFinite(scale) ||
    scale <= 0 ||
    !Number.isFinite(anchor.x) ||
    !Number.isFinite(anchor.y)
  ) {
    return geometry;
  }
  const maximum = Math.max(
    Math.min(131072 / Math.max(geometry.natural.width, geometry.natural.height), 32),
    fitScale(geometry),
  );
  const clamped = Math.min(Math.max(scale, Math.min(fitScale(geometry), 0.01)), maximum);
  const ratio = clamped / geometry.scale;
  const ax = anchor.x - geometry.viewport.width / 2;
  const ay = anchor.y - geometry.viewport.height / 2;
  return clampPan({
    ...geometry,
    pan: {
      x: ax - (ax - geometry.pan.x) * ratio,
      y: ay - (ay - geometry.pan.y) * ratio,
    },
    scale: clamped,
    fitted: false,
  });
}

/** `pan_by` — returns the next geometry; `moved` reports whether it changed. */
export function panBy(geometry: ImageGeometry, delta: ImagePoint): { geometry: ImageGeometry; moved: boolean } {
  if (!Number.isFinite(delta.x) || !Number.isFinite(delta.y)) {
    return { geometry, moved: false };
  }
  const next = clampPan({
    ...geometry,
    pan: { x: geometry.pan.x + delta.x, y: geometry.pan.y + delta.y },
  });
  return { geometry: next, moved: next.pan.x !== geometry.pan.x || next.pan.y !== geometry.pan.y };
}

/** `image_origin` — centered, then offset by pan. */
export function imageOrigin(geometry: ImageGeometry): ImagePoint {
  return {
    x: (geometry.viewport.width - geometry.natural.width * geometry.scale) / 2 + geometry.pan.x,
    y: (geometry.viewport.height - geometry.natural.height * geometry.scale) / 2 + geometry.pan.y,
  };
}

/**
 * `scroll_pixels` — a wheel delta normalized to px: `lines` mode moves
 * 40px per line (image_viewer.rs:117-122).
 */
export function scrollPixels(delta: { deltaMode: number; deltaX: number; deltaY: number }): ImagePoint {
  // DOM_DELTA_LINE is 1, DOM_DELTA_PAGE is 2; the desktop only knows
  // pixels/lines, and page mode is not produced for image pan targets.
  const unit = delta.deltaMode === 1 ? 40 : 1;
  return { x: delta.deltaX * unit, y: delta.deltaY * unit };
}

/**
 * `wheel`'s zoom factor — `exp(clamp(deltaY * 0.0025, -2, 2))` on the
 * DESKTOP's deltaY (GPUI: positive = wheel up = zoom in). The DOM's wheel
 * deltaY is sign-flipped, so the factor negates it here: web wheel-up
 * (negative deltaY) still zooms in.
 */
export function wheelZoomFactor(deltaY: number): number {
  return Math.exp(Math.min(Math.max(-deltaY * 0.0025, -2), 2));
}

// ---------------------------------------------------------------------------
// The mutable view state (drag + pinch machines)
// ---------------------------------------------------------------------------

interface DragState {
  readonly start: ImagePoint;
  readonly pan: ImagePoint;
}

interface PinchState {
  readonly startScale: number;
  factor: number;
}

/**
 * `ViewState` — the per-viewer gesture machine: wheel (ctrl ⇒ zoom at the
 * cursor, else pan), two-pointer pinch accumulating native deltas, and a
 * left-drag pan that only counts after a 4px threshold (so a click after a
 * drag can be swallowed).
 */
export class ImageViewState {
  geometry: ImageGeometry = DEFAULT_GEOMETRY;
  #drag: DragState | null = null;
  #dragged = false;
  #pinch: PinchState | null = null;

  /** The wheel path — returns true when the event was consumed. */
  wheel(event: { x: number; y: number; deltaX: number; deltaY: number; deltaMode: number; ctrl: boolean }): boolean {
    const delta = scrollPixels(event);
    if (event.ctrl) {
      this.geometry = zoom(this.geometry, this.geometry.scale * wheelZoomFactor(delta.y), {
        x: event.x,
        y: event.y,
      });
      return true;
    }
    const { geometry, moved } = panBy(this.geometry, delta);
    this.geometry = geometry;
    return moved;
  }

  /**
   * The pinch path — native-delta accumulation: the gesture remembers its
   * start scale and a multiplicative factor, each moved event adds its
   * delta to the factor, and the target scale is `start * factor`.
   */
  pinch(event: { x: number; y: number; phase: "started" | "moved" | "ended"; delta: number }): void {
    if (event.phase === "ended") {
      this.#pinch = null;
      return;
    }
    if (!Number.isFinite(event.delta)) {
      return;
    }
    if (event.phase === "started") {
      this.#pinch = { startScale: this.geometry.scale, factor: 1 };
    }
    const pinch = this.#pinch ?? { startScale: this.geometry.scale, factor: 1 };
    this.#pinch = pinch;
    pinch.factor = Math.max(pinch.factor + event.delta, 0.001);
    this.geometry = zoom(this.geometry, pinch.startScale * pinch.factor, { x: event.x, y: event.y });
  }

  /** `pointer_down` — arm a drag candidate. */
  pointerDown(position: ImagePoint): void {
    this.#dragged = false;
    this.#drag = { start: position, pan: this.geometry.pan };
  }

  /**
   * `pointer_move` — pan under the drag, but only once the 4px threshold is
   * crossed; returns true when the move was consumed (a drag in flight).
   */
  pointerMove(position: ImagePoint, dragging: boolean): boolean {
    if (!dragging) {
      this.#drag = null;
      return false;
    }
    const drag = this.#drag;
    if (drag === null) {
      return false;
    }
    const dx = position.x - drag.start.x;
    const dy = position.y - drag.start.y;
    if (Math.hypot(dx, dy) >= 4) {
      this.#dragged = true;
    }
    if (!this.#dragged) {
      return false;
    }
    this.geometry = clampPan({ ...this.geometry, pan: drag.pan });
    const { geometry } = panBy(this.geometry, { x: dx, y: dy });
    this.geometry = geometry;
    return true;
  }

  /** A click is only a click when no drag threshold was crossed first. */
  get dragged(): boolean {
    return this.#dragged;
  }

  /** `reset` — a fresh viewer state (a new image loads fitted). */
  reset(): void {
    this.geometry = DEFAULT_GEOMETRY;
    this.#drag = null;
    this.#dragged = false;
    this.#pinch = null;
  }

  /** Whether a local point lands inside the rendered image rect. */
  pointInsideImage(local: ImagePoint): boolean {
    const origin = imageOrigin(this.geometry);
    return (
      local.x >= origin.x &&
      local.y >= origin.y &&
      local.x <= origin.x + this.geometry.natural.width * this.geometry.scale &&
      local.y <= origin.y + this.geometry.natural.height * this.geometry.scale
    );
  }
}
