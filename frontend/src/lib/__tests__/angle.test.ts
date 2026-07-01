import { describe, it, expect } from 'vitest';
import {
    snapAngleToGrid,
    detentAngle,
    SNAP_ANGLE_RAD,
    CARDINAL_ANGLE_RAD,
    CARDINAL_TOL_RAD,
} from '../angle';

const DEG = Math.PI / 180;

describe('snapAngleToGrid', () => {
    it('rounds to the nearest 15° multiple by default', () => {
        expect(snapAngleToGrid(7 * DEG)).toBeCloseTo(0, 9);
        expect(snapAngleToGrid(8 * DEG)).toBeCloseTo(15 * DEG, 9);
        expect(snapAngleToGrid(22 * DEG)).toBeCloseTo(15 * DEG, 9);
        expect(snapAngleToGrid(23 * DEG)).toBeCloseTo(30 * DEG, 9);
    });

    it('honors a custom step', () => {
        expect(snapAngleToGrid(20 * DEG, CARDINAL_ANGLE_RAD)).toBeCloseTo(0, 9);
        expect(snapAngleToGrid(30 * DEG, CARDINAL_ANGLE_RAD)).toBeCloseTo(45 * DEG, 9);
    });

    it('is wrap-invariant — 2π is a multiple of 15°', () => {
        const a = 23 * DEG;
        expect(snapAngleToGrid(a + 2 * Math.PI)).toBeCloseTo(
            snapAngleToGrid(a) + 2 * Math.PI,
            9,
        );
    });
});

describe('detentAngle', () => {
    it('snaps to the nearest mark when inside the tolerance', () => {
        // 1° off upright, within the ±2° band → snaps to 0.
        expect(detentAngle(1 * DEG, CARDINAL_ANGLE_RAD, CARDINAL_TOL_RAD)).toBeCloseTo(0, 9);
        // 1° shy of 45° → snaps to 45°.
        expect(detentAngle(44 * DEG, CARDINAL_ANGLE_RAD, CARDINAL_TOL_RAD)).toBeCloseTo(
            CARDINAL_ANGLE_RAD,
            9,
        );
    });

    it('passes through unchanged just outside the tolerance', () => {
        const a = 3 * DEG; // 3° > 2° band
        expect(detentAngle(a, CARDINAL_ANGLE_RAD, CARDINAL_TOL_RAD)).toBeCloseTo(a, 9);
    });

    it('is identity far from any mark', () => {
        const a = 20 * DEG;
        expect(detentAngle(a, CARDINAL_ANGLE_RAD, CARDINAL_TOL_RAD)).toBeCloseTo(a, 9);
    });

    it('exposes the documented constants', () => {
        expect(SNAP_ANGLE_RAD).toBeCloseTo(15 * DEG, 12);
        expect(CARDINAL_ANGLE_RAD).toBeCloseTo(45 * DEG, 12);
        expect(CARDINAL_TOL_RAD).toBeCloseTo(2 * DEG, 12);
    });
});
