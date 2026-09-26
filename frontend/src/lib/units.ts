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
    /** artist-facing name, for the brush-bar entry editor's unit selector. */
    readonly label: string;
    /** whether a brush author can pick this unit for an entry. `Normalized`
     *  is the only one they cannot: it is the enum's default rather than a
     *  deliberate choice (no registration calls `with_unit` with it), and it
     *  is indistinguishable from `Raw`, so offering both would be two rows
     *  that do the same thing. */
    readonly authorable: boolean;
}

const DEG_PER_RAD = 180 / Math.PI;

// One entry per UnitType. Mirrors crates/darkly/src/units.rs
// UnitType::{to_display, from_display, suffix}; verified against the same
// reference values in units.test.ts.
export const UNITS: Record<UnitType, Unit> = {
    Normalized: { toDisplay: v => v,               toPort: d => d,               suffix: '',   format: d => d.toFixed(2),        label: 'Number',      authorable: false },
    Raw:        { toDisplay: v => v,               toPort: d => d,               suffix: '',   format: d => d.toFixed(2),        label: 'Plain number', authorable: true },
    Pixels:     { toDisplay: v => v,               toPort: d => d,               suffix: 'px', format: d => `${Math.round(d)}px`, label: 'Pixels',       authorable: true },
    Percent:    { toDisplay: v => v * 100,         toPort: d => d / 100,         suffix: '%',  format: d => `${Math.round(d)}%`,  label: 'Percent',      authorable: true },
    Degrees:    { toDisplay: v => v * DEG_PER_RAD, toPort: d => d / DEG_PER_RAD, suffix: '°',  format: d => `${Math.round(d)}°`,  label: 'Degrees',      authorable: true },
};

/** `[UnitType, label]` rows for a brush-bar entry's unit selector.
 *  Derived from UNITS, which `Record<UnitType, Unit>` keeps exhaustive, so a
 *  new UnitType forces a decision here rather than appearing silently or not
 *  at all. */
export function unitOptions(): [UnitType, string][] {
    return (Object.keys(UNITS) as UnitType[])
        .filter(u => UNITS[u].authorable)
        .map(u => [u, UNITS[u].label]);
}

/** Resolve a unit table entry from a (possibly unknown) unit_type string,
 *  defaulting to Normalized, which matches Rust's `UnitType::default()`. */
export function unitFor(unitType: string | null | undefined): Unit {
    return (unitType && UNITS[unitType as UnitType]) || UNITS.Normalized;
}
