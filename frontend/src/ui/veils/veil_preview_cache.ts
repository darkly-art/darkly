//! Session cache + payload conversion for veil picker previews.
//!
//! Split out from `VeilPreview.svelte` so the cache/no-re-request logic is
//! unit-testable without mounting a Svelte component (and driving rAF/canvas).
//! The engine renders each veil's preview frames once per session; this module
//! converts the raw WASM payload into replayable `ImageData` and caches it.
//! In-memory only — frames are cheap to regenerate and must not outlive a build
//! whose veil shaders may have changed.

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

const previewCache = new Map<string, PreviewData>();

/** Convert a raw WASM payload into replay-ready `ImageData` frames. */
export function toPreviewData(raw: RawVeilPreview): PreviewData {
    const frames = raw.frames.map(
        (buf) => new ImageData(new Uint8ClampedArray(buf), raw.width, raw.height),
    );
    return { width: raw.width, height: raw.height, fps: raw.fps, frames };
}

/** Return the cached preview if present; otherwise kick off generation on the
 *  engine and return `null`. Idempotent on the engine side — `start_veil_preview`
 *  is a no-op once frames are cached or in flight. */
export function getOrStartPreview(
    handle: VeilPreviewHandle,
    veilType: string,
): PreviewData | null {
    const cached = previewCache.get(veilType);
    if (cached) return cached;
    handle.start_veil_preview(veilType);
    return null;
}

/** Poll the engine for completion. On the first successful poll, converts and
 *  caches the frames; subsequent calls return the cache. `null` while pending. */
export function pollPreview(handle: VeilPreviewHandle, veilType: string): PreviewData | null {
    const cached = previewCache.get(veilType);
    if (cached) return cached;
    const raw = handle.poll_veil_preview(veilType) as RawVeilPreview | null | undefined;
    if (!raw) return null;
    const data = toPreviewData(raw);
    previewCache.set(veilType, data);
    return data;
}

/** Test-only: clear the session cache between cases. */
export function _clearPreviewCache(): void {
    previewCache.clear();
}
