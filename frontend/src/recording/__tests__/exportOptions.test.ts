import { describe, it, expect } from 'vitest';
import {
    EXPORT_MAX_DIM,
    aspectLabel,
    canPassthrough,
    clampFps,
    computeDrawRect,
    gifDelayMs,
    groupSegmentsByAspect,
    lockedDims,
} from '../exportOptions';
import type { SegmentMeta } from '../segments';

function meta(overrides: Partial<SegmentMeta> = {}): SegmentMeta {
    return {
        n: 1,
        codec: 'avc1.640028',
        width: 1920,
        height: 1080,
        canvasWidth: 1920,
        canvasHeight: 1080,
        frameCount: 10,
        description: 'YXZjQw==',
        ...overrides,
    };
}

describe('computeDrawRect', () => {
    it('stretch covers the whole canvas regardless of source aspect', () => {
        expect(computeDrawRect('stretch', 100, 200, 400, 300)).toEqual({
            x: 0,
            y: 0,
            w: 400,
            h: 300,
        });
    });

    it('fit letterboxes a wide source into a square canvas', () => {
        // 200×100 into 100×100: scaled to 100×50, centered vertically.
        expect(computeDrawRect('fit', 200, 100, 100, 100)).toEqual({ x: 0, y: 25, w: 100, h: 50 });
    });

    it('fill center-crops a wide source into a square canvas', () => {
        // 200×100 into 100×100: scaled to 200×100, overflowing horizontally.
        expect(computeDrawRect('fill', 200, 100, 100, 100)).toEqual({
            x: -50,
            y: 0,
            w: 200,
            h: 100,
        });
    });

    it('a source matching the output aspect fills exactly under all methods', () => {
        for (const method of ['stretch', 'fit', 'fill'] as const) {
            expect(computeDrawRect(method, 960, 540, 1920, 1080)).toEqual({
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            });
        }
    });
});

describe('groupSegmentsByAspect', () => {
    it('groups by exact reduced canvas ratio, in order of first appearance', () => {
        const { groups, defaultIndex } = groupSegmentsByAspect([
            meta({ n: 1, canvasWidth: 1920, canvasHeight: 1080, frameCount: 5 }),
            meta({
                n: 2,
                canvasWidth: 1000,
                canvasHeight: 1000,
                width: 1000,
                height: 1000,
                frameCount: 3,
            }),
            // 960×540 reduces to 16:9 like segment 1 — same group.
            meta({
                n: 3,
                canvasWidth: 960,
                canvasHeight: 540,
                width: 960,
                height: 540,
                frameCount: 2,
            }),
        ]);
        expect(groups).toHaveLength(2);
        expect(groups[0]).toMatchObject({ arW: 16, arH: 9, label: '16:9', frameCount: 7 });
        expect(groups[1]).toMatchObject({ arW: 1, arH: 1, label: '1:1', frameCount: 3 });
        // Default is the LAST segment's group — the document's final shape.
        expect(defaultIndex).toBe(0);
    });

    it('offers the largest segment in a group as the native resolution', () => {
        const { groups } = groupSegmentsByAspect([
            meta({ canvasWidth: 960, canvasHeight: 540, width: 960, height: 540 }),
            meta({ canvasWidth: 1920, canvasHeight: 1080, width: 1920, height: 1080 }),
        ]);
        expect(groups[0]).toMatchObject({ nativeWidth: 1920, nativeHeight: 1080 });
    });

    it('defaults to the trailing group', () => {
        const { defaultIndex } = groupSegmentsByAspect([
            meta({ canvasWidth: 1920, canvasHeight: 1080 }),
            meta({ canvasWidth: 1000, canvasHeight: 1000 }),
        ]);
        expect(defaultIndex).toBe(1);
    });
});

describe('aspectLabel', () => {
    it('names common ratios and their inverses', () => {
        expect(aspectLabel(16, 9)).toBe('16:9');
        expect(aspectLabel(9, 16)).toBe('9:16');
        expect(aspectLabel(1, 1)).toBe('1:1');
        expect(aspectLabel(4, 3)).toBe('4:3');
    });

    it('snaps near-misses within 1% to the named ratio', () => {
        // 1921×1080 → reduced 1921:1080, 0.05% off 16:9.
        expect(aspectLabel(1921, 1080)).toBe('16:9');
    });

    it('shows small exact terms verbatim', () => {
        expect(aspectLabel(5, 4)).toBe('5:4');
    });

    it('falls back to a decimal for awkward fractions', () => {
        expect(aspectLabel(1000, 541)).toBe('1.85:1');
        expect(aspectLabel(541, 1000)).toBe('1:1.85');
    });
});

describe('lockedDims', () => {
    it('derives the other axis from the exact aspect fraction', () => {
        expect(lockedDims('w', 1920, 16, 9)).toEqual({ width: 1920, height: 1080 });
        expect(lockedDims('h', 1080, 16, 9)).toEqual({ width: 1920, height: 1080 });
    });

    it('even-aligns both axes', () => {
        const { width, height } = lockedDims('w', 333, 16, 9);
        expect(width % 2).toBe(0);
        expect(height % 2).toBe(0);
        expect(width).toBe(332);
    });

    it('clamps to the export range', () => {
        expect(lockedDims('w', 100000, 1, 1)).toEqual({
            width: EXPORT_MAX_DIM,
            height: EXPORT_MAX_DIM,
        });
        expect(lockedDims('w', 0, 1, 1)).toEqual({ width: 2, height: 2 });
        expect(lockedDims('h', NaN, 1, 1)).toEqual({ width: 2, height: 2 });
    });
});

describe('clampFps', () => {
    it('clamps to [1, 120] and allows fractions', () => {
        expect(clampFps(0)).toBe(1);
        expect(clampFps(0.25)).toBe(1);
        expect(clampFps(1)).toBe(1);
        expect(clampFps(23.976)).toBe(23.976);
        expect(clampFps(500)).toBe(120);
    });

    it('falls back to 30 on garbage', () => {
        expect(clampFps(NaN)).toBe(30);
        expect(clampFps(Infinity)).toBe(30);
    });
});

describe('gifDelayMs', () => {
    it('is the frame period above the 20ms floor', () => {
        expect(gifDelayMs(10)).toBe(100);
        expect(gifDelayMs(1)).toBe(1000);
    });

    it('floors at 20ms (browser sub-20ms snapping)', () => {
        expect(gifDelayMs(60)).toBe(20);
        expect(gifDelayMs(120)).toBe(20);
    });
});

describe('canPassthrough', () => {
    it('accepts compatible segments at their native resolution', () => {
        expect(canPassthrough([meta({ n: 1 }), meta({ n: 2 })], 1920, 1080)).toBe(true);
    });

    it('rejects any decoder-config mismatch', () => {
        expect(
            canPassthrough([meta(), meta({ codec: 'vp09.00.10.08', description: undefined })], 1920, 1080),
        ).toBe(false);
    });

    it('rejects an output resolution differing from the packets', () => {
        expect(canPassthrough([meta()], 1280, 720)).toBe(false);
    });

    it('rejects an empty recording', () => {
        expect(canPassthrough([], 1920, 1080)).toBe(false);
    });
});
