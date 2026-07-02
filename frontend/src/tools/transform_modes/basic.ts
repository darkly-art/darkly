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
import {
    affineToMat3,
    mat3Apply,
    mat3Inverse,
    mat3ToAffine,
    type Mat3,
} from '../transform_projective';
import { pointInPolygon } from './types';
import type { BBoxPolygon, DragSession, GizmoGeometry, TransformMode } from './types';
import { snapAngleToGrid } from '../../lib/angle';

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

/** Convert a source-local point to canvas space using the geometry. The
 *  carried matrix is a `Mat3`; for basic mode its bottom row is [0,0,1] so the
 *  perspective divide is a no-op. */
function toCanvas(geo: GizmoGeometry, lx: number, ly: number): [number, number] {
    const [cx, cy] = mat3Apply(geo.matrix, lx, ly);
    return [cx + geo.origin[0], cy + geo.origin[1]];
}

function mid(a: [number, number], b: [number, number]): [number, number] {
    return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
}

/** The four source-rect corners in TL, TR, BR, BL order. */
function srcCorners(w: number, ht: number): [number, number][] {
    return [
        [0, 0],
        [w, 0],
        [w, ht],
        [0, ht],
    ];
}

/**
 * Least-squares affine fit of `src` corners → `dst` corners — the affine that
 * minimizes the squared error over the four correspondences (solving the
 * 3-unknown normal equations for `[a, b, tx]` and `[c, d, ty]` independently).
 *
 * This is an honest fit, NOT a "drop the projective bottom row" truncation:
 * truncation ignores the per-pixel perspective divide and yields a visibly
 * wrong parallelogram when switching out of a strongly-warped perspective quad.
 * For an affine (parallelogram) input the fit is exact.
 */
function leastSquaresAffine(src: [number, number][], dst: [number, number][]): Affine2D {
    // Normal-equation accumulators: A = MᵀM (3×3 symmetric, rows of M are
    // [sx, sy, 1]); bx = Mᵀ·dstX, by = Mᵀ·dstY.
    let s00 = 0, s01 = 0, s02 = 0, s11 = 0, s12 = 0, s22 = 0;
    let bx0 = 0, bx1 = 0, bx2 = 0, by0 = 0, by1 = 0, by2 = 0;
    for (let i = 0; i < src.length; i++) {
        const [sx, sy] = src[i];
        const [dx, dy] = dst[i];
        s00 += sx * sx; s01 += sx * sy; s02 += sx;
        s11 += sy * sy; s12 += sy; s22 += 1;
        bx0 += sx * dx; bx1 += sy * dx; bx2 += dx;
        by0 += sx * dy; by1 += sy * dy; by2 += dy;
    }
    const normal: Mat3 = [s00, s01, s02, s01, s11, s12, s02, s12, s22];
    const inv = mat3Inverse(normal);
    // Degenerate source (zero extent) — nothing to fit; keep identity.
    if (!inv) return [1, 0, 0, 0, 1, 0];
    const solve = (b0: number, b1: number, b2: number) =>
        [
            inv[0] * b0 + inv[1] * b1 + inv[2] * b2,
            inv[3] * b0 + inv[4] * b1 + inv[5] * b2,
            inv[6] * b0 + inv[7] * b1 + inv[8] * b2,
        ] as [number, number, number];
    const [a, b, tx] = solve(bx0, bx1, bx2);
    const [c, d, ty] = solve(by0, by1, by2);
    return [a, b, tx, c, d, ty];
}

export const basicMode: TransformMode = {
    tag: 0,
    label: 'Free transform',
    liveCapable: true,

    seedMatrix(geo: GizmoGeometry): Mat3 {
        const src = srcCorners(geo.srcW, geo.srcH);
        const dst = src.map(([x, y]) => mat3Apply(geo.matrix, x, y)) as [number, number][];
        return affineToMat3(leastSquaresAffine(src, dst));
    },

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
        // Internal handle math stays affine; lower the carried Mat3 here and
        // lift the result back to a Mat3 at the `updateDrag` boundary.
        const initialMatrix: Affine2D = mat3ToAffine(geo.matrix);
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

    updateDrag(geo, dragSession, cx, cy, shift): Mat3 {
        const drag = dragSession as BasicDrag;
        const { handle, initialMatrix, startCanvas, anchorLocal, centerCanvas, startAngle } = drag;
        const { srcW, srcH, origin } = geo;

        if (handle === Handle.Body) {
            const dx = cx - startCanvas[0];
            const dy = cy - startCanvas[1];
            return affineToMat3(affineMultiply(affineTranslate(dx, dy), initialMatrix));
        }

        if (handle === Handle.Rotate) {
            let angle = Math.atan2(cy - centerCanvas[1], cx - centerCanvas[0]) - startAngle;
            if (shift) {
                // Snap the content's absolute orientation to 15° marks, not the
                // rotation delta — otherwise the snap grid is anchored to where
                // the gesture began (off by the free rotation already accrued
                // when Shift is pressed mid-drag). `atan2(c, a)` recovers the
                // base rotation baked into `initialMatrix`.
                const base = Math.atan2(initialMatrix[3], initialMatrix[0]);
                angle = snapAngleToGrid(base + angle) - base;
            }
            const cLocal: [number, number] = [srcW / 2, srcH / 2];
            const cOffset = affineTransform(initialMatrix, cLocal[0], cLocal[1]);
            return affineToMat3(
                affineMultiply(
                    affineTranslate(cOffset[0], cOffset[1]),
                    affineMultiply(
                        affineRotate(angle),
                        affineMultiply(affineTranslate(-cOffset[0], -cOffset[1]), initialMatrix),
                    ),
                ),
            );
        }

        const dragLocal = handleLocal(handle, srcW, srcH);
        const mouseOffset: [number, number] = [cx - origin[0], cy - origin[1]];

        const inv = affineInverse(initialMatrix);
        if (!inv) return affineToMat3(initialMatrix);
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

        return affineToMat3(
            affineMultiply(
                initialMatrix,
                affineMultiply(
                    affineTranslate(anchorLocal[0], anchorLocal[1]),
                    affineMultiply(
                        affineScale(sx, sy),
                        affineTranslate(-anchorLocal[0], -anchorLocal[1]),
                    ),
                ),
            ),
        );
    },
};
