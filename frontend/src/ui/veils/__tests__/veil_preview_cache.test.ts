import { describe, it, expect, beforeEach, vi } from 'vitest';

// vitest's default node environment has no DOM globals. The cache module only
// uses `ImageData` as a plain RGBA frame container, so a minimal stand-in is
// enough — avoids pulling in jsdom.
class FakeImageData {
    data: Uint8ClampedArray;
    width: number;
    height: number;
    constructor(data: Uint8ClampedArray, width: number, height: number) {
        this.data = data;
        this.width = width;
        this.height = height;
    }
}
(globalThis as unknown as { ImageData: unknown }).ImageData ??= FakeImageData;

import {
    getOrStartPreview,
    pollPreview,
    toPreviewData,
    _clearPreviewCache,
    type RawVeilPreview,
} from '../veil_preview_cache';

/** A fake `DarklyHandle` surface: records `start_veil_preview` calls and hands
 *  back a scripted `poll_veil_preview` payload (null until "ready"). */
function fakeHandle(payload: RawVeilPreview | null) {
    const start = vi.fn((_veilType: string) => {});
    const poll = vi.fn((_veilType: string) => payload);
    return {
        start,
        poll,
        start_veil_preview(veilType: string) {
            start(veilType);
        },
        poll_veil_preview(veilType: string) {
            return poll(veilType);
        },
    };
}

function rawPreview(frameCount: number, w = 2, h = 2): RawVeilPreview {
    const frames = Array.from(
        { length: frameCount },
        (_, i) => new Uint8Array(w * h * 4).fill(i),
    );
    return { width: w, height: h, fps: 24, frames };
}

beforeEach(() => {
    _clearPreviewCache();
});

describe('toPreviewData', () => {
    it('converts each raw RGBA buffer into an ImageData of the right size', () => {
        const data = toPreviewData(rawPreview(3, 4, 2));
        expect(data.frames).toHaveLength(3);
        expect(data.frames[0]).toBeInstanceOf(ImageData);
        expect(data.frames[0].width).toBe(4);
        expect(data.frames[0].height).toBe(2);
        expect(data.fps).toBe(24);
    });
});

describe('getOrStartPreview', () => {
    it('kicks off generation and returns null on a cold cache', () => {
        const handle = fakeHandle(null);
        const result = getOrStartPreview(handle, 'vhs');
        expect(result).toBeNull();
        expect(handle.start).toHaveBeenCalledOnce();
        expect(handle.start).toHaveBeenCalledWith('vhs');
    });

    it('returns the cache without re-requesting once the preview is ready', () => {
        const handle = fakeHandle(rawPreview(4));
        // Cold: starts generation.
        expect(getOrStartPreview(handle, 'vhs')).toBeNull();
        // Poll lands the frames and caches them.
        const polled = pollPreview(handle, 'vhs');
        expect(polled?.frames).toHaveLength(4);

        // Second mount: served from cache, no new start_veil_preview call.
        const cached = getOrStartPreview(handle, 'vhs');
        expect(cached).toBe(polled);
        expect(handle.start).toHaveBeenCalledOnce();
    });
});

describe('pollPreview', () => {
    it('returns null while the engine is still generating', () => {
        const handle = fakeHandle(null);
        expect(pollPreview(handle, 'grain')).toBeNull();
    });

    it('caches on first completion and stops polling the handle afterwards', () => {
        const handle = fakeHandle(rawPreview(2));
        const first = pollPreview(handle, 'grain');
        expect(first?.frames).toHaveLength(2);
        expect(handle.poll).toHaveBeenCalledOnce();

        // Cached: handle is not polled again.
        const second = pollPreview(handle, 'grain');
        expect(second).toBe(first);
        expect(handle.poll).toHaveBeenCalledOnce();
    });
});
