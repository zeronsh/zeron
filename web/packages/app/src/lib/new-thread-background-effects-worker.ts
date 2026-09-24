/**
 * The new-thread background rasterizer worker — the web peer of the
 * desktop's background executor (new_thread_background_effects.rs:69-105):
 * the four effect rasters are pure CPU work over up-to-2048px artwork, so
 * they run here, off the main thread, and the resolved raster (buffer
 * transferred, zero-copy) refreshes the hero. No animation, no state: one
 * job in, one raster out.
 */

import {
  rasterizeEffect,
  type RasterJobRequest,
  type RasterJobResponse,
} from "./new-thread-background-effects";

const scope = self as unknown as {
  onmessage: ((event: MessageEvent<RasterJobRequest>) => void) | null;
  postMessage: (message: RasterJobResponse, transfer?: Transferable[]) => void;
};

scope.onmessage = (event: MessageEvent<RasterJobRequest>): void => {
  const { id, source, effect, light } = event.data;
  try {
    const raster = rasterizeEffect(source, effect, light);
    scope.postMessage({ id, raster }, [raster.data.buffer]);
  } catch (error) {
    scope.postMessage({
      id,
      error: error instanceof Error ? error.message : String(error),
    });
  }
};
