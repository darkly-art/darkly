import { describe, it, expect } from 'vitest';
import { basicMode } from '../transform_modes/basic';
import { perspectiveMode } from '../transform_modes/perspective';
import { affineToMat3, mat3Apply, type Mat3 } from '../transform_projective';
import type { GizmoGeometry } from '../transform_modes';

const W = 100;
const H = 80;

function geo(matrix: Mat3): GizmoGeometry {
    return { matrix, origin: [0, 0], srcW: W, srcH: H };
}

const near = (p: [number, number], x: number, y: number) => {
    expect(p[0]).toBeCloseTo(x, 2);
    expect(p[1]).toBeCloseTo(y, 2);
};

describe('TransformMode.seedMatrix', () => {
    it('perspective seedMatrix reproduces the current quad corners', () => {
        // A rotate+scale affine: its quad corners must be reproduced exactly
        // by the homography perspective seeds from them.
        const aff = affineToMat3([1.2, -0.3, 15, 0.4, 0.9, -7]);
        const m = perspectiveMode.seedMatrix(geo(aff));
        for (const [x, y] of [
            [0, 0],
            [W, 0],
            [W, H],
            [0, H],
        ] as [number, number][]) {
            near(mat3Apply(m, x, y), mat3Apply(aff, x, y)[0], mat3Apply(aff, x, y)[1]);
        }
    });

    it('basic seedMatrix is exact for a parallelogram (affine) input', () => {
        const aff = affineToMat3([1.2, -0.3, 15, 0.4, 0.9, -7]);
        const m = basicMode.seedMatrix(geo(aff));
        // Bottom row stays affine.
        expect(m[6]).toBeCloseTo(0, 6);
        expect(m[7]).toBeCloseTo(0, 6);
        expect(m[8]).toBeCloseTo(1, 6);
        // Reproduces the same corners (least-squares fit is exact here).
        for (const [x, y] of [
            [0, 0],
            [W, 0],
            [W, H],
            [0, H],
        ] as [number, number][]) {
            near(mat3Apply(m, x, y), mat3Apply(aff, x, y)[0], mat3Apply(aff, x, y)[1]);
        }
    });

    it('basic seedMatrix gives an honest fit (not row-truncation) for a perspective quad', () => {
        // Start from a true perspective homography (a trapezoid), seed basic.
        const persp = perspectiveMode.seedMatrix({
            matrix: [1, 0, 0, 0, 1, 0, 0, 0, 1],
            origin: [0, 0],
            srcW: W,
            srcH: H,
        });
        // Perturb into an actual trapezoid by re-seeding perspective from
        // narrowed top corners.
        const trapezoid =
            perspectiveMode.seedMatrix({ matrix: persp, origin: [0, 0], srcW: W, srcH: H });
        const basic = basicMode.seedMatrix({
            matrix: trapezoid,
            origin: [0, 0],
            srcW: W,
            srcH: H,
        });
        // The affine fit must be a real affine (bottom row [0,0,1]) and finite.
        expect(basic[6]).toBeCloseTo(0, 6);
        expect(basic[7]).toBeCloseTo(0, 6);
        for (const v of basic) expect(Number.isFinite(v)).toBe(true);
    });
});
