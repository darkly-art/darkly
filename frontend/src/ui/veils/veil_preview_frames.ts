//! Payload conversion for veil picker previews.
//!
//! Split out from `VeilPreview.svelte` so the polling/conversion logic is
//! unit-testable without mounting a Svelte component (and driving rAF/canvas).
//! Previews are rendered by the engine over the *current canvas* and are **not**
//! cached — each time the picker opens, frames are regenerated, so the preview
//! always reflects the live document.

import type { Engine } from '../../engine/protocol';

/** Engine `poll_veil_preview` response: all frames concatenated into a single
 *  `bytes` buffer (stride = width*height*4), sliced into `frameCount` frames. */
export interface RawVeilPreview {
    width: number;
    height: number;
    fps: number;
    frameCount: number;
    bytes: Uint8Array;
}

/** Replay-ready preview: each frame pre-converted to `ImageData`. */
export interface PreviewData {
    width: number;
    height: number;
    fps: number;
    frames: ImageData[];
}

/** Convert a raw WASM payload into replay-ready `ImageData` frames. Slices the
 *  concatenated `bytes` buffer into `frameCount` per-frame views (stride =
 *  width*height*4) and wraps each in an `ImageData`. */
export function toPreviewData(raw: RawVeilPreview): PreviewData {
    const stride = raw.width * raw.height * 4;
    const frames: ImageData[] = [];
    for (let i = 0; i < raw.frameCount; i++) {
        const slice = raw.bytes.subarray(i * stride, (i + 1) * stride);
        frames.push(new ImageData(new Uint8ClampedArray(slice), raw.width, raw.height));
    }
    return { width: raw.width, height: raw.height, fps: raw.fps, frames };
}

/** Poll the engine for a veil preview. Returns converted frames once the
 *  generation completes, or `null` while it's still rendering. No caching — the
 *  caller polls until frames arrive, then stops on its own. */
export async function pollPreview(
    engine: Engine,
    veilType: string,
): Promise<PreviewData | null> {
    const raw = (await engine.send('poll_veil_preview', { veil_type: veilType })) as
        | RawVeilPreview
        | null
        | undefined;
    if (!raw || !raw.bytes || raw.frameCount === 0) return null;
    return toPreviewData(raw);
}
