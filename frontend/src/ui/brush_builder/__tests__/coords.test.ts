import { describe, it, expect } from 'vitest';
import { createGraphCoords } from '../coords';

/**
 * Vitest's default node environment has no DOM, so we fake just enough
 * of `Element` (a `getBoundingClientRect`) for these pure helpers. The
 * helpers never touch any other DOM property.
 */
function fakeEl(rect: { left: number; top: number; width?: number; height?: number }): Element {
    const r = {
        left: rect.left,
        top: rect.top,
        width: rect.width ?? 0,
        height: rect.height ?? 0,
        right: rect.left + (rect.width ?? 0),
        bottom: rect.top + (rect.height ?? 0),
        x: rect.left,
        y: rect.top,
    };
    return { getBoundingClientRect: () => r as DOMRect } as unknown as Element;
}

/** Build a coords instance that mimics a NodeCanvas at the given pan/zoom.
 *  The .node-layer is at `inset: 0` inside a container offset (containerLeft,
 *  containerTop) from the page origin; after `translate(panX, panY) scale(zoom)`
 *  with `transform-origin: 0 0`, its visual top-left lands at
 *  `(containerLeft + panX, containerTop + panY)`. */
function makeCoords(opts: {
    containerLeft: number;
    containerTop: number;
    containerWidth: number;
    containerHeight: number;
    panX: number;
    panY: number;
    zoom: number;
}) {
    const nodeLayerEl = fakeEl({
        left: opts.containerLeft + opts.panX,
        top: opts.containerTop + opts.panY,
        width: opts.containerWidth * opts.zoom,
        height: opts.containerHeight * opts.zoom,
    });
    return createGraphCoords({
        nodeLayerEl: () => nodeLayerEl as unknown as HTMLElement,
        zoom: () => opts.zoom,
    });
}

describe('clientToGraph', () => {
    it('returns (0,0) for the node-layer top-left at zoom 1, no pan', () => {
        const coords = makeCoords({
            containerLeft: 100, containerTop: 50,
            containerWidth: 800, containerHeight: 600,
            panX: 0, panY: 0, zoom: 1,
        });
        expect(coords.clientToGraph(100, 50)).toEqual({ x: 0, y: 0 });
    });

    it('subtracts pan and container offset', () => {
        const coords = makeCoords({
            containerLeft: 100, containerTop: 50,
            containerWidth: 800, containerHeight: 600,
            panX: 30, panY: 20, zoom: 1,
        });
        // Click at screen (200, 100). Container at (100,50), pan (30,20)
        // → graph (200-100-30, 100-50-20) = (70, 30).
        expect(coords.clientToGraph(200, 100)).toEqual({ x: 70, y: 30 });
    });

    it('divides by zoom so a screen click maps to the underlying graph point', () => {
        // A graph point (gx, gy) renders at screen (containerLeft + panX + gx*zoom, ...).
        // Inverse must recover gx regardless of zoom.
        for (const z of [0.5, 1, 2, 3]) {
            const coords = makeCoords({
                containerLeft: 100, containerTop: 50,
                containerWidth: 800, containerHeight: 600,
                panX: 30, panY: 20, zoom: z,
            });
            const gx = 140, gy = 75;
            const screenX = 100 + 30 + gx * z;
            const screenY = 50 + 20 + gy * z;
            const out = coords.clientToGraph(screenX, screenY);
            expect(out.x).toBeCloseTo(gx);
            expect(out.y).toBeCloseTo(gy);
        }
    });
});

describe('clientToElementLocal', () => {
    /**
     * Regression: at zoom != 1, CurveEditor.svgToNorm was reading
     * `e.clientX - rect.left` (screen pixels, post-zoom-scale) but treating
     * it as a layout-pixel offset against its `clientWidth`-derived inner
     * width. Dragged curve points warped away from the cursor. This helper
     * must return the element's pre-transform local coords at any zoom.
     */
    it('returns (0,0) at the element top-left at any zoom', () => {
        for (const z of [0.5, 1, 2]) {
            const coords = makeCoords({
                containerLeft: 0, containerTop: 0,
                containerWidth: 800, containerHeight: 600,
                panX: 0, panY: 0, zoom: z,
            });
            // A 128x128 layout SVG inside the node-layer renders at 128*z visual pixels.
            const svg = fakeEl({ left: 200, top: 100, width: 128 * z, height: 128 * z });
            expect(coords.clientToElementLocal(svg, 200, 100)).toEqual({ x: 0, y: 0 });
        }
    });

    it('returns the layout-pixel center for a click at the visual center', () => {
        // Reproduces the CurveEditor scenario: SVG laid out 128x128 in the
        // node's local coords; ancestor `.node-layer` scaled by zoom.
        for (const z of [0.5, 1, 2]) {
            const coords = makeCoords({
                containerLeft: 0, containerTop: 0,
                containerWidth: 800, containerHeight: 600,
                panX: 0, panY: 0, zoom: z,
            });
            const svg = fakeEl({ left: 200, top: 100, width: 128 * z, height: 128 * z });
            const visualCenterX = 200 + 64 * z;
            const visualCenterY = 100 + 64 * z;
            const out = coords.clientToElementLocal(svg, visualCenterX, visualCenterY);
            expect(out.x).toBeCloseTo(64);
            expect(out.y).toBeCloseTo(64);
        }
    });
});

describe('clientDeltaToGraph', () => {
    it('is (dx/zoom, dy/zoom) regardless of pan', () => {
        for (const z of [0.5, 1, 2, 3]) {
            const coords = makeCoords({
                containerLeft: 100, containerTop: 50,
                containerWidth: 800, containerHeight: 600,
                panX: 30, panY: 20, zoom: z,
            });
            expect(coords.clientDeltaToGraph(100, 60)).toEqual({ x: 100 / z, y: 60 / z });
        }
    });
});

describe('elementCenterInParent', () => {
    // Ported from the original port_layout regression suite.
    it('returns the dot-center-to-node-left delta at zoom 1', () => {
        const coords = makeCoords({
            containerLeft: 0, containerTop: 0,
            containerWidth: 1000, containerHeight: 800,
            panX: 0, panY: 0, zoom: 1,
        });
        const nodeEl = fakeEl({ left: 100, top: 50, width: 200, height: 100 });
        const dotEl  = fakeEl({ left: 235, top: 95, width: 10, height: 10 });
        expect(coords.elementCenterInParent(dotEl, nodeEl)).toEqual({ x: 140, y: 50 });
    });

    it('divides by zoom so the offset is graph-space, not screen-space', () => {
        const coords = makeCoords({
            containerLeft: 0, containerTop: 0,
            containerWidth: 1000, containerHeight: 800,
            panX: 0, panY: 0, zoom: 2,
        });
        // At zoom=2, a 140-graph-pixel right port appears 280px right of
        // the node on screen. The stored offset must be 140 (graph), not
        // 280 (screen); otherwise wire paths double-apply the zoom.
        const nodeEl = fakeEl({ left: 0, top: 0, width: 400, height: 200 });
        const dotEl  = fakeEl({ left: 270, top: 90, width: 20, height: 20 });
        expect(coords.elementCenterInParent(dotEl, nodeEl)).toEqual({ x: 140, y: 50 });
    });

    it('is zoom-invariant for the same dot at the same graph position', () => {
        for (const z of [0.5, 1, 2, 3]) {
            const coords = makeCoords({
                containerLeft: 0, containerTop: 0,
                containerWidth: 2000, containerHeight: 1500,
                panX: 0, panY: 0, zoom: z,
            });
            const nodeEl = fakeEl({ left: 1000, top: 200, width: 300 * z, height: 150 * z });
            // Dot is 140 graph-px right and 50 graph-px down of node-left;
            // its visual size is 10*zoom, so the dot's screen left is
            // (nodeLeft + 140*z) - (10*z)/2 = nodeLeft + 135*z.
            const dotEl = fakeEl({
                left: 1000 + 135 * z,
                top: 200 + 45 * z,
                width: 10 * z,
                height: 10 * z,
            });
            const out = coords.elementCenterInParent(dotEl, nodeEl);
            expect(out.x).toBeCloseTo(140);
            expect(out.y).toBeCloseTo(50);
        }
    });
});
