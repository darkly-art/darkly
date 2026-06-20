import { describe, it, expect } from 'vitest';
import { effectivePressure } from '../pressure';

// Regression test for the iPad finger-painting bug: finger touch reports
// `pressure: 0`, which — wired into brush size/flow — collapsed every dab to
// the sub-pixel radius floor and painted nothing. A zero reading from a
// non-stylus (or a stylus that doesn't surface force) must read as full
// pressure. No DOM in the node test env, so we use plain PointerEvent fakes.
const ev = (pointerType: string, pressure: number) =>
    ({ pointerType, pressure }) as PointerEvent;

describe('effectivePressure', () => {
    it('forces full pressure for finger touch reporting 0 (the bug)', () => {
        expect(effectivePressure(ev('touch', 0))).toBe(1.0);
    });

    it('forces full pressure for a stylus that reports 0 (no force surfaced)', () => {
        expect(effectivePressure(ev('pen', 0))).toBe(1.0);
    });

    it('overrides mouse (W3C 0.5 placeholder) to full pressure', () => {
        expect(effectivePressure(ev('mouse', 0.5))).toBe(1.0);
    });

    it('keeps a real stylus pressure reading', () => {
        expect(effectivePressure(ev('pen', 0.4))).toBe(0.4);
    });
});
