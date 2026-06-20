/** Effective pen pressure for a PointerEvent, normalised so pressure-driven
 *  brush dynamics (size, flow) never collapse on devices without a force
 *  sensor.
 *
 *  Only a stylus carries a real force sensor. Mouse reports the W3C spec
 *  default of 0.5 (a "split the difference" value for generic gesture
 *  surfaces). Finger-touch — and a stylus on engines that don't surface
 *  force through PointerEvents (notably iOS Safari) — reports 0. A raw 0
 *  there means "no sensor", not "no pressure": brushes that wire pressure
 *  into size or flow (e.g. the Round and Smudge presets) would shrink every
 *  dab to the sub-pixel radius floor and vanish. So treat any non-stylus
 *  input, or a zero reading, as full pressure — matching Krita, Photoshop,
 *  GIMP, and MyPaint, which all override mouse to full pressure for the same
 *  reason. A real stylus with a positive reading keeps its true value so
 *  pressure dynamics still work.
 */
export function effectivePressure(e: PointerEvent): number {
    return e.pointerType === 'pen' && e.pressure > 0 ? e.pressure : 1.0;
}
