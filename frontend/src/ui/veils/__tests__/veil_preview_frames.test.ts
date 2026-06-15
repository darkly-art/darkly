import { describe, it, expect, vi } from 'vitest';
import { pollPreview, toPreviewData, type RawVeilPreview } from '../veil_preview_frames';
import type { Engine } from '../../../engine/protocol';

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

// The engine concatenates all frames into a single `bytes` buffer; the helper
// slices it back into `frameCount` per-frame views (stride = w*h*4). Build a
// buffer whose frame i is filled with the value i to verify slicing.
function rawPreview(frameCount: number, w = 2, h = 2): RawVeilPreview {
    const stride = w * h * 4;
    const bytes = new Uint8Array(stride * frameCount);
    for (let i = 0; i < frameCount; i++) {
        bytes.fill(i, i * stride, (i + 1) * stride);
    }
    return { width: w, height: h, fps: 24, frameCount, bytes };
}

/** Fake engine whose `send('poll_veil_preview', …)` returns the scripted
 *  payload. Only the `send` surface `pollPreview` touches is implemented. */
function fakeEngine(payload: RawVeilPreview | null) {
    const send = vi.fn(async (_kind: string, _payload: unknown) => payload);
    return { engine: { send } as unknown as Engine, send };
}

describe('toPreviewData', () => {
    it('slices the concatenated buffer into ImageData frames of the right size', () => {
        const data = toPreviewData(rawPreview(3, 4, 2));
        expect(data.frames).toHaveLength(3);
        expect(data.frames[0]).toBeInstanceOf(ImageData);
        expect(data.frames[0].width).toBe(4);
        expect(data.frames[0].height).toBe(2);
        expect(data.fps).toBe(24);
        // Frame 2 was filled with the value 2.
        expect(data.frames[2].data[0]).toBe(2);
    });
});

describe('pollPreview', () => {
    it('returns null while the engine is still generating', async () => {
        const { engine } = fakeEngine(null);
        expect(await pollPreview(engine, 'grain')).toBeNull();
    });

    it('returns null for an empty frame set', async () => {
        const { engine } = fakeEngine({
            width: 2,
            height: 2,
            fps: 24,
            frameCount: 0,
            bytes: new Uint8Array(0),
        });
        expect(await pollPreview(engine, 'grain')).toBeNull();
    });

    it('converts the frames once the generation completes', async () => {
        const { engine } = fakeEngine(rawPreview(4, 8, 4));
        const data = await pollPreview(engine, 'vhs');
        expect(data?.frames).toHaveLength(4);
        expect(data?.width).toBe(8);
        expect(data?.height).toBe(4);
    });

    it('re-polls the engine each call (no caching)', async () => {
        const { engine, send } = fakeEngine(rawPreview(2));
        await pollPreview(engine, 'vhs');
        await pollPreview(engine, 'vhs');
        // Unlike the old cached path, every call hits the engine — the preview
        // tracks the live canvas, so results are never memoised.
        expect(send).toHaveBeenCalledTimes(2);
    });
});
