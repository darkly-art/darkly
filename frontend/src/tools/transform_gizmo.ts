/**
 * Generic transform gizmo: the consumer-agnostic helper.
 *
 * It takes a bounding box + an initial transform + pointer input and outputs
 * numbers (an updated affine), which it hands to whatever `TransformBinding` is
 * wired to it. It has ZERO knowledge of voids, floating, layers, or the
 * document; it imports only the overlay renderer, the affine util, and the
 * mode registry. A consumer owns a transform and supplies a binding; the gizmo
 * just drives it.
 *
 * Reads cross the async request/response transport, so `read()` is a Promise:
 * the gizmo caches the geometry, interaction uses the cache synchronously, and
 * live updates are fire-and-forget through the binding.
 */
import { app } from '../state/app.svelte';
import type { SessionEngine } from './tool_session';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import { mat3Multiply, MAT3_IDENTITY, type Mat3 } from './transform_projective';
import {
    allModes,
    modeForTag,
    pointInPolygon,
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
    /** Whether the consumer composites this transform live, every frame (a
     *  void), versus committing it once (floating raster). A live binding
     *  offers only `liveCapable` modes in the mode-switch menu. */
    readonly live: boolean;
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
    /** Live accessor for the owning instance's current tool session. Read on
     *  every overlay push/clear so the gizmo always targets the *instance's*
     *  fresh session (which survives a layer rebind), never a stale capture:
     *  the per-instance replacement for the old global `toolEngine()`. */
    private session: () => SessionEngine | null;
    private binding: TransformBinding | null = null;
    private mode: TransformMode = modeForTag(0);
    private geo: GizmoGeometry = { matrix: [...MAT3_IDENTITY], origin: [0, 0], srcW: 0, srcH: 0 };
    private drag: DragSession | null = null;
    private overlay: OverlayBuilder | null = null;
    private bbox: BBoxPolygon | null = null;

    constructor(canvasEl: HTMLCanvasElement, session: () => SessionEngine | null) {
        this.canvasEl = canvasEl;
        this.session = session;
    }

    get active(): boolean {
        return this.binding !== null;
    }

    /** Wire a consumer's binding and seed geometry from it. Returns whether the
     *  gizmo became active (the binding had a valid target). */
    async attach(binding: TransformBinding): Promise<boolean> {
        this.binding = binding;
        if ((await this.adopt(binding)) !== 'adopted') {
            // 'stale' means we were re-attached to a newer binding mid-read:
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

    /** Wire tag of the gizmo's current mode (matches the document's stored
     *  `Transform::mode_tag`). */
    get modeTag(): number {
        return this.mode.tag;
    }

    /** Modes offered for the current binding: all registered modes, minus any
     *  the binding can't render live. Empty when inactive. */
    availableModes(): { tag: number; label: string }[] {
        if (!this.binding) return [];
        const live = this.binding.live;
        return allModes()
            .filter((m) => !live || m.liveCapable)
            .map((m) => ({ tag: m.tag, label: m.label }));
    }

    /**
     * Switch the gizmo to `tag`, seeding the new mode's matrix from the current
     * geometry and **pushing it through the binding**: mode is
     * document-derived (the stored `Transform`), not a session-local flag, so
     * the next `adopt()` reads the same mode back and the gizmo stays put. A
     * no-op when inactive or already in `tag`.
     */
    setMode(tag: number): void {
        if (!this.binding || this.mode.tag === tag) return;
        const target = modeForTag(tag);
        const m = target.seedMatrix(this.geo);
        this.mode = target;
        this.applyMatrix(m);
    }

    /**
     * Mirror the content about the source rect's centre, horizontally (`'h'`) or
     * vertically (`'v'`): Krita's transform-tool `Mirror Horizontal` /
     * `Mirror Vertical` (`kis_tool_transform_config_widget.cpp::slotFlipX`,
     * which negates `scaleX` around the anchor).
     *
     * Composing the mirror in *source* space (`M · mirror`) leaves the
     * destination quad exactly where it is and only swaps which side of the
     * content lands on which edge. That makes it exact and mode-agnostic: every
     * mode's matrix maps the source rect onto that quad, so the same
     * composition mirrors an affine box and a perspective quad alike.
     */
    flip(axis: 'h' | 'v'): void {
        const { srcW, srcH } = this.geo;
        if (!this.binding || srcW <= 0 || srcH <= 0) return;
        const mirror: Mat3 =
            axis === 'h'
                ? [-1, 0, srcW, 0, 1, 0, 0, 0, 1]
                : [1, 0, 0, 0, -1, srcH, 0, 0, 1];
        this.applyMatrix(mat3Multiply(this.geo.matrix, mirror));
    }

    pointerMove(cx: number, cy: number, shift: boolean): void {
        if (!this.binding) return;
        if (this.drag != null) {
            this.applyMatrix(this.mode.updateDrag(this.geo, this.drag, cx, cy, shift));
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

    /** Adopt `m` as the current transform: store it, push it through the binding
     *  under the active mode, and redraw. The one path every matrix edit (drag,
     *  mode switch, flip) takes. */
    private applyMatrix(m: Mat3): void {
        if (!this.binding) return;
        this.geo.matrix = m;
        this.binding.update(m, this.mode.tag);
        this.rebuildOverlay();
        app.requestFrame();
    }

    private rebuildOverlay(): void {
        const engine = this.session();
        if (!engine) return;
        const o = new OverlayBuilder(this.canvasEl);
        this.bbox = this.mode.buildOverlay(this.geo, o);
        o.push(engine);
        this.overlay = o;
    }

    private clear(): void {
        this.binding = null;
        this.drag = null;
        this.bbox = null;
        this.overlay = null;
        this.session()?.api.clearOverlay();
        app.toolCursor = null;
    }
}
