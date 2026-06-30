import { describe, it, expect, beforeEach, vi } from 'vitest';
import { DarklyInstance } from '../app.svelte';

let inst: DarklyInstance;
beforeEach(() => {
    inst = new DarklyInstance();
    // `requestFrame` schedules a rAF + touches the engine; neither exists in the
    // node test env. Stub it — we only assert the view-state mutation.
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
