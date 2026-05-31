/**
 * Convert a port dot's screen-space position (from `getBoundingClientRect`)
 * into a graph-space offset relative to its containing node.
 *
 * The node-layer has `transform: scale(zoom)`, so bounding rects are in
 * screen pixels. Wire paths are rendered inside an SVG that also scales
 * by zoom, meaning path coords must be in graph space — dividing the
 * screen-pixel delta by zoom converts.
 */
export function portOffsetInGraph(
    dotRect: { left: number; top: number; width: number; height: number },
    nodeRect: { left: number; top: number },
    zoom: number,
): { x: number; y: number } {
    return {
        x: ((dotRect.left + dotRect.width / 2) - nodeRect.left) / zoom,
        y: ((dotRect.top + dotRect.height / 2) - nodeRect.top) / zoom,
    };
}
