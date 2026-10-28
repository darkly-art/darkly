// Pure value <-> position math for the shared Slider widget and every other
// control that drags a value along a range. Kept free of DOM so it can be
// unit-tested headlessly (vitest runs in node, no `window`).

import type { TravelCurve } from '../engine/protocol_gen';

export type { TravelCurve };

/** Even travel: the fraction of the track is the fraction of the range. */
const LINEAR: TravelCurve = { exponent: 1, reversed: false };

/** Step between adjacent slider values. Explicit `step` wins; otherwise integer
 *  sliders move by 1 and continuous ones split the range into 200 notches. */
export function resolveStep(min: number, max: number, integer: boolean, step?: number): number {
    if (step && step > 0) return step;
    if (integer) return 1;
    const span = max - min;
    return span > 0 ? span / 200 : 1;
}

/** Decimal places a readout needs to resolve a hundredth of the range: two
 *  for a unit range, three for `[0, 0.1]`, never fewer than two. */
export function readoutDecimals(min: number, max: number): number {
    const span = max - min;
    return span > 0 ? Math.max(2, Math.ceil(-Math.log10(span / 100))) : 2;
}

/** Clamp to `[min, max]`, rounding to an integer when the slider is integral. */
export function clampValue(value: number, min: number, max: number, integer: boolean): number {
    let v = integer ? Math.round(value) : value;
    if (v < min) v = min;
    if (v > max) v = max;
    return v;
}

/** Fraction of the track a value occupies, in `[0, 1]`: the inverse of
 *  {@link travelToValue} for the same curve. Degenerate ranges (max <= min)
 *  collapse to 0 so the handle parks at the left. */
export function valueToFraction(
    value: number,
    min: number,
    max: number,
    travel: TravelCurve = LINEAR,
): number {
    if (max <= min) return 0;
    const f = Math.min(1, Math.max(0, (value - min) / (max - min)));
    const p = 1 / travel.exponent;
    return travel.reversed ? 1 - Math.pow(1 - f, p) : Math.pow(f, p);
}

/** The value a fraction `t` of the track reads, unquantized: `t^exponent`
 *  of the range, so an exponent above 1 gives the low end more of the travel
 *  (reflected about the range when `reversed`). The power curve is Krita's
 *  slider exponent ratio (`KisSliderSpinBoxPrivate::valueForPoint`,
 *  KDE/Krita, https://invent.kde.org/graphics/krita). */
export function travelToValue(
    t: number,
    min: number,
    max: number,
    travel: TravelCurve = LINEAR,
): number {
    const u = Math.min(1, Math.max(0, t));
    const p = travel.exponent;
    const g = travel.reversed ? 1 - Math.pow(1 - u, p) : Math.pow(u, p);
    return min + g * (max - min);
}

/** Snap a raw value to the nearest step (anchored at `min`), then clamp. */
export function quantize(
    raw: number,
    min: number,
    max: number,
    integer: boolean,
    step?: number,
): number {
    const s = resolveStep(min, max, integer, step);
    const snapped = min + Math.round((raw - min) / s) * s;
    // Stepping accumulates float error; fixed() trims it before clamping so a
    // value that should land on e.g. 0.3 doesn't read as 0.30000000000000004.
    const trimmed = integer ? Math.round(snapped) : Number(snapped.toFixed(6));
    return clampValue(trimmed, min, max, integer);
}

/** Convert a track fraction (`[0, 1]`) to a quantized, clamped value. */
export function fractionToValue(
    fraction: number,
    min: number,
    max: number,
    integer: boolean,
    step?: number,
): number {
    return quantize(travelToValue(fraction, min, max), min, max, integer, step);
}
