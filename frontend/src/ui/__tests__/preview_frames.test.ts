import { describe, it, expect, vi } from 'vitest';
import {
    pollPreview,
    showsPreview,
    toPreviewData,
    type RawPreview,
} from '../preview_frames';
import type { Engine } from '../../engine/protocol';
import { withApi } from '../../engine/testApi';

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
function rawPreview(frameCount: number, w = 2, h = 2): RawPreview {
    const stride = w * h * 4;
    const bytes = new Uint8Array(stride * frameCount);
    for (let i = 0; i < frameCount; i++) {
        bytes.fill(i, i * stride, (i + 1) * stride);
    }
    return { width: w, height: h, fps: 24, frameCount, bytes };
}

/** Fake engine whose `send('poll_preview', …)` returns the scripted payload.
 *  Captures the payload so tests can assert the `{ catalog, type }` wire shape. */
function fakeEngine(payload: RawPreview | null) {
    const send = vi.fn(async (_kind: string, _payload: unknown) => payload);
    const engine = withApi({ send, post: vi.fn() }) as unknown as Engine;
    return { engine, send };
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
        expect(await pollPreview(engine, 'veils', 'grain')).toBeNull();
    });

    it('returns null for an empty frame set', async () => {
        const { engine } = fakeEngine({
            width: 2,
            height: 2,
            fps: 24,
            frameCount: 0,
            bytes: new Uint8Array(0),
        });
        expect(await pollPreview(engine, 'veils', 'grain')).toBeNull();
    });

    it('converts the frames once the generation completes', async () => {
        const { engine } = fakeEngine(rawPreview(4, 8, 4));
        const data = await pollPreview(engine, 'veils', 'vhs');
        expect(data?.frames).toHaveLength(4);
        expect(data?.width).toBe(8);
        expect(data?.height).toBe(4);
    });

    it('sends the generic poll_preview with { catalog, type } for every catalog', async () => {
        const { engine, send } = fakeEngine(rawPreview(1));
        // Catalog ids, not a second vocabulary — the same strings the pickers
        // already hold and `catalogs()` publishes.
        for (const catalog of ['veils', 'voids', 'filters']) {
            await pollPreview(engine, catalog, 'noise');
        }
        expect(send).toHaveBeenNthCalledWith(1, 'poll_preview', { catalog: 'veils', type: 'noise' });
        expect(send).toHaveBeenNthCalledWith(2, 'poll_preview', { catalog: 'voids', type: 'noise' });
        expect(send).toHaveBeenNthCalledWith(3, 'poll_preview', {
            catalog: 'filters',
            type: 'noise',
        });
    });

    it('re-polls the engine each call (no caching)', async () => {
        const { engine, send } = fakeEngine(rawPreview(2));
        await pollPreview(engine, 'voids', 'noise');
        await pollPreview(engine, 'voids', 'noise');
        // Unlike a cached path, every call hits the engine — the preview tracks
        // the live document, so results are never memoised.
        expect(send).toHaveBeenCalledTimes(2);
    });
});

describe('showsPreview', () => {
    // Every picker renders a live thumbnail when the entry opts into a rendered
    // preview, and falls back otherwise. This is the predicate that drives the
    // first arm of `EffectPreview`'s chain, whatever the catalog.
    it('is true only when the entry declares supportsPreview', () => {
        // A filter, a veil and a void entry are the same shape to this
        // predicate — that is the point of generalising it.
        expect(showsPreview({ supportsPreview: true })).toBe(true);
        expect(showsPreview({ supportsPreview: false })).toBe(false);
        // Missing flag is treated as no preview, which is what sends the card
        // down the icon → named-placeholder half of the chain.
        expect(showsPreview({})).toBe(false);
    });
});
