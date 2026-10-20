import { describe, it, expect } from 'vitest';
import { colorToRgba01 } from '../../lib/color';
import type { Color } from '../../lib/color';

// Regression for the brush-darkens-paint bug: the brush was the only paint
// tool that ran the picked color through an sRGB->linear conversion before
// sending it to the engine, so a stroke landed a darker color than the picker
// (and than fill / gradient, which send the raw picked bytes). Darkly stores
// and displays raw picked RGBA end to end; no tool rescales the number, so the
// foreground the canvas view pushes to the engine (`colorToRgba01`) must be
// the picked bytes straight through as `c / 255`.
//
// Guard values: the picked components below must map to their plain /255
// fractions, NOT the darkened sRGB->linear values that caused the bug
// (e.g. 128 -> ~0.216, 200 -> ~0.578). If this test starts asserting those
// darker numbers again, the conversion has been reintroduced.

describe('brush color convention', () => {
    it('forwards the picked sRGB color unchanged (no gamma darkening)', () => {
        const fg: Color = { r: 128, g: 64, b: 200, a: 255 };
        const [r, g, b, a] = colorToRgba01(fg);

        expect(r).toBeCloseTo(128 / 255, 5); // ~0.50196, NOT srgbToLinear(128) ~0.2158
        expect(g).toBeCloseTo(64 / 255, 5);  // ~0.25098, NOT ~0.0513
        expect(b).toBeCloseTo(200 / 255, 5); // ~0.78431, NOT ~0.5776
        expect(a).toBeCloseTo(1.0, 5);
    });

    it('maps a partial alpha straight through', () => {
        const fg: Color = { r: 255, g: 255, b: 255, a: 128 };
        const [r, g, b, a] = colorToRgba01(fg);

        expect(r).toBeCloseTo(1.0, 5);
        expect(g).toBeCloseTo(1.0, 5);
        expect(b).toBeCloseTo(1.0, 5);
        expect(a).toBeCloseTo(128 / 255, 5);
    });
});
