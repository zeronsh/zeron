import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { createPortal } from "react-dom";

/**
 * The shared attachment lightbox — the web port of
 * `crates/ui/src/attachments.rs::lightbox_with_size` over the pan/zoom
 * geometry of `crates/ui/src/image_viewer.rs` (`ImageView`). One component
 * serves both entry points: the composer's staged strip (local previewUrl
 * bytes) and the transcript's user-row thumbnails (read-back bytes). No
 * entrance animation, no caption beyond the filename — the desktop mounts
 * it deferred at top priority and that is the whole chrome.
 *
 * Geometry (image_viewer.rs): `fit_scale` never upscales; zoom is bounded to
 * `min(131072/max(natural.w, natural.h), 32)` and down to
 * `min(fit_scale, 0.01)`; a zoom keeps the anchor point over the same image
 * pixel; pan is clamped to `max((natural·scale − viewport)/2, 0)` per axis.
 * Wheel: plain wheel pans, ctrl+wheel (the browser's synthesized pinch)
 * zooms by `scale · exp(clamp(±dy·0.0025, −2, 2))` — the sign is flipped from
 * the desktop's because GPUI's positive wheel-Y means "wheel up" while the
 * browser's means "wheel down". A press becomes a drag past 4px; a click that
 * was not a drag closes, wherever it lands.
 */

/** `image_viewer.rs::Geometry` — the pure state the interactions advance. */
interface ViewerGeometry {
  readonly natural: { readonly width: number; readonly height: number };
  readonly viewport: { readonly width: number; readonly height: number };
  readonly scale: number;
  readonly panX: number;
  readonly panY: number;
  readonly fitted: boolean;
}

const NATURAL_DEFAULT = { width: 1, height: 1 };
const VIEWPORT_DEFAULT = { width: 1, height: 1 };

function initialGeometry(): ViewerGeometry {
  return { natural: NATURAL_DEFAULT, viewport: VIEWPORT_DEFAULT, scale: 1, panX: 0, panY: 0, fitted: true };
}

/** `fit_scale` — `min(vw/nw, vh/nh, 1.0)`, never an upscale. */
function fitScale(geometry: ViewerGeometry): number {
  const byWidth = geometry.viewport.width / geometry.natural.width;
  const byHeight = geometry.viewport.height / geometry.natural.height;
  return Math.min(byWidth, byHeight, 1.0);
}

/** `resize` — re-fit while fitted, otherwise keep the scale and clamp. */
function resizeGeometry(geometry: ViewerGeometry, natural: { width: number; height: number }, viewport: { width: number; height: number }): ViewerGeometry {
  const values = [natural.width, natural.height, viewport.width, viewport.height];
  if (!values.every((v) => Number.isFinite(v) && v > 0)) {
    return geometry;
  }
  const resized = { ...geometry, natural, viewport };
  return resized.fitted ? fit(resized) : clampPan(resized);
}

function fit(geometry: ViewerGeometry): ViewerGeometry {
  return { ...geometry, scale: fitScale(geometry), panX: 0, panY: 0, fitted: true };
}

/** `clamp_pan` — `max((natural·scale − viewport)/2, 0)` per axis. */
function clampPan(geometry: ViewerGeometry): ViewerGeometry {
  const limitX = Math.max((geometry.natural.width * geometry.scale - geometry.viewport.width) / 2, 0);
  const limitY = Math.max((geometry.natural.height * geometry.scale - geometry.viewport.height) / 2, 0);
  return {
    ...geometry,
    panX: Math.min(Math.max(geometry.panX, -limitX), limitX),
    panY: Math.min(Math.max(geometry.panY, -limitY), limitY),
  };
}

/**
 * `zoom` — clamp the requested scale, then adjust pan so the anchor point
 * stays over the same image pixel (anchors are viewport-relative).
 */
function zoomAt(geometry: ViewerGeometry, scale: number, anchorX: number, anchorY: number): ViewerGeometry {
  if (!Number.isFinite(scale) || scale <= 0 || !Number.isFinite(anchorX) || !Number.isFinite(anchorY)) {
    return geometry;
  }
  // Bound layout coordinates even for SVGs with enormous logical dimensions.
  const maximum = Math.max(Math.min(131072 / Math.max(geometry.natural.width, geometry.natural.height), 32), fitScale(geometry));
  const clamped = Math.min(Math.max(scale, Math.min(fitScale(geometry), 0.01)), maximum);
  const ratio = clamped / geometry.scale;
  const ax = anchorX - geometry.viewport.width / 2;
  const ay = anchorY - geometry.viewport.height / 2;
  return clampPan({
    ...geometry,
    panX: ax - (ax - geometry.panX) * ratio,
    panY: ay - (ay - geometry.panY) * ratio,
    scale: clamped,
    fitted: false,
  });
}

/** `pan_by` — returns the same geometry when nothing moved (drag tests). */
function panBy(geometry: ViewerGeometry, deltaX: number, deltaY: number): ViewerGeometry {
  if (!Number.isFinite(deltaX) || !Number.isFinite(deltaY)) {
    return geometry;
  }
  return clampPan({ ...geometry, panX: geometry.panX + deltaX, panY: geometry.panY + deltaY });
}

/** `image_origin` — the image element's top-left, viewport-relative. */
function imageOrigin(geometry: ViewerGeometry): { x: number; y: number } {
  return {
    x: (geometry.viewport.width - geometry.natural.width * geometry.scale) / 2 + geometry.panX,
    y: (geometry.viewport.height - geometry.natural.height * geometry.scale) / 2 + geometry.panY,
  };
}

export interface LightboxProps {
  /** The filename strip under the image (`preview.name`). */
  readonly name: string;
  /** The image source (a data URL — staged preview or read-back bytes). */
  readonly src: string;
  readonly onClose: () => void;
}

export function Lightbox({ name, src, onClose }: LightboxProps) {
  const frameRef = useRef<HTMLDivElement | null>(null);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [natural, setNatural] = useState<{ width: number; height: number } | null>(null);
  const [viewport, setViewport] = useState<{ width: number; height: number } | null>(null);
  const [geometry, setGeometry] = useState<ViewerGeometry>(initialGeometry);
  const [press, setPress] = useState<{
    startX: number;
    startY: number;
    panX: number;
    panY: number;
    moved: boolean;
  } | null>(null);

  // Decode the natural size (`lightbox_with_size`'s natural_size fallback:
  // the decoded image's own dimensions). Zero-dimension SVGs stay "Loading…".
  useEffect(() => {
    setNatural(null);
    const image = new Image();
    image.onload = () => {
      if (image.naturalWidth > 0 && image.naturalHeight > 0) {
        setNatural({ width: image.naturalWidth, height: image.naturalHeight });
      }
    };
    image.src = src;
    return () => {
      image.onload = null;
    };
  }, [src]);

  // Measure the image box (its size is 90% × 85% of the frame, but the frame
  // tracks the window, so a live measure beats assuming).
  useLayoutEffect(() => {
    const el = viewportRef.current;
    if (el === null || typeof ResizeObserver === "undefined") {
      return;
    }
    const publish = (): void => {
      setViewport({ width: el.clientWidth, height: el.clientHeight });
    };
    const observer = new ResizeObserver(publish);
    observer.observe(el);
    publish();
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (natural === null || viewport === null) {
      return;
    }
    setGeometry((current) => resizeGeometry(current, natural, viewport));
  }, [natural, viewport]);

  // Take focus so Escape reaches the lightbox before any shell surface
  // (`track_focus`), and hand it back to the caller's element on close.
  useLayoutEffect(() => {
    const frame = frameRef.current;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    frame?.focus();
    return () => {
      if (previous !== null && previous.isConnected) {
        previous.focus();
      }
    };
  }, []);

  // The wheel listener must be non-passive to preventDefault (React attaches
  // wheel passively); it swallows every wheel so the transcript never scrolls
  // underneath, and pans/zooms while the cursor is over the image viewport.
  useEffect(() => {
    const frame = frameRef.current;
    if (frame === null) {
      return;
    }
    const onWheel = (event: WheelEvent): void => {
      event.stopPropagation();
      event.preventDefault();
      const box = viewportRef.current;
      if (box === null || !box.contains(event.target as Node)) {
        return;
      }
      const line = event.deltaMode === 1 || event.deltaMode === 2 ? 40 : 1;
      const dx = event.deltaX * line;
      const dy = event.deltaY * line;
      const rect = box.getBoundingClientRect();
      const anchorX = event.clientX - rect.left;
      const anchorY = event.clientY - rect.top;
      if (event.ctrlKey) {
        // ctrl+wheel is the browser's pinch gesture; GPUI's positive Y is
        // wheel-up, the browser's is wheel-down, hence the sign flip.
        const factor = Math.exp(Math.min(Math.max(-dy * 0.0025, -2), 2));
        setGeometry((current) => zoomAt(current, current.scale * factor, anchorX, anchorY));
      } else {
        setGeometry((current) => panBy(current, -dx, -dy));
      }
    };
    frame.addEventListener("wheel", onWheel, { passive: false });
    return () => frame.removeEventListener("wheel", onWheel);
  }, []);

  // A click that never crossed the 4px drag threshold closes, wherever it
  // ended (the desktop's frame-level on_click + `!viewer.dragged()`). Pointer
  // capture keeps the drag tracking when the cursor leaves the window — the
  // desktop achieves the same with a window-level move handler.
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (event.button !== 0) {
      return;
    }
    setPress({
      startX: event.clientX,
      startY: event.clientY,
      panX: geometry.panX,
      panY: geometry.panY,
      moved: false,
    });
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      // An already-released pointer — the move/up handlers no-op anyway.
    }
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (press === null || (event.buttons & 1) === 0) {
      return;
    }
    const deltaX = event.clientX - press.startX;
    const deltaY = event.clientY - press.startY;
    if (!press.moved && Math.hypot(deltaX, deltaY) < 4) {
      return;
    }
    setPress({ ...press, moved: true });
    setGeometry(clampPan({ ...geometry, panX: press.panX + deltaX, panY: press.panY + deltaY }));
  };

  const onPointerUp = (): void => {
    if (press === null) {
      return;
    }
    const moved = press.moved;
    setPress(null);
    if (!moved) {
      onClose();
    }
  };

  const origin = imageOrigin(geometry);
  const loading = natural === null;

  // Portaled to the body: the composer pill (the staged strip's entry point)
  // carries `backdrop-filter`, which forms a containing block for fixed
  // descendants — a fixed overlay in the pill would size to the pill, not
  // the viewport. The desktop's `deferred`/`anchored` layer is exactly this
  // escape from the paint tree.
  return createPortal(
    <div
      ref={frameRef}
      className="lightbox"
      id="attachment-lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={name}
      tabIndex={-1}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => setPress(null)}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="lightbox-image-box" ref={viewportRef}>
        {loading ? (
          <span className="lightbox-loading">Loading image…</span>
        ) : (
          <img
            className="lightbox-image"
            src={src}
            alt={name}
            draggable={false}
            style={{
              left: `${origin.x}px`,
              top: `${origin.y}px`,
              width: `${geometry.natural.width * geometry.scale}px`,
              height: `${geometry.natural.height * geometry.scale}px`,
            }}
          />
        )}
      </div>
      <div className="lightbox-name" title={name}>
        {name}
      </div>
    </div>,
    document.body,
  );
}

// Pure geometry exports (image_viewer.rs `#[cfg(test)]`-style seams, kept
// module-private except for tests via the lightbox.test import below).
export const __viewerGeometryForTests = {
  initialGeometry,
  resizeGeometry,
  fit,
  fitScale,
  clampPan,
  zoomAt,
  panBy,
  imageOrigin,
};
