/**
 * Transform-mode strategy interface.
 *
 * A mode owns everything about *how pointer input becomes transform numbers*
 * for one interaction style: which handles exist, how dragging a handle mutates
 * the affine, and how the on-canvas overlay is drawn. `basic` (affine: pan /
 * scale / rotate) is the only mode today; `perspective` and `warp` slot in as
 * new files + one entry in `index.ts`, with the gizmo shell unchanged.
 *
 * Modes are consumer-agnostic — they know nothing about voids, floating, or
 * layers. They operate purely on a bounding box (`GizmoGeometry`) and produce
 * an updated affine that the gizmo hands back to whatever `TransformBinding`
 * is wired to it.
 */
import type { OverlayBuilder } from '../../canvas/gpu_overlay';
import type { Affine2D } from '../transform_affine';

/** The current transform geometry the gizmo draws around. */
export interface GizmoGeometry {
    /** Current affine (local → local), edited by dragging. */
    matrix: Affine2D;
    /** Source origin in plane (canvas) space — the affine's local frame anchor. */
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

    /** Advance a drag → return the new affine matrix. */
    updateDrag(
        geo: GizmoGeometry,
        drag: DragSession,
        cx: number,
        cy: number,
        shift: boolean,
    ): Affine2D;
}
