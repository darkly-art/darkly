import { describe, it, expect } from 'vitest';
import { perspectiveMode } from '../transform_modes/perspective';
import type { GizmoGeometry } from '../transform_modes/types';
import { mat3Apply, MAT3_IDENTITY, type Mat3 } from '../transform_projective';

/**
 * Perspective mode: dragging one corner recomputes the full homography from all
 * four destination corners, mapping the source rect onto the new quad. Follows
 * GIMP/Krita (recompute every motion; a corner drag moves only that corner).
 */

const SRC_W = 100;
const SRC_H = 80;

function geometry(matrix: Mat3 = [...MAT3_IDENTITY], origin: [number, number] = [0, 0]): GizmoGeometry {
    return { matrix, origin, srcW: SRC_W, srcH: SRC_H };
}

/** Source-rect corners → canvas via the matrix + origin. */
function destCorner(geo: GizmoGeometry, lx: number, ly: number): [number, number] {
    const [x, y] = mat3Apply(geo.matrix, lx, ly);
    return [x + geo.origin[0], y + geo.origin[1]];
}

const Handle = { TopLeft: 0, TopRight: 1, BottomRight: 2, BottomLeft: 3, Body: 4 };

describe('perspective mode corner drag', () => {
    it('maps the rect corners onto the dragged quad', () => {
        const geo = geometry();
        // Drag the top-right corner inward+down to make a trapezoid.
        const drag = perspectiveMode.beginDrag(geo, Handle.TopRight, SRC_W, 0);
        const target: [number, number] = [70, 15];
        const m = perspectiveMode.updateDrag(geo, drag, target[0], target[1], false);

        // The four source corners now land on TL, dragged-TR, BR, BL.
        const near = (p: [number, number], q: [number, number]) => {
            expect(p[0]).toBeCloseTo(q[0], 2);
            expect(p[1]).toBeCloseTo(q[1], 2);
        };
        near(mat3Apply(m, 0, 0), [0, 0]);
        near(mat3Apply(m, SRC_W, 0), target);
        near(mat3Apply(m, SRC_W, SRC_H), [SRC_W, SRC_H]);
        near(mat3Apply(m, 0, SRC_H), [0, SRC_H]);
    });

    it('translates the whole quad on a body drag', () => {
        const geo = geometry();
        const drag = perspectiveMode.beginDrag(geo, Handle.Body, 50, 40);
        const m = perspectiveMode.updateDrag(geo, drag, 60, 47, false); // +10,+7

        const p = mat3Apply(m, 0, 0);
        expect(p[0]).toBeCloseTo(10, 2);
        expect(p[1]).toBeCloseTo(7, 2);
    });

    it('buildOverlay returns the four canvas corners as the bbox', () => {
        const geo = geometry([...MAT3_IDENTITY], [5, 9]);
        const calls: { handles: number } = { handles: 0 };
        const o = {
            line: () => {},
            handle: () => {
                calls.handles++;
            },
            hitTest: () => null,
        };
        const bbox = perspectiveMode.buildOverlay(geo, o as never);
        expect(calls.handles).toBe(4); // corner handles only — no edge/rotate
        expect(bbox[0]).toEqual(destCorner(geo, 0, 0));
        expect(bbox[2]).toEqual(destCorner(geo, SRC_W, SRC_H));
    });
});
