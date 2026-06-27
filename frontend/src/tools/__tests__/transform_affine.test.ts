import { describe, it, expect } from 'vitest';
import {
    affineTransform,
    affineMultiply,
    affineScale,
    affineTranslate,
    type Affine2D,
} from '../transform_affine';

/**
 * Mirror of Rust `transform::tests::affine_contract`. Both sides transform the
 * SAME matrix + point and MUST agree — this pins the row-major `[a,b,tx,c,d,ty]`
 * convention so the JS gizmo and the Rust record can't silently diverge across
 * the WASM boundary. If you change one, the other must change identically.
 */
describe('affine contract (mirrors Rust transform::tests::affine_contract)', () => {
    it('transforms a point row-major', () => {
        const m: Affine2D = [2, 0, 10, 0, 3, 20];
        const [x, y] = affineTransform(m, 5, 7);
        expect(x).toBeCloseTo(2 * 5 + 10); // 20
        expect(y).toBeCloseTo(3 * 7 + 20); // 41
    });

    it('multiply applies b first (a ∘ b)', () => {
        // scale ∘ translate: translate first (x+1), then scale (×2) → (x+1)*2
        const m = affineMultiply(affineScale(2, 2), affineTranslate(1, 0));
        const [x] = affineTransform(m, 3, 0);
        expect(x).toBeCloseTo((3 + 1) * 2); // 8
    });
});
