import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  ImageViewState,
  imageOrigin,
  resize as resizeGeometry,
  type ImageSize,
} from "../../lib/image-geometry";
import type { WorkspaceFilesClient } from "../../lib/files-client";

/**
 * `ImageView` — the shared image viewport (crates/ui/src/image_viewer.rs):
 * fit-to-viewport on load and resize (never upscale past 1:1), ctrl+wheel
 * zoom anchored at the cursor, unmodified wheel pans, two-pointer pinch
 * zoom, and left-drag pan with a 4px threshold so a real drag never reads
 * as a click. Drag continuation keeps working after the pointer leaves the
 * image (move/up listeners ride `window`).
 *
 * The in-panel viewer passes no `onImageClick` (a plain click inside the
 * image does nothing — no click-to-zoom on desktop); the markdown
 * lightbox passes a close callback instead (image_preview.rs's lightbox
 * wiring).
 */

export interface ImageViewProps {
  readonly src: string;
  readonly natural: ImageSize;
  /**
   * Plain click (no preceding drag) inside the image — the lightbox's
   * close-on-click hook. Absent for the in-panel viewer.
   */
  readonly onImageClick?: () => void;
  readonly alt?: string;
}

export function ImageView({ src, natural, onImageClick, alt }: ImageViewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef(new ImageViewState());
  const frameRef = useRef(0);
  const [, setFrame] = useState(0);
  const pointersRef = useRef(new Map<number, { x: number; y: number }>());
  const pinchRef = useRef<{ distance: number } | null>(null);

  const bump = useCallback((): void => {
    frameRef.current += 1;
    setFrame(frameRef.current);
  }, []);

  // A fresh image loads fitted (ViewState::default).
  useEffect(() => {
    viewRef.current.reset();
    bump();
  }, [src, bump]);

  // `resize` — a fitted view re-fits, a manual zoom only re-clamps.
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    const apply = (): void => {
      const rect = container.getBoundingClientRect();
      viewRef.current.geometry = resizeGeometry(viewRef.current.geometry, natural, {
        width: rect.width,
        height: rect.height,
      });
      bump();
    };
    apply();
    const observer = new ResizeObserver(apply);
    observer.observe(container);
    return () => {
      observer.disconnect();
    };
  }, [natural, bump]);

  // The wheel path needs a non-passive listener (preventDefault on zoom).
  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    const onWheel = (event: WheelEvent): void => {
      const view = viewRef.current;
      const rect = container.getBoundingClientRect();
      const consumed = view.wheel({
        x: event.clientX - rect.left,
        y: event.clientY - rect.top,
        deltaX: event.deltaX,
        deltaY: event.deltaY,
        deltaMode: event.deltaMode,
        ctrl: event.ctrlKey,
      });
      if (consumed) {
        event.preventDefault();
        bump();
      }
    };
    container.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      container.removeEventListener("wheel", onWheel);
    };
  }, [bump]);

  const localPoint = (event: PointerEvent): { x: number; y: number } => {
    const rect = containerRef.current?.getBoundingClientRect();
    return {
      x: event.clientX - (rect?.left ?? 0),
      y: event.clientY - (rect?.top ?? 0),
    };
  };

  // Drag + pinch: move/up ride `window` so a drag continues past the
  // image's own hitbox (image_viewer.rs's window.on_mouse_event).
  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    const onPointerDown = (event: PointerEvent): void => {
      if (event.button !== 0) {
        return;
      }
      pointersRef.current.set(event.pointerId, localPoint(event));
      if (pointersRef.current.size === 2) {
        const [a, b] = [...pointersRef.current.values()];
        pinchRef.current = { distance: Math.hypot(a!.x - b!.x, a!.y - b!.y) };
        return;
      }
      viewRef.current.pointerDown(localPoint(event));
    };
    const onPointerMove = (event: PointerEvent): void => {
      const pointers = pointersRef.current;
      if (!pointers.has(event.pointerId)) {
        // The drag's own move/up listeners below carry single-pointer pans.
        return;
      }
      pointers.set(event.pointerId, localPoint(event));
      const pinch = pinchRef.current;
      if (pinch !== null && pointers.size >= 2) {
        const [a, b] = [...pointers.values()];
        const distance = Math.hypot(a!.x - b!.x, a!.y - b!.y);
        if (distance > 0 && pinch.distance > 0) {
          const mid = { x: (a!.x + b!.x) / 2, y: (a!.y + b!.y) / 2 };
          // Native-delta accumulation, web translation: the multiplicative
          // factor from this move feeds the same accumulate-and-zoom machine.
          viewRef.current.pinch({
            x: mid.x,
            y: mid.y,
            phase: "moved",
            delta: distance / pinch.distance - 1,
          });
          pinch.distance = distance;
          bump();
        }
        return;
      }
      if (viewRef.current.pointerMove(localPoint(event), (event.buttons & 1) !== 0)) {
        bump();
      }
    };
    const onPointerUp = (event: PointerEvent): void => {
      pointersRef.current.delete(event.pointerId);
      if (pointersRef.current.size < 2) {
        pinchRef.current = null;
      }
      // A click after a real drag is swallowed (ViewState.dragged).
      if (
        onImageClick !== undefined &&
        viewRef.current.dragged === false &&
        pointersRef.current.size === 0
      ) {
        const local = localPoint(event);
        if (viewRef.current.pointInsideImage(local)) {
          onImageClick();
        }
      }
    };
    const onPointerCancel = (event: PointerEvent): void => {
      pointersRef.current.delete(event.pointerId);
      pinchRef.current = null;
    };
    container.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onPointerCancel);
    return () => {
      container.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerCancel);
    };
  }, [onImageClick, bump]);

  const geometry = viewRef.current.geometry;
  const origin = imageOrigin(geometry);

  return (
    <div
      ref={containerRef}
      className="files-image-viewport"
      data-testid="image-view"
      // A click after a real drag is swallowed before it can reach the
      // lightbox scrim (the desktop's `state.dragged` click guard).
      onClick={(event) => {
        if (viewRef.current.dragged) {
          event.stopPropagation();
          event.preventDefault();
        }
      }}
    >
      <img
        className="files-image"
        src={src}
        alt={alt ?? ""}
        draggable={false}
        style={{
          left: `${origin.x}px`,
          top: `${origin.y}px`,
          width: `${natural.width * geometry.scale}px`,
          height: `${natural.height * geometry.scale}px`,
        }}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// The workspace image load (readFile → readImage → decode)
// ---------------------------------------------------------------------------

export interface WorkspaceImageLoad {
  readonly url: string;
  readonly width: number;
  readonly height: number;
  /** Decoded size estimate (w × h × 4) for the memory budgets. */
  readonly decodedBytes: number;
}

/** The 30s per-image timeout (markdown_media.rs / image_preview.rs). */
const IMAGE_TIMEOUT_MS = 30_000;
/** `MAX_MEDIA_BYTES` — the decoded-image memory cap. */
const MAX_DECODED_BYTES = 64 * 1024 * 1024;

function withTimeout<T>(promise: Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error("Image preview timed out"));
    }, IMAGE_TIMEOUT_MS);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });
}

function decodeImage(url: string): Promise<{ width: number; height: number }> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = (): void => {
      resolve({ width: image.naturalWidth, height: image.naturalHeight });
    };
    image.onerror = (): void => {
      reject(new Error("This image was removed from the workspace."));
    };
    image.src = url;
  });
}

/**
 * The image read pipeline (image_preview.rs: the read resolves the checkout
 * identity, the chunked RPC carries the bytes — 8 MiB wire cap inside
 * `readImage`), the 30s timeout, and the decode whose 64 MiB decoded-size
 * check guards the retained-memory budget. The caller owns the returned
 * object URL's lifetime (`URL.revokeObjectURL` on unmount).
 */
export async function loadWorkspaceImage(
  client: WorkspaceFilesClient,
  path: string,
): Promise<WorkspaceImageLoad> {
  const { url, width, height } = await withTimeout(
    (async () => {
      const file = await client.readFile(path);
      const image = await client.readImage(path, file.checkoutId);
      const objectUrl = URL.createObjectURL(new Blob([image.bytes as BlobPart], { type: image.mimeType }));
      const size = await decodeImage(objectUrl);
      return { url: objectUrl, ...size };
    })(),
  );
  const decodedBytes = width * height * 4;
  if (decodedBytes > MAX_DECODED_BYTES) {
    URL.revokeObjectURL(url);
    throw new Error("Image exceeds preview memory limit");
  }
  return { url, width, height, decodedBytes };
}
