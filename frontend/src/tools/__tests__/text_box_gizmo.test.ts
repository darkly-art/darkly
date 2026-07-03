import { describe, it, expect, vi } from 'vitest';

// The gizmo module imports `app` (via gpu_overlay/coordinates); the pure
// resize math under test never touches it, so an empty fake is enough.
vi.mock('../../state/app.svelte', () => ({ app: {} }));

import { resizeBox, handleLocal, MIN_BOX, type BoxGeo } from '../text_box_gizmo';
import type { Affine2D } from '../transform_affine';

// A box at canvas origin (10, 20), size 100×60, no rotation/scale:
// G = translate(10, 20), so local (x, y) → canvas (x + 10, y + 20).
const G: Affine2D = [1, 0, 10, 0, 1, 20];
const geo: BoxGeo = { G, w: 100, h: 60 };

describe('text box gizmo handle layout', () => {
    it('places handles at corners, edge midpoints', () => {
        expect(handleLocal('nw', 100, 60)).toEqual([0, 0]);
        expect(handleLocal('ne', 100, 60)).toEqual([100, 0]);
        expect(handleLocal('se', 100, 60)).toEqual([100, 60]);
        expect(handleLocal('sw', 100, 60)).toEqual([0, 60]);
        expect(handleLocal('n', 100, 60)).toEqual([50, 0]);
        expect(handleLocal('e', 100, 60)).toEqual([100, 30]);
        expect(handleLocal('s', 100, 60)).toEqual([50, 60]);
        expect(handleLocal('w', 100, 60)).toEqual([0, 30]);
    });
});

describe('text box gizmo resize math', () => {
    it('east edge grows width, origin and linear part unchanged', () => {
        // Pointer at canvas x = 160 → local x = 150.
        const r = resizeBox(geo, 'e', 160, 50)!;
        expect(r.w).toBeCloseTo(150);
        expect(r.h).toBeCloseTo(60);
        expect(r.G[2]).toBeCloseTo(10); // origin x fixed
        expect(r.G[5]).toBeCloseTo(20); // origin y fixed
        expect([r.G[0], r.G[1], r.G[3], r.G[4]]).toEqual([1, 0, 0, 1]); // basis fixed
    });

    it('south edge grows height only', () => {
        const r = resizeBox(geo, 's', 50, 110)!; // local y = 90
        expect(r.w).toBeCloseTo(100);
        expect(r.h).toBeCloseTo(90);
        expect(r.G[2]).toBeCloseTo(10);
        expect(r.G[5]).toBeCloseTo(20);
    });

    it('se corner grows both, origin fixed', () => {
        const r = resizeBox(geo, 'se', 130, 120)!; // local (120, 100)
        expect(r.w).toBeCloseTo(120);
        expect(r.h).toBeCloseTo(100);
        expect(r.G[2]).toBeCloseTo(10);
        expect(r.G[5]).toBeCloseTo(20);
    });

    it('nw corner moves the origin and shrinks the box', () => {
        // Pointer at canvas (40, 30) → local (30, 10); the SE corner stays fixed.
        const r = resizeBox(geo, 'nw', 40, 30)!;
        expect(r.w).toBeCloseTo(70); // 100 - 30
        expect(r.h).toBeCloseTo(50); // 60 - 10
        expect(r.G[2]).toBeCloseTo(40); // origin moved to the new top-left
        expect(r.G[5]).toBeCloseTo(30);
    });

    it('clamps to a minimum box size', () => {
        // Drag the east edge inward past the west edge.
        const e = resizeBox(geo, 'e', 12, 50)!; // local x = 2 < MIN_BOX
        expect(e.w).toBeCloseTo(MIN_BOX);

        // Drag the west edge rightward past the east edge: origin clamps so the
        // box keeps MIN_BOX width, pinned to the fixed east edge (local x=100).
        const w = resizeBox(geo, 'w', 210, 50)!; // local x = 200
        expect(w.w).toBeCloseTo(MIN_BOX);
        expect(w.G[2]).toBeCloseTo(10 + (100 - MIN_BOX)); // origin canvas x
    });
});
