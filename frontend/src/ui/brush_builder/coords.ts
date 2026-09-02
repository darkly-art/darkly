/**
 * Coordinate-system helpers for the brush node graph.
 *
 * The `.node-layer` container in `NodeCanvas.svelte` is CSS-transformed by
 * `translate(panX, panY) scale(zoom)`. Every consumer that needs to convert
 * between a pointer event's `clientX`/`clientY` (post-transform, in screen
 * pixels) and the node-layer's pre-transform layout coordinate system
 * (the units that `clientWidth`, inline SVG user-units, and stored node
 * positions all live in) goes through this module, so no consumer ever
 * has to know what `zoom` is, and adding a new interactive widget inside
 * a node can't reintroduce the divide-by-zoom-by-hand class of bug.
 */

export interface GraphCoords {
    /** Convert a client-pixel point to graph-absolute coords (origin at
     *  the node-layer's untransformed (0,0)). Use for caret-style hit
     *  testing across the whole canvas. */
    clientToGraph(clientX: number, clientY: number): { x: number; y: number };

    /** Convert a client-pixel point to coords local to `el`'s pre-transform
     *  layout system: the same units as `el.clientWidth` and inline SVG
     *  user units. Use for widgets that read pointer position relative
     *  to themselves (curve editors, scrub bars, sliders). */
    clientToElementLocal(el: Element, clientX: number, clientY: number): { x: number; y: number };

    /** Convert a client-pixel delta to a graph-space delta. No translation,
     *  only zoom. Use for drag deltas. */
    clientDeltaToGraph(dx: number, dy: number): { x: number; y: number };

    /** Offset of `child`'s visual center relative to `parent`'s top-left,
     *  expressed in `parent`'s pre-transform layout coords. Use for laying
     *  out wire endpoints at a port dot inside a node. */
    elementCenterInParent(child: Element, parent: Element): { x: number; y: number };
}

export function createGraphCoords(opts: {
    nodeLayerEl: () => HTMLElement;
    zoom: () => number;
}): GraphCoords {
    return {
        clientToGraph(clientX, clientY) {
            const r = opts.nodeLayerEl().getBoundingClientRect();
            const z = opts.zoom();
            return { x: (clientX - r.left) / z, y: (clientY - r.top) / z };
        },
        clientToElementLocal(el, clientX, clientY) {
            const r = el.getBoundingClientRect();
            const z = opts.zoom();
            return { x: (clientX - r.left) / z, y: (clientY - r.top) / z };
        },
        clientDeltaToGraph(dx, dy) {
            const z = opts.zoom();
            return { x: dx / z, y: dy / z };
        },
        elementCenterInParent(child, parent) {
            const c = child.getBoundingClientRect();
            const p = parent.getBoundingClientRect();
            const z = opts.zoom();
            return {
                x: ((c.left + c.width / 2) - p.left) / z,
                y: ((c.top + c.height / 2) - p.top) / z,
            };
        },
    };
}
