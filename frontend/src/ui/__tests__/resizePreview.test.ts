import { describe, it, expect } from 'vitest';
import {
    clampDim,
    rectFromAnchor,
    matchedAnchor,
    applyDrag,
    computeFit,
    toPreview,
    toContent,
    MAX_DIM,
    type Rect,
} from '../resizePreview';

describe('clampDim', () => {
    it('rounds and clamps to [1, MAX_DIM]', () => {
        expect(clampDim(12.4)).toBe(12);
        expect(clampDim(12.6)).toBe(13);
        expect(clampDim(0)).toBe(1);
        expect(clampDim(-5)).toBe(1);
        expect(clampDim(MAX_DIM + 100)).toBe(MAX_DIM);
    });
});

describe('rectFromAnchor', () => {
    const docW = 100;
    const docH = 100;
    it('top-left anchor keeps the origin when growing', () => {
        expect(rectFromAnchor(docW, docH, 140, 160, 0, 0)).toEqual({ x: 0, y: 0, w: 140, h: 160 });
    });
    it('center anchor splits the delta', () => {
        expect(rectFromAnchor(docW, docH, 140, 160, 0.5, 0.5)).toEqual({
            x: -20,
            y: -30,
            w: 140,
            h: 160,
        });
    });
    it('bottom-right anchor takes the whole delta off the top/left', () => {
        expect(rectFromAnchor(docW, docH, 140, 160, 1, 1)).toEqual({
            x: -40,
            y: -60,
            w: 140,
            h: 160,
        });
    });
    it('center anchor shrink pulls the origin inward', () => {
        expect(rectFromAnchor(docW, docH, 60, 40, 0.5, 0.5)).toEqual({ x: 20, y: 30, w: 60, h: 40 });
    });
});

describe('matchedAnchor', () => {
    const docW = 100;
    const docH = 100;
    it('recognizes a centered rect', () => {
        const r = rectFromAnchor(docW, docH, 140, 160, 0.5, 0.5);
        expect(matchedAnchor(docW, docH, r)).toEqual({ ax: 0.5, ay: 0.5 });
    });
    it('recognizes a top-left grow on both axes', () => {
        const r = rectFromAnchor(docW, docH, 140, 160, 0, 0);
        expect(matchedAnchor(docW, docH, r)).toEqual({ ax: 0, ay: 0 });
    });
    it('returns null for an off-anchor offset', () => {
        const r: Rect = { x: -7, y: -30, w: 140, h: 160 };
        expect(matchedAnchor(docW, docH, r).ax).toBeNull();
        expect(matchedAnchor(docW, docH, r).ay).toBe(0.5);
    });
    it('is ambiguous (null) when a dimension is unchanged', () => {
        const r: Rect = { x: 0, y: -30, w: 100, h: 160 };
        expect(matchedAnchor(docW, docH, r).ax).toBeNull();
    });
});

describe('applyDrag', () => {
    const start: Rect = { x: 0, y: 0, w: 100, h: 80 };

    it('right edge grows width, origin fixed', () => {
        expect(applyDrag(start, 'e', 20, 999)).toEqual({ x: 0, y: 0, w: 120, h: 80 });
    });
    it('left edge moves the origin and shrinks width, right edge fixed', () => {
        expect(applyDrag(start, 'w', 15, 0)).toEqual({ x: 15, y: 0, w: 85, h: 80 });
    });
    it('top edge moves origin.y, bottom fixed', () => {
        expect(applyDrag(start, 'n', 0, -10)).toEqual({ x: 0, y: -10, w: 100, h: 90 });
    });
    it('se corner grows both, top-left fixed', () => {
        expect(applyDrag(start, 'se', 20, 30)).toEqual({ x: 0, y: 0, w: 120, h: 110 });
    });
    it('nw corner moves origin and resizes both, bottom-right fixed', () => {
        expect(applyDrag(start, 'nw', 10, 10)).toEqual({ x: 10, y: 10, w: 90, h: 70 });
    });
    it('body translates without resizing', () => {
        expect(applyDrag(start, 'body', 25, -15)).toEqual({ x: 25, y: -15, w: 100, h: 80 });
    });
    it('clamps to a minimum size when dragging an edge past the opposite one', () => {
        const r = applyDrag(start, 'e', -500, 0);
        expect(r.w).toBe(1);
        expect(r.x).toBe(0);
    });
    it('locks aspect ratio on a corner when requested', () => {
        // start ratio 100:80 = 1.25. Drag se by +40 wide; height follows ratio.
        const r = applyDrag(start, 'se', 40, 0, true);
        expect(r.w).toBe(140);
        expect(r.h).toBe(Math.round(140 / 1.25)); // 112
    });
});

describe('computeFit / round-trip', () => {
    it('maps content corners inside the preview box and inverts', () => {
        const docW = 200;
        const docH = 100;
        const rect: Rect = { x: -20, y: -10, w: 240, h: 120 };
        const fit = computeFit(docW, docH, rect, 380, 380);

        // Inverse of forward is identity for arbitrary points.
        for (const [cx, cy] of [
            [0, 0],
            [docW, docH],
            [rect.x, rect.y],
            [50, 50],
        ]) {
            const [px, py] = toPreview(fit, cx, cy);
            const [bx, by] = toContent(fit, px, py);
            expect(bx).toBeCloseTo(cx, 5);
            expect(by).toBeCloseTo(cy, 5);
        }

        // All union corners land within the preview box.
        for (const [cx, cy] of [
            [rect.x, rect.y],
            [rect.x + rect.w, rect.y + rect.h],
        ]) {
            const [px, py] = toPreview(fit, cx, cy);
            expect(px).toBeGreaterThanOrEqual(0);
            expect(px).toBeLessThanOrEqual(380);
            expect(py).toBeGreaterThanOrEqual(0);
            expect(py).toBeLessThanOrEqual(380);
        }
    });
});
