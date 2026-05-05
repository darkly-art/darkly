import { app } from '../../state/app.svelte';

// Mirrors `darkly::engine::DEFAULT_THUMB_SIZE`. The engine's auto-queue
// path renders thumbnail readbacks at this size; if it ever drifts,
// the cached bytes won't fit our HTML img dimensions. `app.svelte.ts`
// asserts equality against `handle.engine_default_thumb_size()` at
// init so drift fails loudly on first run, not silently.
const THUMB_SIZE = 36;

/** Convert RGBA byte array to a data URL suitable for <img src>. */
export function rgbaToDataUrl(rgba: Uint8Array, width: number, height: number): string {
    const tmpCanvas = document.createElement('canvas');
    tmpCanvas.width = width;
    tmpCanvas.height = height;
    const tmpCtx = tmpCanvas.getContext('2d')!;
    // Copy into a Uint8ClampedArray backed by a fresh ArrayBuffer to satisfy ImageData
    const clamped = new Uint8ClampedArray(rgba.length);
    clamped.set(rgba);
    const imageData = new ImageData(clamped, width, height);
    tmpCtx.putImageData(imageData, 0, 0);
    return tmpCanvas.toDataURL();
}

/** Get a thumbnail as a data URL for any node id (raster layer or modifier).
 *  Returns empty string when no cached bytes exist yet. */
export function getNodeThumbnail(nodeId: number): string {
    // Subscribe to `thumbnailEpoch` so any `$derived` calling this
    // function re-runs when an async readback lands in the wasm cache.
    // Do NOT delete this read — without it the cache update is invisible
    // to Svelte and thumbnails freeze on the placeholder returned by
    // the first call. (See `app.svelte.ts` `requestFrame` for the bump.)
    void app.thumbnailEpoch;
    if (!app.handle) return '';
    const rgba = app.handle.node_thumbnail(nodeId, THUMB_SIZE, THUMB_SIZE);
    if (!rgba || rgba.length === 0) return '';
    return rgbaToDataUrl(rgba, THUMB_SIZE, THUMB_SIZE);
}

export { THUMB_SIZE };
