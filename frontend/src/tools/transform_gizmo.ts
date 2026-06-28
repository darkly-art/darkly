/**
 * Generic transform gizmo — the consumer-agnostic helper.
 *
 * It takes a bounding box + an initial transform + pointer input and outputs
 * numbers (an updated affine), which it hands to whatever `TransformBinding` is
 * wired to it. It has ZERO knowledge of voids, floating, layers, or the
 * document — it imports only the overlay renderer, the affine util, and the
 * mode registry. A consumer owns a transform and supplies a binding; the gizmo
 * just drives it.
 *
 * Reads cross the async request/response transport, so `read()` is a Promise:
 * the gizmo caches the geometry, interaction uses the cache synchronously, and
 * live updates are fire-and-forget through the binding.
 */
import { app } from '../state/app.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import {
    homographyFromCorners,
    mat3Apply,
    MAT3_IDENTITY,
    type Mat3,
} from './transform_projective';
import {
    modeForTag,
    pointInPolygon,
    type BBoxPolygon,
    type DragSession,
    type GizmoGeometry,
    type TransformMode,
} from './transform_modes';

/** Wire mode tag for the perspective (homography) sub-mode. */
const PERSPECTIVE_TAG = 1;

/**
 * The seam between the gizmo and a consumer. The gizmo reads the current bbox +
 * transform, emits live updates while dragging, and commits/cancels on exit.
 * Implementations live next to their consumers (see `transform_bindings.ts`).
 */
export interface TransformBinding {
    /** Current bbox + transform, or `null` if the target is no longer valid. */
    read(): Promise<{
        origin: [number, number];
        w: number;
        h: number;
        mode: number;
        matrix: Mat3;
    } | null>;
    /** Live preview: push an updated 3×3 matrix for the given mode. */
    update(matrix: Mat3, modeTag: number): void;
    /** Finalize. */
    commit(): void;
    /** Abandon (restore the pre-edit state). */
    cancel(): void;
}

export class TransformGizmo {
    private canvasEl: HTMLCanvasElement;
    private binding: TransformBinding | null = null;
    private mode: TransformMode = modeForTag(0);
    private geo: GizmoGeometry = { matrix: [...MAT3_IDENTITY], origin: [0, 0], srcW: 0, srcH: 0 };
    private drag: DragSession | null = null;
    private overlay: OverlayBuilder | null = null;
    private bbox: BBoxPolygon | null = null;

    constructor(canvasEl: HTMLCanvasElement) {
        this.canvasEl = canvasEl;
    }

    get active(): boolean {
        return this.binding !== null;
    }

    /** Wire a consumer's binding and seed geometry from it. Returns whether the
     *  gizmo became active (the binding had a valid target). */
    async attach(binding: TransformBinding): Promise<boolean> {
        this.binding = binding;
        if ((await this.adopt(binding)) !== 'adopted') {
            // 'stale' means we were re-attached to a newer binding mid-read —
            // leave that one alone; only clear if we're still on this binding.
            if (this.binding === binding) this.clear();
            return false;
        }
        this.rebuildOverlay();
        return true;
    }

    /** Pull the current bbox + transform from `binding` into local geometry.
     *  The read crosses the async transport, so the gizmo can be committed /
     *  cancelled (cleared) or re-attached to a different binding while it's in
     *  flight: `'stale'` reports that, so a resolved read from a torn-down
     *  session can't resurrect the overlay. `'gone'` means the binding's target
     *  is genuinely no longer valid (e.g. floating committed by an undo). */
    private async adopt(binding: TransformBinding): Promise<'adopted' | 'gone' | 'stale'> {
        const info = await binding.read();
        if (this.binding !== binding) return 'stale';
        if (!info) return 'gone';
        this.geo = {
            matrix: [...info.matrix],
            origin: [...info.origin],
            srcW: info.w,
            srcH: info.h,
        };
        this.mode = modeForTag(info.mode);
        return 'adopted';
    }

    /** Per-frame reconcile: drop if the target vanished (e.g. floating
     *  committed by an unrelated edit / undo), otherwise resync geometry when
     *  idle (so an external change like undo is reflected) and redraw. */
    async frame(): Promise<void> {
        const binding = this.binding;
        if (!binding) return;
        if (!this.drag) {
            const status = await this.adopt(binding);
            // A commit / cancel / re-attach landed while the read was in flight;
            // the resolved geometry belongs to a session that's already gone.
            if (status === 'stale' || this.binding !== binding) return;
            if (status === 'gone') {
                this.clear();
                return;
            }
        }
        this.rebuildOverlay();
    }

    /** Returns true if the gizmo claimed the pointer (it's active). */
    pointerDown(cx: number, cy: number): boolean {
        if (!this.binding) return false;
        const { id } = this.mode.resolveHandle(this.geo, this.overlay, this.bbox, cx, cy);
        this.drag = this.mode.beginDrag(this.geo, id, cx, cy);
        return true;
    }

    /** Whether canvas point `(cx, cy)` is inside the current transform bbox. */
    isInside(cx: number, cy: number): boolean {
        return this.bbox ? pointInPolygon(cx, cy, this.bbox) : false;
    }

    /**
     * Switch the gizmo into perspective (four-corner) mode. Seeds the corners
     * from the current bbox and **pushes a `Perspective` transform through the
     * binding** — mode is document-derived (the stored `Transform`), not a
     * session-local flag, so the next `adopt()` reads `mode: 1` back and the
     * gizmo stays in perspective rather than snapping to basic. One-way:
     * Escape / Enter / re-entering transform returns to basic.
     */
    enterPerspective(): void {
        if (!this.binding || this.mode.tag === PERSPECTIVE_TAG) return;
        const { srcW, srcH } = this.geo;
        // Dest corners that reproduce the current shape (TL, TR, BR, BL).
        const corners = (
            [
                [0, 0],
                [srcW, 0],
                [srcW, srcH],
                [0, srcH],
            ] as [number, number][]
        ).map((p) => mat3Apply(this.geo.matrix, p[0], p[1])) as [
            [number, number],
            [number, number],
            [number, number],
            [number, number],
        ];
        const h = homographyFromCorners(srcW, srcH, corners);
        if (!h) return;
        this.geo.matrix = h;
        this.mode = modeForTag(PERSPECTIVE_TAG);
        this.binding.update(h, PERSPECTIVE_TAG);
        this.rebuildOverlay();
        app.requestFrame();
    }

    pointerMove(cx: number, cy: number, shift: boolean): void {
        if (!this.binding) return;
        if (this.drag != null) {
            const m = this.mode.updateDrag(this.geo, this.drag, cx, cy, shift);
            this.geo.matrix = m;
            this.binding.update(m, this.mode.tag);
            this.rebuildOverlay();
            app.requestFrame();
        } else {
            app.toolCursor = this.mode.resolveHandle(this.geo, this.overlay, this.bbox, cx, cy).cursor;
        }
    }

    pointerUp(): void {
        this.drag = null;
    }

    /** Finalize the current edit and go inactive. */
    commit(): void {
        this.binding?.commit();
        this.clear();
        app.requestFrame();
    }

    /** Abandon the current edit and go inactive. */
    cancel(): void {
        this.binding?.cancel();
        this.clear();
        app.requestFrame();
    }

    private rebuildOverlay(): void {
        if (!app.engine) return;
        const o = new OverlayBuilder(this.canvasEl);
        this.bbox = this.mode.buildOverlay(this.geo, o);
        o.push(app.engine);
        this.overlay = o;
    }

    private clear(): void {
        this.binding = null;
        this.drag = null;
        this.bbox = null;
        this.overlay = null;
        app.engine?.post('clear_overlay');
        app.toolCursor = null;
    }
}
