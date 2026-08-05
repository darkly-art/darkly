//! Payload conversion for picker previews.
//!
//! Split out from `EffectPreview.svelte` so the polling/conversion logic is
//! unit-testable without mounting a Svelte component (and driving rAF/canvas).
//! Previews are rendered by the engine — effects that read a source over the
//! *current canvas*, the rest from scratch — and are **not** cached: each time
//! the picker opens, frames are regenerated, so the preview always reflects the
//! live document.
//!
//! Every entry has two: a `still` the card shows at rest, and an `animated`
//! sequence it asks for when the pointer arrives. They are separate generations
//! and are polled separately, so a card never waits on a sequence to show
//! something.

import type { Engine } from '../engine/protocol';
import type { PreviewVariant } from '../engine/protocol_gen';

export type { PreviewVariant };

/** Engine `poll_preview` response: all frames concatenated into a single
 *  `bytes` buffer (stride = width*height*4), sliced into `frameCount` frames. */
export interface RawPreview {
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
export function toPreviewData(raw: RawPreview): PreviewData {
    const stride = raw.width * raw.height * 4;
    const frames: ImageData[] = [];
    for (let i = 0; i < raw.frameCount; i++) {
        const slice = raw.bytes.subarray(i * stride, (i + 1) * stride);
        frames.push(new ImageData(new Uint8ClampedArray(slice), raw.width, raw.height));
    }
    return { width: raw.width, height: raw.height, fps: raw.fps, frames };
}

/** Whether a picker should render a live thumbnail for this entry, versus
 *  falling back to its icon. Type-owned: the entry declares a `PreviewAnim` on
 *  its registration and `supportsPreview` reports whether it did, so a picker
 *  asks here rather than branching on the catalog or the kind. */
export function showsPreview(entry: { supportsPreview?: boolean }): boolean {
    return entry.supportsPreview === true;
}

/** Poll the engine for one variant of `catalog`/`type`'s preview. Returns
 *  converted frames once that generation completes, or `null` while it's still
 *  rendering. No caching — the caller polls until frames arrive, then stops on
 *  its own. */
export async function pollPreview(
    engine: Engine,
    catalog: string,
    type: string,
    variant: PreviewVariant,
): Promise<PreviewData | null> {
    const raw = (await engine.api.pollPreview({ catalog, type, variant })) as
        | RawPreview
        | null
        | undefined;
    if (!raw || !raw.bytes || raw.frameCount === 0) return null;
    return toPreviewData(raw);
}
