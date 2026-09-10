import { describe, it, expect, beforeEach, vi } from 'vitest';
import { DarklyInstance } from '../app.svelte';

let inst: DarklyInstance;
beforeEach(() => {
    inst = new DarklyInstance();
    // `requestFrame` schedules a rAF + touches the engine; neither exists in the
    // node test env. Stub it; we only assert the view-state mutation.
    inst.requestFrame = vi.fn();
});

describe('fitZoom', () => {
    it('returns min(viewport/doc ratio, 1) and never upscales', () => {
        inst.docW = 200; inst.docH = 100;
        inst.viewportW = 100; inst.viewportH = 100;
        // Width-bound: 100/200 = 0.5 is the limiting ratio.
        expect(inst.fitZoom()).toBeCloseTo(0.5, 9);
    });

    it('clamps to 1 when the doc is smaller than the viewport', () => {
        inst.docW = 50; inst.docH = 50;
        inst.viewportW = 1000; inst.viewportH = 1000;
        expect(inst.fitZoom()).toBe(1);
    });

    it('upscales past 1 when allowUpscale is set', () => {
        inst.docW = 50; inst.docH = 50;
        inst.viewportW = 1000; inst.viewportH = 1000;
        expect(inst.fitZoom(true)).toBeCloseTo(20, 9);
    });

    it('fits the rotated bounding box, not the unrotated doc', () => {
        // A square rotated 45° has a bounding box √2× its side, so the fit
        // zoom must shrink by 1/√2 relative to the unrotated fit.
        inst.docW = 100; inst.docH = 100;
        inst.viewportW = 100; inst.viewportH = 100;
        inst.rotation = Math.PI / 4;
        expect(inst.fitZoom(true)).toBeCloseTo(1 / Math.SQRT2, 9);
    });
});

describe('fitToScreen', () => {
    it('recenters and zooms to fill, preserving rotation and mirror', () => {
        inst.docW = 50; inst.docH = 50;
        inst.viewportW = 1000; inst.viewportH = 1000;
        inst.panX = 37; inst.panY = -12;
        inst.rotation = 1.234;
        inst.mirrorH = true;

        inst.fitToScreen();

        expect(inst.panX).toBe(0);
        expect(inst.panY).toBe(0);
        // Orientation is untouched: this is framing, not a reset.
        expect(inst.rotation).toBe(1.234);
        expect(inst.mirrorH).toBe(true);
        // Small doc enlarges past 1:1 to fill the viewport.
        expect(inst.zoom).toBeGreaterThan(1);
        expect(inst.requestFrame).toHaveBeenCalled();
    });
});

describe('centerView', () => {
    it('zeros pan only, leaving zoom/rotation/mirror untouched', () => {
        inst.panX = 37; inst.panY = -12;
        inst.zoom = 3;
        inst.rotation = 1.234;
        inst.mirrorH = true;

        inst.centerView();

        expect(inst.panX).toBe(0);
        expect(inst.panY).toBe(0);
        expect(inst.zoom).toBe(3);
        expect(inst.rotation).toBe(1.234);
        expect(inst.mirrorH).toBe(true);
        expect(inst.requestFrame).toHaveBeenCalled();
    });
});

describe('resetView', () => {
    it('zeros pan/rotation, clears mirror, and zooms to fit', () => {
        inst.docW = 200; inst.docH = 100;
        inst.viewportW = 100; inst.viewportH = 100;
        inst.panX = 37; inst.panY = -12;
        inst.rotation = 1.234;
        inst.mirrorH = true;
        inst.zoom = 4;

        inst.resetView();

        expect(inst.panX).toBe(0);
        expect(inst.panY).toBe(0);
        expect(inst.rotation).toBe(0);
        expect(inst.mirrorH).toBe(false);
        expect(inst.zoom).toBeCloseTo(0.5, 9);
        expect(inst.requestFrame).toHaveBeenCalled();
    });
});
