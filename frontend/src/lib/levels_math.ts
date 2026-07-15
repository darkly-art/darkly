/**
 * Pure math for the Levels slider handles.
 *
 * Ported from Krita's `libs/widgets/KisLevelsSlider.cpp` (author: the Krita
 * team, GPL-3.0). This is the behaviour that makes the widget feel right: gamma
 * is stored as an exponent (0.1–10) and its handle's on-screen position is
 * *always derived* from that exponent through a non-linear mapping, so dragging
 * the black/white bounds leaves gamma untouched while the gamma handle slides
 * proportionally between the new bounds.
 *
 * Original: https://invent.kde.org/graphics/krita — `KisLevelsSlider.cpp:550-583`.
 */

/** Gamma exponent range (Krita's `KisInputLevelsSliderWithGamma`). */
export const GAMMA_MIN = 0.1;
export const GAMMA_MAX = 10;
/** Minimum spacing between the input black/white handles (they cannot cross). */
export const MIN_INPUT_GAP = 0.001;

const LN_HALF = Math.log(0.5);

function clamp(v: number, lo: number, hi: number): number {
    return Math.max(lo, Math.min(hi, v));
}
const clamp01 = (v: number): number => clamp(v, 0, 1);

/**
 * Map a gamma handle's position *relative to the black↔white span* (`0..1`,
 * where `0.5` is the centre) to a gamma exponent. `relPos = 0.5 ↔ gamma = 1`.
 */
export function positionToGamma(relPos: number): number {
    const p = clamp01(relPos);
    let gamma: number;
    if (p < 0.5) {
        const m = Math.exp(10 * LN_HALF);
        gamma = Math.log(m + p - p * m * 2) / LN_HALF;
    } else {
        const m = Math.exp(0.1 * LN_HALF);
        gamma = Math.log(1 - (m + p) + p * m * 2) / LN_HALF;
    }
    return clamp(gamma, GAMMA_MIN, GAMMA_MAX);
}

/**
 * Inverse of [`positionToGamma`]: the gamma handle's relative position (`0..1`)
 * for a stored gamma exponent. `gamma = 1 ↔ relPos = 0.5`.
 */
export function gammaToPosition(gamma: number): number {
    const g = clamp(gamma, GAMMA_MIN, GAMMA_MAX);
    if (g > 1) {
        const m = Math.exp(10 * LN_HALF);
        return (Math.exp(g * LN_HALF) - m) / (1 - 2 * m);
    }
    const m = Math.exp(0.1 * LN_HALF);
    return (Math.exp(g * LN_HALF) + m - 1) / (2 * m - 1);
}

/**
 * Absolute `[0,1]`-domain position of the gamma handle, derived from the stored
 * gamma and the current input bounds. Dragging black/white recomputes this
 * without changing gamma (Krita `setHandlePosition` else-branch, `:470-500`).
 */
export function gammaHandlePos(black: number, white: number, gamma: number): number {
    return black + gammaToPosition(gamma) * (white - black);
}

/**
 * The gamma exponent implied by dragging the gamma handle to absolute position
 * `pos`, clamped into the current input bounds. Inverse of [`gammaHandlePos`].
 */
export function gammaFromHandlePos(pos: number, black: number, white: number): number {
    const span = white - black;
    if (span <= 0) return 1;
    const relPos = (clamp(pos, black, white) - black) / span;
    return positionToGamma(relPos);
}

/** Clamp the input black handle: `[0, white − MIN_INPUT_GAP]` (cannot cross white). */
export function clampInputBlack(black: number, white: number): number {
    return clamp(black, 0, white - MIN_INPUT_GAP);
}

/** Clamp the input white handle: `[black + MIN_INPUT_GAP, 1]` (cannot cross black). */
export function clampInputWhite(white: number, black: number): number {
    return clamp(white, black + MIN_INPUT_GAP, 1);
}

/** Output handles may cross (allows inversion) — only clamp to `[0,1]`. */
export function clampOutput(v: number): number {
    return clamp01(v);
}
