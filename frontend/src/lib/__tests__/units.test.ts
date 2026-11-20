import { describe, it, expect } from 'vitest';
import { UNITS, formatScalar, unitFor, unitOptions } from '../units';
import type { UnitType } from '../../engine/protocol_gen';

const PI = Math.PI;
const ALL_UNITS: UnitType[] = ['Normalized', 'Percent', 'Degrees', 'Raw', 'Pixels'];

describe('units', () => {
    // Round-trip across every unit, mirroring graph.rs `unit_type_conversion_round_trip`.
    it('toPort(toDisplay(v)) ≈ v for every unit', () => {
        for (const u of ALL_UNITS) {
            for (const v of [0, 0.25, 0.5, 0.75, 1]) {
                const back = UNITS[u].toPort(UNITS[u].toDisplay(v));
                expect(back).toBeCloseTo(v, 6);
            }
        }
    });

    // Reference values: the drift guard. These are the *exact same literals*
    // as the Rust reference test (graph.rs `unit_type_display_values`), so the
    // TS table can never silently diverge from `UnitType`. Pixels is added here
    // (Rust's test omits it) because aligning Pixels is a goal of this change.
    it('produces the same reference display values as Rust', () => {
        expect(UNITS.Degrees.toDisplay(PI)).toBeCloseTo(180, 4);
        expect(UNITS.Degrees.toDisplay(PI / 2)).toBeCloseTo(90, 4);
        expect(UNITS.Degrees.toDisplay(0)).toBe(0);
        expect(UNITS.Degrees.toPort(180)).toBeCloseTo(PI, 4);
        expect(UNITS.Degrees.toPort(90)).toBeCloseTo(PI / 2, 4);

        expect(UNITS.Percent.toDisplay(0.5)).toBe(50);
        expect(UNITS.Percent.toPort(50)).toBe(0.5);

        expect(UNITS.Normalized.toDisplay(0.5)).toBe(0.5);
        expect(UNITS.Raw.toDisplay(0.5)).toBe(0.5);
        expect(UNITS.Pixels.toDisplay(0.5)).toBe(0.5);
    });

    // Suffix, mirroring graph.rs `unit_type_suffix`, plus Pixels.
    it('has the correct suffix per unit', () => {
        expect(UNITS.Percent.suffix).toBe('%');
        expect(UNITS.Degrees.suffix).toBe('°');
        expect(UNITS.Pixels.suffix).toBe('px');
        expect(UNITS.Normalized.suffix).toBe('');
        expect(UNITS.Raw.suffix).toBe('');
    });

    it('formats display values with rounding + suffix', () => {
        expect(UNITS.Percent.format(45)).toBe('45%');
        expect(UNITS.Degrees.format(90)).toBe('90°');
        expect(UNITS.Pixels.format(12.6)).toBe('13px');
        expect(UNITS.Normalized.format(0.5)).toBe('0.50');
        expect(UNITS.Raw.format(0.5)).toBe('0.50');
    });

    it('unitFor falls back to Normalized for unknown / missing types', () => {
        expect(unitFor('Degrees')).toBe(UNITS.Degrees);
        expect(unitFor('bogus')).toBe(UNITS.Normalized);
        expect(unitFor(undefined)).toBe(UNITS.Normalized);
        expect(unitFor(null)).toBe(UNITS.Normalized);
    });

    // The selector's rows come from UNITS, which `Record<UnitType, Unit>`
    // keeps exhaustive, so this is the guard that a new UnitType gets a
    // deliberate authorable/label decision rather than silently appearing or
    // vanishing from the brush-bar entry editor.
    it('offers every authorable unit exactly once, and never Normalized', () => {
        const offered = unitOptions().map(([u]) => u);
        // Sets, not positions: ALL_UNITS is in Rust declaration order while
        // Object.keys(UNITS) is in TS source order, and the difference means
        // nothing to a dropdown.
        expect(new Set(offered)).toEqual(new Set(ALL_UNITS.filter((u) => UNITS[u].authorable)));
        expect(offered).toHaveLength(new Set(offered).size);
        expect(offered).not.toContain('Normalized');
        for (const [, label] of unitOptions()) expect(label.length).toBeGreaterThan(0);
    });
});

describe('fine readout', () => {
    it('keeps two significant digits below 10', () => {
        expect(UNITS.Percent.format(1.44, true)).toBe('1.4%');
        expect(UNITS.Percent.format(0.25, true)).toBe('0.25%');
        expect(UNITS.Percent.format(0.01, true)).toBe('0.01%');
        expect(UNITS.Degrees.format(2.5, true)).toBe('2.5°');
    });

    it('reads whole from 10 up', () => {
        expect(UNITS.Percent.format(9.96, true)).toBe('10%');
        expect(UNITS.Percent.format(45.4, true)).toBe('45%');
        expect(UNITS.Pixels.format(12.6, true)).toBe('13px');
    });

    it('stays whole by default', () => {
        expect(UNITS.Percent.format(1.44)).toBe('1%');
    });

    it('formatScalar reads finely exactly when the control is curved', () => {
        expect(formatScalar(UNITS.Percent, 1.44, { exponent: 3, reversed: false })).toBe('1.4%');
        expect(formatScalar(UNITS.Percent, 1.44, { exponent: 1, reversed: false })).toBe('1%');
        expect(formatScalar(UNITS.Percent, 1.44)).toBe('1%');
    });
});
