import { describe, it, expect, beforeEach } from 'vitest';
import { effectivePressure, resetPressureCapability } from '../pressure';

// No DOM in the node test env, so we use plain PointerEvent fakes.
const ev = (pointerType: string, pressure: number) =>
    ({ pointerType, pressure }) as PointerEvent;

describe('effectivePressure', () => {
    // Capability is session-global; clear it so tests don't leak into each other.
    beforeEach(() => resetPressureCapability());

    // Regression for the iPad finger-painting bug: a sensorless touchscreen
    // reports pressure 0, which (wired into brush size/flow) collapsed every
    // dab to the sub-pixel radius floor and painted nothing. Until a device
    // proves it measures force, its input reads as full pressure.
    it('forces full pressure for finger touch reporting 0 (the bug)', () => {
        expect(effectivePressure(ev('touch', 0))).toBe(1.0);
    });

    // The placeholder values the spec emits for sensorless hardware prove
    // nothing, so they read as full pressure.
    it('treats the mouse 0.5 placeholder as no sensor → full', () => {
        expect(effectivePressure(ev('mouse', 0.5))).toBe(1.0);
    });

    it('treats a bare 1.0 (no other reading seen) as no sensor → full', () => {
        expect(effectivePressure(ev('pen', 1.0))).toBe(1.0);
    });

    // A non-placeholder reading proves a sensor; that value is kept.
    it('keeps a real stylus pressure reading', () => {
        expect(effectivePressure(ev('pen', 0.4))).toBe(0.4);
    });

    // Android touchscreens that report finger force must keep their reading.
    it('keeps a real touch pressure reading', () => {
        expect(effectivePressure(ev('touch', 0.3))).toBe(0.3);
    });

    // Regression: once a pen has proven it carries a sensor, a genuine 0
    // (stroke start / end / featherlight) passes through untouched, not
    // jumping to full, which is what the old `pressure > 0 ? pressure : 1.0`
    // clause did.
    it('passes a proven pen zero through as zero', () => {
        effectivePressure(ev('pen', 0.4)); // prove the sensor exists
        expect(effectivePressure(ev('pen', 0))).toBe(0);
    });

    // Capability is per pointerType: proving a pen must not make a sensorless
    // touchscreen start trusting its zeros.
    it('tracks capability per pointerType', () => {
        effectivePressure(ev('pen', 0.4)); // pen proven
        expect(effectivePressure(ev('touch', 0))).toBe(1.0); // touch still default
    });
});
