/**
 * Transform-mode strategy interface.
 *
 * A mode owns everything about *how pointer input becomes transform numbers*
 * for one interaction style: which handles exist, how dragging a handle mutates
 * the matrix, and how the on-canvas overlay is drawn. `basic` (affine: pan /
 * scale / rotate) and `perspective` (four-corner homography) are the modes
 * today; `warp` would slot in as a new file + one entry in `index.ts`, with
 * the gizmo shell unchanged.
 *
 * Modes are consumer-agnostic — they know nothing about voids, floating, or
 * layers. They operate purely on a bounding box (`GizmoGeometry`) and produce
 * an updated 3×3 matrix that the gizmo hands back to whatever
 * `TransformBinding` is wired to it.
 */
import type { OverlayBuilder } from '../../canvas/gpu_overlay';
import type { Mat3 } from '../transform_projective';

/** The current transform geometry the gizmo draws around. */
export interface GizmoGeometry {
    /** Current transform (local → local), edited by dragging. Always a 3×3
     *  projective matrix; affine modes carry it with bottom row [0,0,1]. */
    matrix: Mat3;
    /** Source origin in plane (canvas) space — the matrix's local frame anchor. */
    origin: [number, number];
    /** Source extent in local pixels. */
    srcW: number;
    srcH: number;
}

/** Transformed bounding-box corners in canvas space (tl, tr, br, bl). */
export type BBoxPolygon = [
    [number, number],
    [number, number],
    [number, number],
    [number, number],
];

/** Opaque per-mode drag state; the gizmo stores and round-trips it verbatim. */
export type DragSession = unknown;

export interface TransformMode {
    /** Stable tag matching the Rust `Transform::mode_tag` wire value. */
    readonly tag: number;

    /** Human-readable name for the mode-switch menu (e.g. `"Free transform"`,
     *  `"Perspective"`). */
    readonly label: string;

    /** Whether this mode renders correctly for a **live** (per-frame
     *  composited) consumer such as a void. The type-owned capability seam:
     *  consumers ask the mode, they never branch on its identity. `basic` and
     *  `perspective` are both live-capable; a future warp mode may not be. */
    readonly liveCapable: boolean;

    /** Produce this mode's matrix from the current geometry when switching
     *  INTO it — i.e. the matrix that best reproduces the geometry's current
     *  destination quad under this mode's representation. */
    seedMatrix(geo: GizmoGeometry): Mat3;

    /**
     * Push the overlay primitives (bbox lines + handles) for the current
     * geometry and return the bbox polygon used for inside/outside hit tests.
     */
    buildOverlay(geo: GizmoGeometry, o: OverlayBuilder): BBoxPolygon;

    /** Resolve which handle + cursor a canvas point corresponds to. */
    resolveHandle(
        geo: GizmoGeometry,
        overlay: OverlayBuilder | null,
        bbox: BBoxPolygon | null,
        cx: number,
        cy: number,
    ): { id: number; cursor: string };

    /** Begin a drag on `handleId` at canvas `(cx, cy)`. */
    beginDrag(geo: GizmoGeometry, handleId: number, cx: number, cy: number): DragSession;

    /** Advance a drag → return the new 3×3 matrix. */
    updateDrag(
        geo: GizmoGeometry,
        drag: DragSession,
        cx: number,
        cy: number,
        shift: boolean,
    ): Mat3;
}

/** Ray-casting point-in-polygon test. Shared by the modes (inside-bbox =
 *  body-translate) and the gizmo's right-click hit test. */
export function pointInPolygon(
    px: number,
    py: number,
    poly: readonly [number, number][],
): boolean {
    let inside = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
        const [xi, yi] = poly[i];
        const [xj, yj] = poly[j];
        if (yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) {
            inside = !inside;
        }
    }
    return inside;
}
