/**
 * Basic transform mode — affine pan / scale / rotate via corner/edge/rotate
 * handles. This is the default mode; perspective and warp will be sibling files.
 *
 * Extracted verbatim (logic-wise) from the original floating transform tool —
 * but now consumer-agnostic: it operates on a `GizmoGeometry` bbox and returns
 * affine matrices, with no knowledge of what's being transformed.
 */
import { OverlayBuilder } from '../../canvas/gpu_overlay';
import {
    affineInverse,
    affineMultiply,
    affineRotate,
    affineScale,
    affineTransform,
    affineTranslate,
    type Affine2D,
} from '../transform_affine';
import type { BBoxPolygon, DragSession, GizmoGeometry, TransformMode } from './types';

const enum Handle {
    TopLeft, Top, TopRight, Right, BottomRight, Bottom, BottomLeft, Left,
    Rotate,
    Body,
}

const CORNER_HANDLES = [Handle.TopLeft, Handle.TopRight, Handle.BottomRight, Handle.BottomLeft];

/** Source-local coordinates for each handle. */
function handleLocal(h: Handle, w: number, ht: number): [number, number] {
    switch (h) {
        case Handle.TopLeft:     return [0, 0];
        case Handle.Top:         return [w / 2, 0];
        case Handle.TopRight:    return [w, 0];
        case Handle.Right:       return [w, ht / 2];
        case Handle.BottomRight: return [w, ht];
        case Handle.Bottom:      return [w / 2, ht];
        case Handle.BottomLeft:  return [0, ht];
        case Handle.Left:        return [0, ht / 2];
        case Handle.Rotate:      return [w / 2, ht / 2];
        case Handle.Body:        return [w / 2, ht / 2];
    }
}

/** Opposite anchor for scale operations. */
function oppositeHandle(h: Handle): Handle {
    switch (h) {
        case Handle.TopLeft:     return Handle.BottomRight;
        case Handle.Top:         return Handle.Bottom;
        case Handle.TopRight:    return Handle.BottomLeft;
        case Handle.Right:       return Handle.Left;
        case Handle.BottomRight: return Handle.TopLeft;
        case Handle.Bottom:      return Handle.Top;
        case Handle.BottomLeft:  return Handle.TopRight;
        case Handle.Left:        return Handle.Right;
        default:                 return Handle.Body;
    }
}

function cursorForHandle(h: Handle): string {
    switch (h) {
        case Handle.TopLeft:
        case Handle.BottomRight: return 'nwse-resize';
        case Handle.TopRight:
        case Handle.BottomLeft:  return 'nesw-resize';
        case Handle.Top:
        case Handle.Bottom:      return 'ns-resize';
        case Handle.Left:
        case Handle.Right:       return 'ew-resize';
        case Handle.Rotate:      return ROTATE_CURSOR;
        case Handle.Body:        return 'move';
    }
}

/**
 * Rotation cursor used when hovering outside the transform bounding box
 * (matches Krita's free-transform behavior). Browsers have no standard
 * rotation cursor, so we use an inline SVG.
 */
const ROTATE_CURSOR =
    "url(\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='white' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'><path d='M21 12a9 9 0 1 1-3.5-7.1'/><polyline points='21 3 21 9 15 9'/></svg>\") 12 12, grab";

/** Per-drag state captured at pointer-down. */
interface BasicDrag {
    handle: Handle;
    initialMatrix: Affine2D;
    startCanvas: [number, number];
    anchorLocal: [number, number];
    anchorCanvas: [number, number];
    centerCanvas: [number, number];
    startAngle: number;
}

/** Convert a source-local point to canvas space using the geometry. */
function toCanvas(geo: GizmoGeometry, lx: number, ly: number): [number, number] {
    const [cx, cy] = affineTransform(geo.matrix, lx, ly);
    return [cx + geo.origin[0], cy + geo.origin[1]];
}

function mid(a: [number, number], b: [number, number]): [number, number] {
    return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
}

/** Ray-casting point-in-polygon test. */
function pointInPolygon(px: number, py: number, poly: readonly [number, number][]): boolean {
    let inside = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
        const [xi, yi] = poly[i];
        const [xj, yj] = poly[j];
        if (((yi > py) !== (yj > py)) &&
            (px < ((xj - xi) * (py - yi)) / (yj - yi) + xi)) {
            inside = !inside;
        }
    }
    return inside;
}

export const basicMode: TransformMode = {
    tag: 0,

    buildOverlay(geo: GizmoGeometry, o: OverlayBuilder): BBoxPolygon {
        const { srcW, srcH } = geo;
        const tl = toCanvas(geo, 0, 0);
        const tr = toCanvas(geo, srcW, 0);
        const br = toCanvas(geo, srcW, srcH);
        const bl = toCanvas(geo, 0, srcH);

        const tm = mid(tl, tr);
        const rm = mid(tr, br);
        const bm = mid(br, bl);
        const lm = mid(bl, tl);

        o.line(tl, tr, { color: '#4af', dash: 6 });
        o.line(tr, br, { color: '#4af', dash: 6 });
        o.line(br, bl, { color: '#4af', dash: 6 });
        o.line(bl, tl, { color: '#4af', dash: 6 });

        o.handle(tl, { id: Handle.TopLeft,     cursor: 'nwse-resize' });
        o.handle(tr, { id: Handle.TopRight,    cursor: 'nesw-resize' });
        o.handle(br, { id: Handle.BottomRight, cursor: 'nwse-resize' });
        o.handle(bl, { id: Handle.BottomLeft,  cursor: 'nesw-resize' });

        o.handle(tm, { id: Handle.Top,    cursor: 'ns-resize', radius: 4 });
        o.handle(rm, { id: Handle.Right,  cursor: 'ew-resize', radius: 4 });
        o.handle(bm, { id: Handle.Bottom, cursor: 'ns-resize', radius: 4 });
        o.handle(lm, { id: Handle.Left,   cursor: 'ew-resize', radius: 4 });

        return [tl, tr, br, bl];
    },

    resolveHandle(geo, overlay, bbox, cx, cy) {
        const hit = overlay?.hitTest(cx, cy);
        if (hit) return { id: hit.id as Handle, cursor: hit.cursor };
        if (bbox && pointInPolygon(cx, cy, bbox)) {
            return { id: Handle.Body, cursor: cursorForHandle(Handle.Body) };
        }
        return { id: Handle.Rotate, cursor: cursorForHandle(Handle.Rotate) };
    },

    beginDrag(geo, handleId, cx, cy): DragSession {
        const handle = handleId as Handle;
        const initialMatrix: Affine2D = [...geo.matrix];
        const startCanvas: [number, number] = [cx, cy];

        const anchorLocal = handleLocal(oppositeHandle(handle), geo.srcW, geo.srcH);
        const anchorCanvas = toCanvas(geo, anchorLocal[0], anchorLocal[1]);

        const centerCanvas = toCanvas(geo, geo.srcW / 2, geo.srcH / 2);
        const startAngle = Math.atan2(cy - centerCanvas[1], cx - centerCanvas[0]);

        const drag: BasicDrag = {
            handle, initialMatrix, startCanvas, anchorLocal, anchorCanvas, centerCanvas, startAngle,
        };
        return drag;
    },

    updateDrag(geo, dragSession, cx, cy, shift): Affine2D {
        const drag = dragSession as BasicDrag;
        const { handle, initialMatrix, startCanvas, anchorLocal, centerCanvas, startAngle } = drag;
        const { srcW, srcH, origin } = geo;

        if (handle === Handle.Body) {
            const dx = cx - startCanvas[0];
            const dy = cy - startCanvas[1];
            return affineMultiply(affineTranslate(dx, dy), initialMatrix);
        }

        if (handle === Handle.Rotate) {
            let angle = Math.atan2(cy - centerCanvas[1], cx - centerCanvas[0]) - startAngle;
            if (shift) {
                const snap = Math.PI / 12;
                angle = Math.round(angle / snap) * snap;
            }
            const cLocal: [number, number] = [srcW / 2, srcH / 2];
            const cOffset = affineTransform(initialMatrix, cLocal[0], cLocal[1]);
            return affineMultiply(
                affineTranslate(cOffset[0], cOffset[1]),
                affineMultiply(
                    affineRotate(angle),
                    affineMultiply(affineTranslate(-cOffset[0], -cOffset[1]), initialMatrix),
                ),
            );
        }

        const dragLocal = handleLocal(handle, srcW, srcH);
        const mouseOffset: [number, number] = [cx - origin[0], cy - origin[1]];

        const inv = affineInverse(initialMatrix);
        if (!inv) return initialMatrix;
        const mouseLocal = affineTransform(inv, mouseOffset[0], mouseOffset[1]);

        const dLocalX = dragLocal[0] - anchorLocal[0];
        const dLocalY = dragLocal[1] - anchorLocal[1];
        const dMouseLocalX = mouseLocal[0] - anchorLocal[0];
        const dMouseLocalY = mouseLocal[1] - anchorLocal[1];

        let sx = Math.abs(dLocalX) > 0.01 ? dMouseLocalX / dLocalX : 1;
        let sy = Math.abs(dLocalY) > 0.01 ? dMouseLocalY / dLocalY : 1;

        if (handle === Handle.Top || handle === Handle.Bottom) sx = 1;
        if (handle === Handle.Left || handle === Handle.Right) sy = 1;

        if (shift && CORNER_HANDLES.includes(handle)) {
            const uniform = Math.max(Math.abs(sx), Math.abs(sy));
            sx = uniform * Math.sign(sx || 1);
            sy = uniform * Math.sign(sy || 1);
        }

        return affineMultiply(
            initialMatrix,
            affineMultiply(
                affineTranslate(anchorLocal[0], anchorLocal[1]),
                affineMultiply(affineScale(sx, sy), affineTranslate(-anchorLocal[0], -anchorLocal[1])),
            ),
        );
    },
};
