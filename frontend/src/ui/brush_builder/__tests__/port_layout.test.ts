import { describe, it, expect } from 'vitest';
import { portOffsetInGraph } from '../port_layout';

/**
 * Regression: when the node-canvas was zoomed and a new node was added,
 * its port offsets were stored in screen pixels (multiplied by zoom),
 * making wire endpoints land right-and-down of the dot. The helper must
 * return graph-space offsets at any zoom.
 */
describe('portOffsetInGraph', () => {
    it('returns the dot-center-to-node-left delta at zoom 1', () => {
        // Node at screen x=100, dot 140px to the right of its left edge.
        const nodeRect = { left: 100, top: 50 };
        const dotRect = { left: 235, top: 95, width: 10, height: 10 };
        expect(portOffsetInGraph(dotRect, nodeRect, 1)).toEqual({ x: 140, y: 50 });
    });

    it('divides by zoom so the offset is graph-space, not screen-space', () => {
        // At zoom=2, a 140-graph-pixel right port appears 280px right of
        // the node on screen. The stored offset must be 140 (graph), not
        // 280 (screen) — otherwise wire paths (drawn in graph space, then
        // SVG-scaled) double-apply the zoom.
        const nodeRect = { left: 0, top: 0 };
        const dotRect = { left: 270, top: 90, width: 20, height: 20 };
        expect(portOffsetInGraph(dotRect, nodeRect, 2)).toEqual({ x: 140, y: 50 });
    });

    it('is zoom-invariant for the same dot at the same graph position', () => {
        // Same node+dot in graph space, measured at three zooms — same answer.
        const measureAtZoom = (z: number) => ({
            nodeRect: { left: 1000, top: 200 },
            // Dot is 140 graph-px right and 50 graph-px down of node-left.
            dotRect: {
                left: 1000 + 140 * z - 5 * z,
                top: 200 + 50 * z - 5 * z,
                width: 10 * z,
                height: 10 * z,
            },
        });
        for (const z of [0.5, 1, 2, 3]) {
            const { nodeRect, dotRect } = measureAtZoom(z);
            expect(portOffsetInGraph(dotRect, nodeRect, z)).toEqual({ x: 140, y: 50 });
        }
    });
});
