import { describe, it, expect } from 'vitest';
import { UNITS, unitFor } from '../units';
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
});
