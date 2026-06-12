//! Payload conversion for veil picker previews.
//!
//! Split out from `VeilPreview.svelte` so the polling/conversion logic is
//! unit-testable without mounting a Svelte component (and driving rAF/canvas).
//! Previews are rendered by the engine over the *current canvas* and are **not**
//! cached — each time the picker opens, frames are regenerated, so the preview
//! always reflects the live document.

/** Raw payload returned by `handle.poll_veil_preview`. */
export interface RawVeilPreview {
    width: number;
    height: number;
    fps: number;
    frames: Uint8Array[];
}

/** Replay-ready preview: each frame pre-converted to `ImageData`. */
export interface PreviewData {
    width: number;
    height: number;
    fps: number;
    frames: ImageData[];
}

/** Minimal surface of `DarklyHandle` this module needs — keeps it decoupled
 *  from the generated WASM types and trivial to fake in tests. */
export interface VeilPreviewHandle {
    start_veil_preview(veilType: string): void;
    poll_veil_preview(veilType: string): unknown;
}

/** Convert a raw WASM payload into replay-ready `ImageData` frames. */
export function toPreviewData(raw: RawVeilPreview): PreviewData {
    const frames = raw.frames.map(
        (buf) => new ImageData(new Uint8ClampedArray(buf), raw.width, raw.height),
    );
    return { width: raw.width, height: raw.height, fps: raw.fps, frames };
}

/** Poll the engine for a veil preview. Returns converted frames once the
 *  generation completes, or `null` while it's still rendering. No caching — the
 *  caller polls until frames arrive, then stops on its own. */
export function pollPreview(handle: VeilPreviewHandle, veilType: string): PreviewData | null {
    const raw = handle.poll_veil_preview(veilType) as RawVeilPreview | null | undefined;
    if (!raw || !raw.frames || raw.frames.length === 0) return null;
    return toPreviewData(raw);
}
