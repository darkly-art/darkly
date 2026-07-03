import { describe, it, expect } from 'vitest';
import {
    resolveStep,
    clampValue,
    valueToFraction,
    quantize,
    fractionToValue,
} from '../slider';

describe('resolveStep', () => {
    it('honors an explicit positive step', () => {
        expect(resolveStep(0, 100, false, 5)).toBe(5);
        expect(resolveStep(0, 100, true, 5)).toBe(5);
    });
    it('ignores a non-positive explicit step', () => {
        expect(resolveStep(0, 100, true, 0)).toBe(1);
        expect(resolveStep(0, 100, false, -1)).toBe(0.5);
    });
    it('uses 1 for integer sliders', () => {
        expect(resolveStep(0, 900, true)).toBe(1);
    });
    it('splits a continuous range into 200 notches', () => {
        expect(resolveStep(0, 1, false)).toBe(0.005);
        expect(resolveStep(-10, 50, false)).toBe(0.3);
    });
    it('falls back to 1 for a degenerate range', () => {
        expect(resolveStep(5, 5, false)).toBe(1);
    });
});

describe('clampValue', () => {
    it('clamps to the range', () => {
        expect(clampValue(-3, 0, 10, false)).toBe(0);
        expect(clampValue(13, 0, 10, false)).toBe(10);
        expect(clampValue(4.2, 0, 10, false)).toBe(4.2);
    });
    it('rounds integer sliders', () => {
        expect(clampValue(4.6, 0, 10, true)).toBe(5);
        expect(clampValue(4.2, 0, 10, true)).toBe(4);
    });
});

describe('valueToFraction', () => {
    it('maps value to [0,1] across the range', () => {
        expect(valueToFraction(0, 0, 100)).toBe(0);
        expect(valueToFraction(50, 0, 100)).toBe(0.5);
        expect(valueToFraction(100, 0, 100)).toBe(1);
    });
    it('handles negative-anchored ranges', () => {
        expect(valueToFraction(20, -10, 50)).toBeCloseTo(0.5, 10);
    });
    it('clamps out-of-range values', () => {
        expect(valueToFraction(-5, 0, 100)).toBe(0);
        expect(valueToFraction(150, 0, 100)).toBe(1);
    });
    it('collapses a degenerate range to 0', () => {
        expect(valueToFraction(5, 5, 5)).toBe(0);
    });
});

describe('quantize', () => {
    it('snaps to the nearest step anchored at min', () => {
        expect(quantize(0.37, 0, 1, false, 0.1)).toBeCloseTo(0.4, 10);
        expect(quantize(7, 0, 100, true, 5)).toBe(5);
        expect(quantize(8, 0, 100, true, 5)).toBe(10);
    });
    it('trims floating-point stepping error', () => {
        // 0.1 + 0.2 style drift must not leak into the result.
        expect(quantize(0.3, 0, 1, false, 0.1)).toBe(0.3);
    });
    it('clamps after snapping', () => {
        expect(quantize(999, 0, 10, true, 3)).toBe(10);
        expect(quantize(-999, 0, 10, true, 3)).toBe(0);
    });
});

describe('fractionToValue', () => {
    it('maps the track fraction back to a value', () => {
        expect(fractionToValue(0, 0, 100, true)).toBe(0);
        expect(fractionToValue(0.5, 0, 100, true)).toBe(50);
        expect(fractionToValue(1, 0, 100, true)).toBe(100);
    });
    it('clamps fractions outside [0,1]', () => {
        expect(fractionToValue(-0.5, 0, 100, true)).toBe(0);
        expect(fractionToValue(1.5, 0, 100, true)).toBe(100);
    });
    it('round-trips with valueToFraction on notch values', () => {
        const min = -10, max = 50;
        for (const v of [-10, 0, 20, 35, 50]) {
            const f = valueToFraction(v, min, max);
            expect(fractionToValue(f, min, max, false, 0.5)).toBeCloseTo(v, 6);
        }
    });
});
