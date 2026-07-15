import { app } from '../../state/app.svelte';

// Mirrors `darkly::engine::DEFAULT_THUMB_SIZE`. The engine's auto-queue
// path renders thumbnail readbacks at this size; if it ever drifts,
// the cached bytes won't fit our HTML img dimensions. `CanvasView` asserts
// equality against `engine.engineDefaultThumbSize()` at init so drift fails
// loudly on first run, not silently.
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

// Reactive cache of node id → data URL. `node_thumbnail` is an async engine
// query now, so it can't be read inline by the layer panel's `$derived`s.
// Instead `getNodeThumbnail` returns the cached URL synchronously and kicks
// an async refresh keyed on `thumbnailEpoch`; when the bytes land we write
// them here, and because this is `$state` the `$derived` consumers re-run.
const cache = $state<Record<number, string>>({});
// Last epoch we fetched per node id — guards against re-fetching the same
// node many times within one epoch (each `$derived` re-eval would otherwise
// fire a fresh query) while still refetching when a new readback lands.
const fetchedEpoch = new Map<number, number>();

/** Get a thumbnail as a data URL for any node id (raster layer or modifier).
 *  Returns empty string when no cached bytes exist yet; the cache fills in
 *  asynchronously and the caller's `$derived` re-runs when it does. */
export function getNodeThumbnail(nodeId: number): string {
    // Subscribe to `engineState.thumbnailVersion` so any `$derived` calling this
    // function re-runs when an async readback lands in the wasm cache, and so we
    // refetch the new bytes. (See `app.svelte.ts` `requestFrame` — render's
    // returned state mirror carries the version.)
    const epoch = app.engineState?.thumbnailVersion ?? 0;
    const engine = app.engine;
    if (!engine) return '';

    if (fetchedEpoch.get(nodeId) !== epoch) {
        fetchedEpoch.set(nodeId, epoch);
        engine
            .api.nodeThumbnail({
                node_id: nodeId,
                width: THUMB_SIZE,
                height: THUMB_SIZE,
            })
            .then(({ bytes }) => {
                // No bytes yet (readback not landed): keep any prior URL so the
                // thumbnail doesn't flicker to the placeholder between epochs.
                if (!bytes || bytes.length === 0) return;
                cache[nodeId] = rgbaToDataUrl(bytes, THUMB_SIZE, THUMB_SIZE);
            })
            .catch(() => {});
    }

    return cache[nodeId] ?? '';
}

export { THUMB_SIZE };
