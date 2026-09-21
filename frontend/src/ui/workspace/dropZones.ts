/**
 * Pure drop-zone geometry for panel-body edge docking. DOM-free: callers hand
 * in a plain rect (from `getBoundingClientRect`) and a pointer position; these
 * functions decide *where* a drop lands. All hit-testing math lives here so it
 * is directly unit-testable.
 */

import { nearestEdge, type Rect } from '../../lib/edges';
import type { DockingSplitDirection } from './tree';

export type { Rect };

/** Where a body drop resolves: an edge (→ split) or the center (→ merge into
 *  the group as a new tab). */
export type DockingEdge = DockingSplitDirection | 'center';

/** Fraction of the shorter body dimension that counts as an edge band. */
export const EDGE_FRACTION = 0.25;

/**
 * Classify a pointer position inside a panel body into an edge band or the
 * center. The band width is `EDGE_FRACTION` of the body's shorter side (so the
 * four bands stay symmetric on non-square panels). When a point falls in two
 * bands at once (a corner), the deeper penetration wins; exact ties break to
 * the edge that splits horizontally (left or right).
 */
export function detectDockingEdge(x: number, y: number, rect: Rect): DockingEdge {
    const band = Math.min(rect.width, rect.height) * EDGE_FRACTION;

    const nearest = Math.min(
        x - rect.left,
        rect.left + rect.width - x,
        y - rect.top,
        rect.top + rect.height - y,
    );

    // Outside every band is a merge into the group, not a split. Inside one,
    // the deepest penetration is the nearest edge: penetration is `band - d`,
    // which is monotone decreasing in the distance `d`, so "deepest band" and
    // "nearest edge" rank identically and `nearestEdge` answers both. Its
    // tie-break order (left, right, top, bottom) resolves an exact corner to a
    // vertical edge, which is the left/right-wins rule this function has always
    // had, pinned by the corner cases in `__tests__/dropZones.test.ts`.
    if (nearest >= band) return 'center';
    return nearestEdge(x, y, rect);
}

/** An edge maps 1:1 to a split direction; `center` has no split. */
export function edgeToSplit(edge: DockingEdge): DockingSplitDirection | null {
    return edge === 'center' ? null : edge;
}

/**
 * Given the horizontal midpoints of the tabs currently in a tab bar and a
 * pointer x, return the insertion index (0..tabMidpoints.length). A drop left
 * of a tab's midpoint inserts before it; right of the last midpoint appends.
 */
export function tabInsertionIndex(x: number, tabMidpoints: number[]): number {
    for (let i = 0; i < tabMidpoints.length; i++) {
        if (x < tabMidpoints[i]) return i;
    }
    return tabMidpoints.length;
}
