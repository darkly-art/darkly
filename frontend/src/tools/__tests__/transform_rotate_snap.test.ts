import { describe, it, expect } from 'vitest';
import { basicMode } from '../transform_modes/basic';
import type { GizmoGeometry } from '../transform_modes/types';
import { affineRotate } from '../transform_affine';
import { affineToMat3, type Mat3 } from '../transform_projective';

/**
 * Regression test: Shift-snapping while rotating floating content must snap the
 * content's ABSOLUTE orientation to 15° marks, not the rotation delta accrued
 * since the gesture began. Snapping the delta makes the snap grid depend on
 * where the drag started (and on whether Shift was held from the start), so
 * pressing Shift mid-rotation lands on offset angles instead of 0/15/30°.
 */

// `Handle.Rotate` from basic.ts: a non-exported const enum (Rotate === 8).
const ROTATE_HANDLE = 8;
const SNAP = Math.PI / 12; // 15°

/** Orientation baked into the matrix's linear part (row-major `m[3]=c`,
 *  `m[0]=a`; same indices for the affine-as-Mat3 the basic mode carries). */
function orientation(m: Mat3): number {
    return Math.atan2(m[3], m[0]);
}

/** Smallest absolute distance from `angle` to a multiple of `SNAP`. */
function distanceToSnapGrid(angle: number): number {
    const rem = ((angle % SNAP) + SNAP) % SNAP;
    return Math.min(rem, SNAP - rem);
}

/** Geometry whose matrix already carries a non-snapped base rotation. */
function geometryAt(baseDeg: number): GizmoGeometry {
    return {
        matrix: affineToMat3(affineRotate((baseDeg * Math.PI) / 180)),
        origin: [0, 0],
        srcW: 100,
        srcH: 100,
    };
}

/** Canvas-space center of the gizmo for a geometry. */
function center(geo: GizmoGeometry): [number, number] {
    const m = geo.matrix;
    return [
        m[0] * 50 + m[1] * 50 + m[2] + geo.origin[0],
        m[3] * 50 + m[4] * 50 + m[5] + geo.origin[1],
    ];
}

/** A pointer 100px from center at the given absolute angle. */
function pointerAt(geo: GizmoGeometry, angleRad: number): [number, number] {
    const [cx, cy] = center(geo);
    return [cx + 100 * Math.cos(angleRad), cy + 100 * Math.sin(angleRad)];
}

describe('rotation snap is relative to absolute document alignment', () => {
    it('snaps the absolute orientation to a 15° mark when base is not aligned', () => {
        const geo = geometryAt(7); // content starts at a non-snapped 7°
        const [sx, sy] = pointerAt(geo, 0); // begin → startAngle 0
        const drag = basicMode.beginDrag(geo, ROTATE_HANDLE, sx, sy);

        const [ex, ey] = pointerAt(geo, (50 * Math.PI) / 180); // drag to +50°
        const result = basicMode.updateDrag(geo, drag, ex, ey, true);

        expect(distanceToSnapGrid(orientation(result))).toBeCloseTo(0, 6);
    });

    it('snaps to the mark nearest the free orientation, independent of start angle', () => {
        const geo = geometryAt(7);

        // The unsnapped (Shift-off) orientation the gesture would produce.
        const [sx, sy] = pointerAt(geo, (20 * Math.PI) / 180);
        const drag = basicMode.beginDrag(geo, ROTATE_HANDLE, sx, sy);
        const [ex, ey] = pointerAt(geo, (95 * Math.PI) / 180);
        const free = orientation(basicMode.updateDrag(geo, drag, ex, ey, false));
        const snapped = orientation(basicMode.updateDrag(geo, drag, ex, ey, true));

        // Snapped orientation is the absolute 15° mark nearest the free one,
        // NOT base + round(delta/snap)*snap, which the old code produced.
        expect(snapped).toBeCloseTo(Math.round(free / SNAP) * SNAP, 6);
    });
});
