import { describe, it, expect } from 'vitest';
import {
    padPointToOffset,
    offsetToPadPoint,
    clampOffset,
    offsetPolar,
} from '../offsetPad';

const SIZE = 80; // radius 40
const MAX = 64;

describe('padPointToOffset', () => {
    it('maps the center to a zero offset', () => {
        expect(padPointToOffset(40, 40, SIZE, MAX)).toEqual([0, 0]);
    });

    it('maps the right edge to +x at full magnitude', () => {
        const [x, y] = padPointToOffset(80, 40, SIZE, MAX);
        expect(x).toBeCloseTo(MAX, 5);
        expect(y).toBeCloseTo(0, 5);
    });

    it('maps the top to −y (screen up)', () => {
        const [x, y] = padPointToOffset(40, 0, SIZE, MAX);
        expect(x).toBeCloseTo(0, 5);
        expect(y).toBeCloseTo(-MAX, 5);
    });

    it('clamps a drag past the edge to the max radius', () => {
        // Far to the right, well beyond the pad.
        const [x] = padPointToOffset(400, 40, SIZE, MAX);
        expect(x).toBeCloseTo(MAX, 5);
    });
});

describe('offsetToPadPoint / padPointToOffset round-trip', () => {
    it('recovers an in-range offset', () => {
        const offset: [number, number] = [20, -12];
        const [px, py] = offsetToPadPoint(offset, SIZE, MAX);
        const back = padPointToOffset(px, py, SIZE, MAX);
        expect(back[0]).toBeCloseTo(offset[0], 4);
        expect(back[1]).toBeCloseTo(offset[1], 4);
    });

    it('parks a zero offset at the pad center', () => {
        expect(offsetToPadPoint([0, 0], SIZE, MAX)).toEqual([40, 40]);
    });

    it('parks an over-max offset at the edge', () => {
        const [px] = offsetToPadPoint([1000, 0], SIZE, MAX);
        expect(px).toBeCloseTo(SIZE, 5); // right edge
    });
});

describe('clampOffset', () => {
    it('leaves an in-range vector unchanged', () => {
        expect(clampOffset([10, 10], MAX)).toEqual([10, 10]);
    });

    it('scales an over-range vector to the max magnitude, keeping direction', () => {
        const [x, y] = clampOffset([300, 400], 50); // magnitude 500 → scale ×0.1
        expect(x).toBeCloseTo(30, 5);
        expect(y).toBeCloseTo(40, 5);
        expect(Math.hypot(x, y)).toBeCloseTo(50, 5);
    });
});

describe('offsetPolar', () => {
    it('reports angle 0 for +x and 90 for +y', () => {
        expect(offsetPolar([5, 0]).angle).toBeCloseTo(0, 5);
        expect(offsetPolar([0, 5]).angle).toBeCloseTo(90, 5);
    });

    it('reports the vector magnitude as distance', () => {
        expect(offsetPolar([3, 4]).distance).toBeCloseTo(5, 5);
    });
});
