import { describe, it, expect } from 'vitest';
import {
    positionToGamma,
    gammaToPosition,
    gammaHandlePos,
    gammaFromHandlePos,
    clampInputBlack,
    clampInputWhite,
    clampOutput,
    MIN_INPUT_GAP,
} from '../levels_math';

describe('gamma ↔ position mapping', () => {
    it('maps gamma 1 to the centre (relPos 0.5) and back', () => {
        expect(gammaToPosition(1)).toBeCloseTo(0.5, 6);
        expect(positionToGamma(0.5)).toBeCloseTo(1, 6);
    });

    it('round-trips gamma → position → gamma across the range', () => {
        for (const g of [0.1, 0.25, 0.5, 0.8, 1, 1.5, 2, 4, 7, 10]) {
            expect(positionToGamma(gammaToPosition(g))).toBeCloseTo(g, 4);
        }
    });

    it('places gamma > 1 below centre and gamma < 1 above centre', () => {
        expect(gammaToPosition(2)).toBeLessThan(0.5);
        expect(gammaToPosition(0.5)).toBeGreaterThan(0.5);
    });
});

describe('gamma handle position follows the bounds, gamma fixed', () => {
    it('derives the handle position from bounds without changing gamma', () => {
        const gamma = 2;
        // Widening/shifting the [black, white] window moves the gamma handle's
        // absolute position while the stored gamma exponent is untouched.
        const posA = gammaHandlePos(0, 1, gamma);
        const posB = gammaHandlePos(0.2, 0.8, gamma);
        expect(posA).not.toBeCloseTo(posB, 3);
        // The handle stays at the same *relative* offset within the window.
        const relA = (posA - 0) / (1 - 0);
        const relB = (posB - 0.2) / (0.8 - 0.2);
        expect(relA).toBeCloseTo(relB, 6);
    });

    it('recovers the gamma exponent from the handle position', () => {
        const black = 0.2;
        const white = 0.9;
        const gamma = 3;
        const pos = gammaHandlePos(black, white, gamma);
        expect(gammaFromHandlePos(pos, black, white)).toBeCloseTo(gamma, 4);
    });
});

describe('handle constraints', () => {
    it('input handles cannot cross (min gap enforced)', () => {
        // Black can't reach white.
        expect(clampInputBlack(0.95, 0.5)).toBeCloseTo(0.5 - MIN_INPUT_GAP, 6);
        // White can't reach black.
        expect(clampInputWhite(0.1, 0.5)).toBeCloseTo(0.5 + MIN_INPUT_GAP, 6);
        // Ordinary values pass through.
        expect(clampInputBlack(0.3, 0.8)).toBeCloseTo(0.3, 6);
    });

    it('output handles may invert (only clamped to [0,1])', () => {
        expect(clampOutput(0.9)).toBeCloseTo(0.9, 6);
        expect(clampOutput(-0.2)).toBe(0);
        expect(clampOutput(1.5)).toBe(1);
        // Nothing forbids outBlack > outWhite — inversion is allowed.
        const outBlack = clampOutput(0.9);
        const outWhite = clampOutput(0.1);
        expect(outBlack).toBeGreaterThan(outWhite);
    });
});
