import { describe, it, expect } from 'vitest';
import type { Rect } from '../../../lib/edges';
import { isRow, offsetForPos, peekOut, placementFromPointer, stripBox, stripPos } from '../geometry';

const region: Rect = { left: 100, top: 50, width: 1000, height: 600 };

describe('isRow', () => {
    it('is true for the horizontal edges only', () => {
        expect(isRow('top')).toBe(true);
        expect(isRow('bottom')).toBe(true);
        expect(isRow('left')).toBe(false);
        expect(isRow('right')).toBe(false);
    });
});

describe('stripPos', () => {
    it('maps 0 / 0.5 / 1 to flush-start, centred and flush-end', () => {
        expect(stripPos(0, 600, 300)).toBe(0);
        expect(stripPos(0.5, 600, 300)).toBe(150);
        expect(stripPos(1, 600, 300)).toBe(300);
    });

    // The offset is a fraction of travel, not of the edge, so it is NOT
    // region-proportional at any value but 0, 0.5 and 1. This is the containment
    // property that fraction-of-travel buys instead.
    it('never puts any part of the strip outside the region, at any length', () => {
        for (const regionLen of [200, 350, 1000, 4000]) {
            for (const offset of [0, 0.25, 0.5, 0.75, 1]) {
                const pos = stripPos(offset, regionLen, 300);
                expect(pos).toBeGreaterThanOrEqual(0);
                expect(pos + 300).toBeLessThanOrEqual(Math.max(regionLen, 300));
            }
        }
    });

    it('pins to the start when the strip is longer than the region', () => {
        // Not centred into a two-sided overflow: the surplus spills one way, so
        // it stays reachable past the region's end rather than off both.
        expect(stripPos(0.5, 100, 300)).toBe(0);
        expect(stripPos(1, 100, 300)).toBe(0);
    });

    it('clamps an out-of-range offset', () => {
        expect(stripPos(-1, 600, 300)).toBe(0);
        expect(stripPos(2, 600, 300)).toBe(300);
    });
});

describe('offsetForPos', () => {
    it('round-trips stripPos', () => {
        expect(offsetForPos(stripPos(0.25, 600, 300), 600, 300)).toBeCloseTo(0.25);
    });

    it('reports the start when there is no travel', () => {
        expect(offsetForPos(50, 100, 300)).toBe(0);
    });
});

describe('stripBox', () => {
    it('hugs the docked edge and runs along it', () => {
        expect(stripBox('left', region, 300, 44, 150)).toEqual({ left: 100, top: 200, width: 44, height: 300 });
        expect(stripBox('right', region, 300, 44, 150)).toEqual({ left: 1056, top: 200, width: 44, height: 300 });
        expect(stripBox('top', region, 300, 44, 150)).toEqual({ left: 250, top: 50, width: 300, height: 44 });
        expect(stripBox('bottom', region, 300, 44, 150)).toEqual({ left: 250, top: 606, width: 300, height: 44 });
    });
});

describe('placementFromPointer', () => {
    it('re-docks to whichever edge the pointer is nearest', () => {
        expect(placementFromPointer(110, 350, region, 300, 150).edge).toBe('left');
        expect(placementFromPointer(1090, 350, region, 300, 150).edge).toBe('right');
        expect(placementFromPointer(600, 60, region, 300, 150).edge).toBe('top');
        expect(placementFromPointer(600, 640, region, 300, 150).edge).toBe('bottom');
    });

    it('keeps the grab point under the pointer, so the strip does not jump', () => {
        // Grabbed 150px down a 300px strip currently at offset 0.5 (pos 150, so
        // the grab is at region-y 300, client 350). Reporting that same point
        // must give the offset back unchanged.
        const { offset } = placementFromPointer(110, 350, region, 300, 150);
        expect(offset).toBeCloseTo(0.5);
    });
});

describe('peekOut', () => {
    it('is hysteretic: out inside near, tucked past far, unchanged between', () => {
        const box: Rect = { left: 100, top: 200, width: 44, height: 300 };
        expect(peekOut(false, 150, 350, box, 32, 56)).toBe(true);
        expect(peekOut(true, 210, 350, box, 32, 56)).toBe(false);
        expect(peekOut(true, 184, 350, box, 32, 56)).toBe(true);
        expect(peekOut(false, 184, 350, box, 32, 56)).toBe(false);
    });

    // The reason distance is measured to the strip's box and not to its edge:
    // painting in the corner of a region whose strip is centred elsewhere on
    // that same edge must leave it tucked.
    it('stays tucked far along the edge from the strip', () => {
        const box: Rect = { left: 100, top: 200, width: 44, height: 300 };
        expect(peekOut(false, 105, 640, box, 32, 56)).toBe(false);
    });
});
