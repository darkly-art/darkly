// Pure value <-> position math for the shared Slider widget. Kept free of DOM
// so it can be unit-tested headlessly (vitest runs in node, no `window`).

/** Step between adjacent slider values. Explicit `step` wins; otherwise integer
 *  sliders move by 1 and continuous ones split the range into 200 notches. */
export function resolveStep(min: number, max: number, integer: boolean, step?: number): number {
    if (step && step > 0) return step;
    if (integer) return 1;
    const span = max - min;
    return span > 0 ? span / 200 : 1;
}

/** Clamp to `[min, max]`, rounding to an integer when the slider is integral. */
export function clampValue(value: number, min: number, max: number, integer: boolean): number {
    let v = integer ? Math.round(value) : value;
    if (v < min) v = min;
    if (v > max) v = max;
    return v;
}

/** Fraction of the track a value occupies, in `[0, 1]`. Degenerate ranges
 *  (max <= min) collapse to 0 so the handle parks at the left. */
export function valueToFraction(value: number, min: number, max: number): number {
    if (max <= min) return 0;
    const f = (value - min) / (max - min);
    return Math.min(1, Math.max(0, f));
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
    const f = Math.min(1, Math.max(0, fraction));
    return quantize(min + f * (max - min), min, max, integer, step);
}
