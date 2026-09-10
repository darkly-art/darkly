import type { UnitType } from '../engine/protocol_gen';

/** A brush-node port unit: how a raw wire value is shown, entered, and labeled.
 *  The frontend needs this only for the raw-port edit path (PortWidget), where
 *  the value lives frontend-side in port space and never round-trips through
 *  Rust before display. Exposed-port scalars are pre-converted in Rust and only
 *  need `format`. */
export interface Unit {
    /** port-space (raw wire value, e.g. radians) → display-space. */
    toDisplay(value: number): number;
    /** display-space → port-space (inverse of toDisplay). */
    toPort(display: number): number;
    /** suffix appended to a formatted display value (e.g. '°', '%', 'px'). */
    readonly suffix: string;
    /** format a *display-space* value for the label (rounding + suffix). */
    format(display: number): string;
}

const DEG_PER_RAD = 180 / Math.PI;

// One entry per UnitType. Mirrors crates/darkly/src/nodegraph/graph.rs
// UnitType::{to_display, from_display, suffix}; verified against the same
// reference values in units.test.ts.
export const UNITS: Record<UnitType, Unit> = {
    Normalized: { toDisplay: v => v,               toPort: d => d,               suffix: '',   format: d => d.toFixed(2) },
    Raw:        { toDisplay: v => v,               toPort: d => d,               suffix: '',   format: d => d.toFixed(2) },
    Pixels:     { toDisplay: v => v,               toPort: d => d,               suffix: 'px', format: d => `${Math.round(d)}px` },
    Percent:    { toDisplay: v => v * 100,         toPort: d => d / 100,         suffix: '%',  format: d => `${Math.round(d)}%` },
    Degrees:    { toDisplay: v => v * DEG_PER_RAD, toPort: d => d / DEG_PER_RAD, suffix: '°',  format: d => `${Math.round(d)}°` },
};

/** Resolve a unit table entry from a (possibly unknown) unit_type string,
 *  defaulting to Normalized, which matches Rust's `UnitType::default()`. */
export function unitFor(unitType: string | null | undefined): Unit {
    return (unitType && UNITS[unitType as UnitType]) || UNITS.Normalized;
}
