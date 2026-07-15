// Pure vector <-> pad-position math for the OffsetPad widget. Kept free of DOM
// so it can be unit-tested headlessly (vitest runs in node — no `window`),
// mirroring the `lib/slider.ts` precedent.
//
// The pad is a square of `size` px with the crosshair center at its middle. A
// handle dragged outward encodes an offset vector: drag *direction* is the
// offset direction, drag *length* is the magnitude, mapped so the pad edge
// (radius = size/2) equals the param's `max` magnitude. Screen y-down matches
// image y-down, so no axis is flipped.

/** Map a pointer position (px within the pad, origin top-left) to an offset
 *  vector, clamped so the pad edge = `max` magnitude. */
export function padPointToOffset(
    px: number,
    py: number,
    size: number,
    max: number,
): [number, number] {
    const r = size / 2;
    if (r <= 0) return [0, 0];
    const dx = px - r;
    const dy = py - r;
    const dist = Math.hypot(dx, dy);
    if (dist < 1e-6) return [0, 0];
    const mag = (Math.min(dist, r) / r) * max;
    return [(dx / dist) * mag, (dy / dist) * mag];
}

/** Inverse of {@link padPointToOffset}: an offset vector → the handle position
 *  (px, origin top-left). An over-`max` vector parks the handle at the edge. */
export function offsetToPadPoint(
    offset: [number, number],
    size: number,
    max: number,
): [number, number] {
    const r = size / 2;
    const mag = Math.hypot(offset[0], offset[1]);
    if (mag < 1e-6 || max <= 0) return [r, r];
    const distPx = Math.min(mag / max, 1) * r;
    return [r + (offset[0] / mag) * distPx, r + (offset[1] / mag) * distPx];
}

/** Clamp an offset vector to `max` magnitude, preserving direction. */
export function clampOffset(offset: [number, number], max: number): [number, number] {
    const mag = Math.hypot(offset[0], offset[1]);
    if (mag <= max || mag < 1e-6) return [offset[0], offset[1]];
    return [(offset[0] / mag) * max, (offset[1] / mag) * max];
}

/** Angle (degrees, 0–360, 0 = +x / right) and magnitude of an offset vector —
 *  the compact numeric readout beside the pad. */
export function offsetPolar(offset: [number, number]): { angle: number; distance: number } {
    const distance = Math.hypot(offset[0], offset[1]);
    let angle = (Math.atan2(offset[1], offset[0]) * 180) / Math.PI;
    if (angle < 0) angle += 360;
    return { angle, distance };
}
