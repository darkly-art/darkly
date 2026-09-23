/**
 * The only place a dock `Edge` becomes numbers. Pure and DOM-free so it
 * unit-tests headlessly, and screen/CSS space throughout: no device-pixel-ratio
 * and no plane coordinates, per `docs/coordinate-systems.md`. Every other
 * TypeScript consumer passes an `Edge` around opaquely.
 */
import { distanceToRect, nearestEdge, type Edge, type Rect } from '../../lib/edges';

/** True when the strip lays out as a row, i.e. docked top or bottom.
 *
 *  Deliberately not called `isHorizontal`: `dropZones.ts` uses "horizontal" for
 *  left/right (the edges that split horizontally), which is the opposite set.
 *  The two modules share an `Edge` type, so one word with two senses across them
 *  would be a long-lived trap. */
export function isRow(edge: Edge): boolean {
    return edge === 'top' || edge === 'bottom';
}

/** Length of the region along the strip's free axis. */
export function regionLength(edge: Edge, region: Rect): number {
    return isRow(edge) ? region.width : region.height;
}

/**
 * Where the strip's leading corner sits along its edge, in pixels from the
 * region's start.
 *
 * `offset` is a fraction of the *travel* the strip has, not of the edge, so it
 * is travel-proportional rather than region-proportional: at offset 0.25 a 300px
 * strip sits at 32.5% of a 1000px region and at 40% of a 500px one. That is the
 * point. It guarantees no offset at any region length ever puts part of the
 * strip outside the region, and when there is no travel at all the strip pins to
 * the start rather than centring into a two-sided overflow (which is what the
 * `safe center` it replaces did).
 */
export function stripPos(offset: number, regionLen: number, stripLen: number): number {
    const travel = Math.max(0, regionLen - stripLen);
    return Math.min(Math.max(offset, 0), 1) * travel;
}

/** Invert `stripPos`: the offset that puts the strip's leading corner at `pos`.
 *  Zero travel has no meaningful offset, so it reports the start. */
export function offsetForPos(pos: number, regionLen: number, stripLen: number): number {
    const travel = Math.max(0, regionLen - stripLen);
    if (travel === 0) return 0;
    return Math.min(Math.max(pos / travel, 0), 1);
}

/**
 * The strip's untucked box in client coordinates. Untucked on purpose: the peek
 * measures against this, so the trigger zone does not move as the strip slides.
 */
export function stripBox(
    edge: Edge,
    region: Rect,
    stripLen: number,
    stripThickness: number,
    pos: number,
): Rect {
    if (isRow(edge)) {
        return {
            left: region.left + pos,
            top: edge === 'top' ? region.top : region.top + region.height - stripThickness,
            width: stripLen,
            height: stripThickness,
        };
    }
    return {
        left: edge === 'left' ? region.left : region.left + region.width - stripThickness,
        top: region.top + pos,
        width: stripThickness,
        height: stripLen,
    };
}

/**
 * The placement a drag resolves to. `grabAlong` is where along the strip the
 * press landed, so the strip keeps its grip under the pointer instead of
 * jumping to centre itself.
 *
 * On an edge flip the caller must reset `grabAlong` to the strip's midpoint:
 * both it and `stripLen` are measured along the axis the strip just left, and
 * the new axis's length is not known until the next layout.
 */
export function placementFromPointer(
    x: number,
    y: number,
    region: Rect,
    stripLen: number,
    grabAlong: number,
): { edge: Edge; offset: number } {
    const edge = nearestEdge(x, y, region);
    const along = isRow(edge) ? x - region.left : y - region.top;
    return {
        edge,
        offset: offsetForPos(along - grabAlong, regionLength(edge, region), stripLen),
    };
}

/**
 * Hysteretic proximity test: slide out within `near` of the strip's box, tuck
 * back only past `far`, hold state in between so a pointer hovering the boundary
 * does not flutter.
 *
 * Distance is to the strip's box, not to the docked edge. That is what makes
 * proximity tolerable: painting in the bottom-left corner while the strip is
 * centred on the left edge leaves it tucked, because the pointer is far from the
 * strip even though it is right against the edge.
 */
export function peekOut(current: boolean, x: number, y: number, box: Rect, near: number, far: number): boolean {
    const d = distanceToRect(x, y, box);
    if (d <= near) return true;
    if (d > far) return false;
    return current;
}
