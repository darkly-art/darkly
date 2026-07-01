/** 15° — discrete rotation-snap increment (Krita DISCRETE_ANGLE_STEP, PS). */
export const SNAP_ANGLE_RAD = Math.PI / 12;
/** 45° — major "cardinal" marks the free rotation detents to. */
export const CARDINAL_ANGLE_RAD = Math.PI / 4;
/** ±2° — cardinal detent tolerance (Krita angleForSnapping). */
export const CARDINAL_TOL_RAD = (2 * Math.PI) / 180;

/** Quantize an absolute angle to the nearest grid multiple. */
export function snapAngleToGrid(angle: number, stepRad: number = SNAP_ANGLE_RAD): number {
    return Math.round(angle / stepRad) * stepRad;
}

/** Pull to the nearest grid multiple only when within `tolRad`, else pass through. */
export function detentAngle(angle: number, stepRad: number, tolRad: number): number {
    const m = Math.round(angle / stepRad) * stepRad;
    return Math.abs(angle - m) <= tolRad ? m : angle;
}
