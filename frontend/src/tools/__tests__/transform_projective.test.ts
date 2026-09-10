import { describe, it, expect } from 'vitest';
import {
    affineToMat3,
    homographyFromCorners,
    mat3Apply,
    mat3Inverse,
    type Mat3,
} from '../transform_projective';

/**
 * Mirror of Rust `mat3::tests::homography_contract`. Both sides build the SAME
 * rect→quad homography and transform the SAME corners; this pins the row-major
 * `Mat3` layout so the JS gizmo and the Rust record can't silently diverge
 * across the WASM boundary. If you change one, change the other identically.
 */
describe('projective contract (mirrors Rust mat3::tests::homography_contract)', () => {
    it('maps the source-rect corners onto the requested dest corners', () => {
        const w = 100;
        const h = 80;
        // A trapezoid: top edge narrowed (vanishing-point look).
        const corners: [
            [number, number],
            [number, number],
            [number, number],
            [number, number],
        ] = [
            [20, 0],
            [80, 0],
            [100, 80],
            [0, 80],
        ];
        const m = homographyFromCorners(w, h, corners)!;
        expect(m).not.toBeNull();

        const near = (p: [number, number], x: number, y: number) => {
            expect(p[0]).toBeCloseTo(x, 2);
            expect(p[1]).toBeCloseTo(y, 2);
        };
        near(mat3Apply(m, 0, 0), 20, 0);
        near(mat3Apply(m, w, 0), 80, 0);
        near(mat3Apply(m, w, h), 100, 80);
        near(mat3Apply(m, 0, h), 0, 80);
    });

    it('inverse round-trips a point', () => {
        const corners: [
            [number, number],
            [number, number],
            [number, number],
            [number, number],
        ] = [
            [10, 5],
            [90, -10],
            [110, 70],
            [-5, 95],
        ];
        const m = homographyFromCorners(100, 80, corners)!;
        const inv = mat3Inverse(m)!;
        const p = mat3Apply(m, 37, 19);
        const back = mat3Apply(inv, p[0], p[1]);
        expect(back[0]).toBeCloseTo(37, 2);
        expect(back[1]).toBeCloseTo(19, 2);
    });

    it('widens an affine losslessly (bottom row [0,0,1])', () => {
        const m: Mat3 = affineToMat3([2, 0, 10, 0, 3, 20]);
        expect(m[6]).toBe(0);
        expect(m[7]).toBe(0);
        expect(m[8]).toBe(1);
        const p = mat3Apply(m, 5, 7);
        expect(p[0]).toBeCloseTo(2 * 5 + 10);
        expect(p[1]).toBeCloseTo(3 * 7 + 20);
    });

    it('rejects a degenerate quad', () => {
        expect(homographyFromCorners(100, 80, [[0, 0], [0, 0], [0, 0], [0, 0]])).toBeNull();
        expect(
            homographyFromCorners(0, 80, [[0, 0], [1, 0], [1, 1], [0, 1]]),
        ).toBeNull();
    });
});
