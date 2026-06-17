/**
 * Generic transform gizmo — the consumer-agnostic helper.
 *
 * It takes a bounding box + an initial transform + pointer input and outputs
 * numbers (an updated affine), which it hands to whatever `TransformBinding` is
 * wired to it. It has ZERO knowledge of voids, floating, layers, or the
 * document — it imports only the overlay renderer, the affine util, and the
 * mode registry. A consumer owns a transform and supplies a binding; the gizmo
 * just drives it.
 */
import { app } from '../state/app.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import { IDENTITY, type Affine2D } from './transform_affine';
import {
    modeForTag,
    type BBoxPolygon,
    type DragSession,
    type GizmoGeometry,
    type TransformMode,
} from './transform_modes';

/**
 * The seam between the gizmo and a consumer. The gizmo reads the current bbox +
 * transform, emits live updates while dragging, and commits/cancels on exit.
 * Implementations live next to their consumers (see `transform_bindings.ts`).
 */
export interface TransformBinding {
    /** Current bbox + transform, or `null` if the target is no longer valid. */
    read(): {
        origin: [number, number];
        w: number;
        h: number;
        mode: number;
        affine: Affine2D;
    } | null;
    /** Live preview: push an updated affine for the given mode. */
    update(affine: Affine2D, modeTag: number): void;
    /** Finalize. */
    commit(): void;
    /** Abandon (restore the pre-edit state). */
    cancel(): void;
}

export class TransformGizmo {
    private canvasEl: HTMLCanvasElement;
    private binding: TransformBinding | null = null;
    private mode: TransformMode = modeForTag(0);
    private geo: GizmoGeometry = { matrix: [...IDENTITY], origin: [0, 0], srcW: 0, srcH: 0 };
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
    attach(binding: TransformBinding): boolean {
        this.binding = binding;
        if (!this.adopt()) {
            this.clear();
            return false;
        }
        this.rebuildOverlay();
        return true;
    }

    /** Pull the current bbox + transform from the binding into local geometry.
     *  Returns false (and leaves geometry stale) if the target is gone. */
    private adopt(): boolean {
        const info = this.binding?.read();
        if (!info) return false;
        this.geo = {
            matrix: [...info.affine],
            origin: [...info.origin],
            srcW: info.w,
            srcH: info.h,
        };
        this.mode = modeForTag(info.mode);
        return true;
    }

    /** Per-frame reconcile: drop if the target vanished (e.g. floating
     *  committed by an unrelated edit / undo), otherwise resync geometry when
     *  idle (so an external change like undo is reflected) and redraw. */
    frame(): void {
        if (!this.binding) return;
        if (!this.drag && !this.adopt()) {
            this.clear();
            return;
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

    pointerMove(cx: number, cy: number, shift: boolean): void {
        if (!this.binding) return;
        if (this.drag != null) {
            const m = this.mode.updateDrag(this.geo, this.drag, cx, cy, shift);
            this.geo.matrix = m;
            this.binding.update(m, this.mode.tag);
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
        if (!app.handle) return;
        const o = new OverlayBuilder(this.canvasEl);
        this.bbox = this.mode.buildOverlay(this.geo, o);
        o.push(app.handle);
        this.overlay = o;
    }

    private clear(): void {
        this.binding = null;
        this.drag = null;
        this.bbox = null;
        this.overlay = null;
        app.handle?.clear_overlay();
        app.toolCursor = null;
    }
}
