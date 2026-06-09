import { describe, it, expect, vi } from 'vitest';
import { pollPreview, toPreviewData, type RawVeilPreview } from '../veil_preview_frames';

// vitest's default node environment has no DOM globals. The conversion only
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

function rawPreview(frameCount: number, w = 2, h = 2): RawVeilPreview {
    const frames = Array.from(
        { length: frameCount },
        (_, i) => new Uint8Array(w * h * 4).fill(i),
    );
    return { width: w, height: h, fps: 24, frames };
}

/** Fake handle whose `poll_veil_preview` returns the scripted payload. */
function fakeHandle(payload: RawVeilPreview | null) {
    const poll = vi.fn((_veilType: string) => payload);
    return {
        poll,
        start_veil_preview: vi.fn((_veilType: string) => {}),
        poll_veil_preview(veilType: string) {
            return poll(veilType);
        },
    };
}

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

describe('pollPreview', () => {
    it('returns null while the engine is still generating', () => {
        const handle = fakeHandle(null);
        expect(pollPreview(handle, 'grain')).toBeNull();
    });

    it('returns null for an empty frame set', () => {
        const handle = fakeHandle({ width: 2, height: 2, fps: 24, frames: [] });
        expect(pollPreview(handle, 'grain')).toBeNull();
    });

    it('converts the frames once the generation completes', () => {
        const handle = fakeHandle(rawPreview(4, 8, 4));
        const data = pollPreview(handle, 'vhs');
        expect(data?.frames).toHaveLength(4);
        expect(data?.width).toBe(8);
        expect(data?.height).toBe(4);
    });

    it('re-polls the engine each call (no caching)', () => {
        const handle = fakeHandle(rawPreview(2));
        pollPreview(handle, 'vhs');
        pollPreview(handle, 'vhs');
        // Unlike the old cached path, every call hits the engine — the preview
        // tracks the live canvas, so results are never memoised.
        expect(handle.poll).toHaveBeenCalledTimes(2);
    });
});
