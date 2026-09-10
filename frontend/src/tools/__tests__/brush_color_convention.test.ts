import { describe, it, expect } from 'vitest';
import { brushStrokeParams } from '../brush.svelte';
import type { Color } from '../../state/app.svelte';

// Regression for the brush-darkens-paint bug: the brush was the only paint
// tool that ran the picked color through an sRGB->linear conversion before
// sending it to the engine, so a stroke landed a darker color than the picker
// (and than fill / gradient, which send the raw picked bytes). Darkly stores
// and displays raw picked RGBA end to end; no tool rescales the number, so
// `brushStrokeParams` must forward the foreground straight through as `c / 255`.
//
// Guard values: the picked components below must map to their plain /255
// fractions, NOT the darkened sRGB->linear values that caused the bug
// (e.g. 128 -> ~0.216, 200 -> ~0.578). If this test starts asserting those
// darker numbers again, the conversion has been reintroduced.

// Vitest runs in node (no DOM), so PointerEvent is faked as a plain object.
// `pointerType` is required because `brushStrokeParams` -> `effectivePressure(e)`
// reads it; it does not influence the color channels under test.
const fakeEvent = {
    pointerType: 'mouse',
    pressure: 1,
    tiltX: 0,
    tiltY: 0,
    twist: 0,
    tangentialPressure: 0,
    timeStamp: 0,
} as unknown as PointerEvent;

describe('brush color convention', () => {
    it('forwards the picked sRGB color unchanged (no gamma darkening)', () => {
        const fg: Color = { r: 128, g: 64, b: 200, a: 255 };
        const p = brushStrokeParams(fakeEvent, 10, 20, fg);

        expect(p.cr).toBeCloseTo(128 / 255, 5); // ~0.50196, NOT srgbToLinear(128) ~0.2158
        expect(p.cg).toBeCloseTo(64 / 255, 5);  // ~0.25098, NOT ~0.0513
        expect(p.cb).toBeCloseTo(200 / 255, 5); // ~0.78431, NOT ~0.5776
        expect(p.ca).toBeCloseTo(1.0, 5);
    });

    it('maps a partial alpha straight through', () => {
        const fg: Color = { r: 255, g: 255, b: 255, a: 128 };
        const p = brushStrokeParams(fakeEvent, 0, 0, fg);

        expect(p.cr).toBeCloseTo(1.0, 5);
        expect(p.cg).toBeCloseTo(1.0, 5);
        expect(p.cb).toBeCloseTo(1.0, 5);
        expect(p.ca).toBeCloseTo(128 / 255, 5);
    });
});
