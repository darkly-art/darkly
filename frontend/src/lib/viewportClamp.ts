/**
 * Keep a floating surface inside the viewport.
 *
 * A popup positioned at the pointer clips when the pointer is near a bottom or
 * right edge, which is exactly where a right-click lands often enough to
 * matter. `ColorPopup` and `AddNodeMenu` each solved it with their own copy of
 * this arithmetic; `ContextMenu` did not solve it at all and clipped.
 */

/** Gap kept between the surface and the viewport edge, in px. */
export const VIEWPORT_MARGIN = 8;

export interface Size {
    width: number;
    height: number;
}

/**
 * The nearest position to `x`/`y` that keeps a `size` surface fully on screen.
 *
 * Clamps rather than flipping: the caller has already chosen a side, and a
 * surface nudged a few pixels stays where the pointer expects it, where a
 * flipped one jumps across the cursor.
 */
export function clampToViewport(
    x: number,
    y: number,
    size: Size,
    margin = VIEWPORT_MARGIN,
): { x: number; y: number } {
    const maxX = window.innerWidth - margin - size.width;
    const maxY = window.innerHeight - margin - size.height;
    return {
        // `Math.max(margin, ...)` last, so a surface taller or wider than the
        // viewport pins to the top-left corner rather than off the other edge.
        x: Math.max(margin, Math.min(x, maxX)),
        y: Math.max(margin, Math.min(y, maxY)),
    };
}
