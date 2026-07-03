/**
 * Pure drop-zone geometry for panel-body edge docking. DOM-free: callers hand
 * in a plain rect (from `getBoundingClientRect`) and a pointer position; these
 * functions decide *where* a drop lands. All hit-testing math lives here so it
 * is directly unit-testable.
 */

import type { DockingSplitDirection } from './tree';

export interface Rect {
    left: number;
    top: number;
    width: number;
    height: number;
}

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
 * the horizontal edge.
 */
export function detectDockingEdge(x: number, y: number, rect: Rect): DockingEdge {
    const dl = x - rect.left;
    const dr = rect.left + rect.width - x;
    const dt = y - rect.top;
    const db = rect.top + rect.height - y;

    const band = Math.min(rect.width, rect.height) * EDGE_FRACTION;

    // Distance *into* each edge band (0 = right at the edge, `band` = inner
    // boundary). Negative means outside the band.
    const candidates: { edge: DockingSplitDirection; penetration: number }[] = [];
    if (dl < band) candidates.push({ edge: 'left', penetration: band - dl });
    if (dr < band) candidates.push({ edge: 'right', penetration: band - dr });
    if (dt < band) candidates.push({ edge: 'top', penetration: band - dt });
    if (db < band) candidates.push({ edge: 'bottom', penetration: band - db });

    if (candidates.length === 0) return 'center';

    // Deepest penetration wins; horizontal edges (left/right) win exact ties.
    const horizontal = new Set<DockingSplitDirection>(['left', 'right']);
    let best = candidates[0];
    for (const c of candidates.slice(1)) {
        if (c.penetration > best.penetration) best = c;
        else if (c.penetration === best.penetration && horizontal.has(c.edge) && !horizontal.has(best.edge)) best = c;
    }
    return best.edge;
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
