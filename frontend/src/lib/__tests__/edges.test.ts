import { describe, it, expect } from 'vitest';
import { distanceToRect, nearestEdge, type Rect } from '../edges';

const rect: Rect = { left: 0, top: 0, width: 400, height: 200 };

describe('nearestEdge', () => {
    it('picks each of the four edges', () => {
        expect(nearestEdge(10, 100, rect)).toBe('left');
        expect(nearestEdge(390, 100, rect)).toBe('right');
        expect(nearestEdge(200, 10, rect)).toBe('top');
        expect(nearestEdge(200, 190, rect)).toBe('bottom');
    });

    // `detectDockingEdge` is expressed on top of this, and its own tests pin
    // only the left-vs-top corner. Pin all four so the shared tie-break cannot
    // drift underneath it.
    it('breaks every exact corner tie toward the vertical edge', () => {
        expect(nearestEdge(5, 5, rect)).toBe('left');
        expect(nearestEdge(395, 5, rect)).toBe('right');
        expect(nearestEdge(5, 195, rect)).toBe('left');
        expect(nearestEdge(395, 195, rect)).toBe('right');
    });

    it('prefers the vertical edges when all four are equidistant', () => {
        // Dead centre of a square: left, right, top and bottom all tie.
        expect(nearestEdge(50, 50, { left: 0, top: 0, width: 100, height: 100 })).toBe('left');
    });

    it('resolves a left-vs-right tie to left, and a top-vs-bottom tie to top', () => {
        // Centred horizontally in a tall rect: left and right tie at 100, which
        // beats top and bottom at 200.
        expect(nearestEdge(100, 200, { left: 0, top: 0, width: 200, height: 400 })).toBe('left');
        // Centred vertically in a wide rect: top and bottom tie and win.
        expect(nearestEdge(200, 100, rect)).toBe('top');
    });

    // The toolbar has no 'center', unlike panel docking: a point anywhere,
    // including dead centre and outside the rect, resolves to an edge.
    it('resolves a point outside the rect', () => {
        expect(nearestEdge(-50, 100, rect)).toBe('left');
        expect(nearestEdge(200, 300, rect)).toBe('bottom');
    });
});

describe('distanceToRect', () => {
    it('is zero anywhere inside', () => {
        expect(distanceToRect(200, 100, rect)).toBe(0);
        expect(distanceToRect(0, 0, rect)).toBe(0);
    });

    it('is the perpendicular distance beside the rect', () => {
        expect(distanceToRect(-30, 100, rect)).toBe(30);
        expect(distanceToRect(200, 250, rect)).toBe(50);
    });

    it('is the diagonal distance past a corner', () => {
        expect(distanceToRect(-3, -4, rect)).toBe(5);
    });
});
