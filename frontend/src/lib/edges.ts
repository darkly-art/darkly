/**
 * The four edges of a rectangle, and the two questions anything docking to one
 * needs to ask: which edge is a point nearest, and how far from the rectangle
 * is it. DOM-free so it unit-tests headlessly, and screen/CSS space only: no
 * device-pixel-ratio and no plane coordinates ever pass through here (see
 * `docs/coordinate-systems.md`).
 *
 * Two consumers with different semantics share this: the workspace's panel
 * docking (`ui/workspace/dropZones.ts`), which gates an edge behind a band and
 * otherwise merges into the center, and the tool strip
 * (`ui/tool_strip/geometry.ts`), which always lands on one of the four.
 */

export type Edge = 'left' | 'right' | 'top' | 'bottom';

export interface Rect {
    left: number;
    top: number;
    width: number;
    height: number;
}

/**
 * Which edge of `rect` the point is nearest. Distances are measured
 * perpendicular to each edge, so a point outside the rect still resolves.
 *
 * Ties break in `left, right, top, bottom` order, so an exact corner resolves
 * to a vertical edge. `detectDockingEdge` documents the same rule as "horizontal
 * edges win exact ties", naming the *split* a left/right drop produces rather
 * than the edge itself; the behaviour is identical and pinned by tests on both
 * sides.
 */
export function nearestEdge(x: number, y: number, rect: Rect): Edge {
    const distances: [Edge, number][] = [
        ['left', x - rect.left],
        ['right', rect.left + rect.width - x],
        ['top', y - rect.top],
        ['bottom', rect.top + rect.height - y],
    ];

    let best = distances[0];
    for (const candidate of distances.slice(1)) {
        if (candidate[1] < best[1]) best = candidate;
    }
    return best[0];
}

/**
 * Shortest distance from the point to the rectangle: 0 anywhere inside it,
 * and the straight-line distance to the nearest point on its perimeter outside.
 */
export function distanceToRect(x: number, y: number, rect: Rect): number {
    const dx = Math.max(rect.left - x, 0, x - (rect.left + rect.width));
    const dy = Math.max(rect.top - y, 0, y - (rect.top + rect.height));
    return Math.hypot(dx, dy);
}
