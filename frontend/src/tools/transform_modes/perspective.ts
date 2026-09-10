/**
 * Perspective transform mode: a true projective (vanishing-point) warp driven
 * by dragging the four corners independently. Like `basic`, it is consumer-
 * agnostic: it operates on a `GizmoGeometry` bbox and returns a 3×3 homography
 * the gizmo hands back to whatever binding is wired to it.
 *
 * Following GIMP (`gimpperspectivetool.c`) and Krita
 * (`kis_perspective_transform_strategy.cpp`), every motion recomputes the full
 * homography from all four destination corners; a corner drag moves only the
 * dragged corner. The source is always an axis-aligned rect, so the matrix
 * comes from the closed-form `homographyFromCorners` (no linear solve).
 */
import { OverlayBuilder } from '../../canvas/gpu_overlay';
import { homographyFromCorners, mat3Apply, type Mat3 } from '../transform_projective';
import { pointInPolygon } from './types';
import type { BBoxPolygon, DragSession, GizmoGeometry, TransformMode } from './types';

/** Corner handles map to source-rect corners TL, TR, BR, BL (the order
 *  `homographyFromCorners` expects). `Body` translates the whole quad. */
const enum Handle {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
    Body,
}

/** Source-local positions of the four corners, in TL, TR, BR, BL order. */
function srcCorners(srcW: number, srcH: number): [number, number][] {
    return [
        [0, 0],
        [srcW, 0],
        [srcW, srcH],
        [0, srcH],
    ];
}

/** The four destination corners in the matrix's *local* frame (pre-origin),
 *  i.e. the current homography applied to the source rect corners. */
function destLocalCorners(geo: GizmoGeometry): [number, number][] {
    return srcCorners(geo.srcW, geo.srcH).map(([x, y]) => mat3Apply(geo.matrix, x, y));
}

/** Lift a dest-local corner into canvas space. */
function toCanvas(geo: GizmoGeometry, local: [number, number]): [number, number] {
    return [local[0] + geo.origin[0], local[1] + geo.origin[1]];
}

/** Per-drag state captured at pointer-down. */
interface PerspectiveDrag {
    handle: Handle;
    /** Destination corners (local frame) at drag start, TL/TR/BR/BL. */
    initialCorners: [number, number][];
    /** Last valid homography, returned when a drag goes momentarily
     *  degenerate (`homographyFromCorners` → null). */
    lastValid: Mat3;
    startCanvas: [number, number];
}

/** Recompute the homography from four dest-local corners, falling back to the
 *  last valid matrix if the quad is momentarily degenerate / behind-camera. */
function matrixFromCorners(geo: GizmoGeometry, corners: [number, number][], fallback: Mat3): Mat3 {
    const tuple = corners as [
        [number, number],
        [number, number],
        [number, number],
        [number, number],
    ];
    return homographyFromCorners(geo.srcW, geo.srcH, tuple) ?? fallback;
}

export const perspectiveMode: TransformMode = {
    tag: 1,
    label: 'Perspective',
    liveCapable: true,

    seedMatrix(geo: GizmoGeometry): Mat3 {
        // The current quad's dest-local corners → the homography that
        // reproduces them. Identity geometry yields a plain (perspective-less)
        // homography over the same rect.
        const corners = destLocalCorners(geo) as [
            [number, number],
            [number, number],
            [number, number],
            [number, number],
        ];
        return homographyFromCorners(geo.srcW, geo.srcH, corners) ?? geo.matrix;
    },

    buildOverlay(geo: GizmoGeometry, o: OverlayBuilder): BBoxPolygon {
        const local = destLocalCorners(geo);
        const [tl, tr, br, bl] = local.map((p) => toCanvas(geo, p));

        o.line(tl, tr, { color: '#4af', dash: 6 });
        o.line(tr, br, { color: '#4af', dash: 6 });
        o.line(br, bl, { color: '#4af', dash: 6 });
        o.line(bl, tl, { color: '#4af', dash: 6 });

        o.handle(tl, { id: Handle.TopLeft, cursor: 'crosshair' });
        o.handle(tr, { id: Handle.TopRight, cursor: 'crosshair' });
        o.handle(br, { id: Handle.BottomRight, cursor: 'crosshair' });
        o.handle(bl, { id: Handle.BottomLeft, cursor: 'crosshair' });

        return [tl, tr, br, bl];
    },

    resolveHandle(geo, overlay, bbox, cx, cy) {
        const hit = overlay?.hitTest(cx, cy);
        if (hit) return { id: hit.id as Handle, cursor: hit.cursor };
        if (bbox && pointInPolygon(cx, cy, bbox)) {
            return { id: Handle.Body, cursor: 'move' };
        }
        return { id: Handle.Body, cursor: 'default' };
    },

    beginDrag(geo, handleId, cx, cy): DragSession {
        const drag: PerspectiveDrag = {
            handle: handleId as Handle,
            initialCorners: destLocalCorners(geo),
            lastValid: [...geo.matrix] as Mat3,
            startCanvas: [cx, cy],
        };
        return drag;
    },

    updateDrag(geo, dragSession, cx, cy): Mat3 {
        const drag = dragSession as PerspectiveDrag;
        const corners = drag.initialCorners.map((p) => [...p]) as [number, number][];

        if (drag.handle === Handle.Body) {
            const dx = cx - drag.startCanvas[0];
            const dy = cy - drag.startCanvas[1];
            for (const c of corners) {
                c[0] += dx;
                c[1] += dy;
            }
        } else {
            // Move only the dragged corner to the pointer (in the local frame).
            corners[drag.handle] = [cx - geo.origin[0], cy - geo.origin[1]];
        }

        const m = matrixFromCorners(geo, corners, drag.lastValid);
        drag.lastValid = m;
        return m;
    },
};
