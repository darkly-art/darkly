/** Whether a device of a given `pointerType` has proven, this session, that it
 *  carries a real force sensor.
 *
 *  The web platform exposes no capability flag — only the pressure value, which
 *  is ambiguous. For sensorless hardware the Pointer Events spec emits 0.5
 *  while a button is held and 0 otherwise, and 1.0 is an unremarkable full
 *  press. So a reading that is none of {0, 0.5, 1.0} is the first proof the
 *  device actually measures force. Until that proof arrives the input reads as
 *  full pressure (so brushes that wire pressure into size/flow don't collapse
 *  every dab to the sub-pixel radius floor); once it arrives, the device's
 *  values — including genuine zeros — are trusted verbatim for the rest of the
 *  session. Keyed by `pointerType` because one session can mix devices (e.g. a
 *  pressure pen and a sensorless touchscreen), and proving one must not make
 *  the other start trusting its zeros.
 */
const pressureCapable: Record<string, boolean> = {};

/** Pressure values the spec emits for sensorless hardware (plus a plain full
 *  press), so seeing one proves nothing about whether a sensor exists. */
const PLACEHOLDER_PRESSURES = new Set([0, 0.5, 1.0]);

/** Effective pressure for a PointerEvent. Returns the device's real reading
 *  once it has proven it carries a force sensor; full pressure until then.
 *  Pinning sensorless input to full matches Krita, Photoshop, GIMP, MyPaint. */
export function effectivePressure(e: PointerEvent): number {
    if (!pressureCapable[e.pointerType] && !PLACEHOLDER_PRESSURES.has(e.pressure)) {
        pressureCapable[e.pointerType] = true;
    }
    return pressureCapable[e.pointerType] ? e.pressure : 1.0;
}

/** Clear the learned per-`pointerType` capability. Test-only — a real session
 *  never needs to un-learn a sensor. */
export function resetPressureCapability(): void {
    for (const key of Object.keys(pressureCapable)) delete pressureCapable[key];
}
