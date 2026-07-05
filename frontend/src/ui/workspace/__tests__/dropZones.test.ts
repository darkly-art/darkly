import { describe, it, expect } from 'vitest';
import { detectDockingEdge, edgeToSplit, tabInsertionIndex, type Rect } from '../dropZones';

const rect: Rect = { left: 0, top: 0, width: 400, height: 200 };
// band = min(400,200) * 0.25 = 50px

describe('detectDockingEdge', () => {
    it('returns center in the middle', () => {
        expect(detectDockingEdge(200, 100, rect)).toBe('center');
    });

    it('detects each edge band', () => {
        expect(detectDockingEdge(10, 100, rect)).toBe('left');
        expect(detectDockingEdge(390, 100, rect)).toBe('right');
        expect(detectDockingEdge(200, 10, rect)).toBe('top');
        expect(detectDockingEdge(200, 190, rect)).toBe('bottom');
    });

    it('just inside the band boundary is still an edge, just outside is center', () => {
        expect(detectDockingEdge(49, 100, rect)).toBe('left');
        expect(detectDockingEdge(60, 100, rect)).toBe('center');
    });

    it('breaks a corner tie toward the horizontal edge', () => {
        // Top-left corner, equal penetration into left and top bands → left wins.
        expect(detectDockingEdge(5, 5, rect)).toBe('left');
    });

    it('picks the deeper penetration in an asymmetric corner', () => {
        // Very close to the left edge but only shallowly into the top band.
        expect(detectDockingEdge(2, 45, rect)).toBe('left');
        // Very close to the top edge but only shallowly into the left band.
        expect(detectDockingEdge(45, 2, rect)).toBe('top');
    });
});

describe('edgeToSplit', () => {
    it('maps edges to directions and center to null', () => {
        expect(edgeToSplit('left')).toBe('left');
        expect(edgeToSplit('bottom')).toBe('bottom');
        expect(edgeToSplit('center')).toBeNull();
    });
});

describe('tabInsertionIndex', () => {
    const mids = [10, 30, 50];
    it('inserts before the first tab left of its midpoint', () => {
        expect(tabInsertionIndex(5, mids)).toBe(0);
    });
    it('inserts between tabs', () => {
        expect(tabInsertionIndex(20, mids)).toBe(1);
        expect(tabInsertionIndex(40, mids)).toBe(2);
    });
    it('appends past the last midpoint', () => {
        expect(tabInsertionIndex(100, mids)).toBe(3);
    });
    it('appends into an empty bar', () => {
        expect(tabInsertionIndex(50, [])).toBe(0);
    });
});
