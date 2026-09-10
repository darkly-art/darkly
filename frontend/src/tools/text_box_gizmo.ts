/**
 * Interactive text-box gizmo: the on-canvas frame for an area-text object.
 *
 * Structurally a sibling of the transform gizmo ([transform_gizmo.ts]): it
 * reads an object's geometry, draws a frame + handles via the GPU
 * `OverlayBuilder`, and hit-tests handles, but its *edit* is a box resize, not
 * an affine: dragging a handle changes the object's layout box (`box_size`) and
 * moves its origin, reflowing the text. The opposite edge stays put.
 *
 * Geometry comes from `vector_object_info` as a row-major canvas affine
 * `G = layer · obj` ([transform_affine.ts] `Affine2D`) plus the box size
 * `(w, h)`. A resize keeps `G`'s linear part fixed (no glyph scaling) and
 * changes only the translation (the moved top-left origin) and `(w, h)`; the
 * new `G` + box go back through `set_text_box`, which strips the layer
 * transform exactly like `update_vector_object_transform`.
 */
import { app } from '../state/app.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';
import {
    affineTransform,
    affineInverse,
    type Affine2D,
} from './transform_affine';

/** A box handle: corners + edge midpoints. The letters encode which local
 *  edges the handle drives (`n`/`s` = top/bottom, `w`/`e` = left/right). */
export type HandleId = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';

export const HANDLES: readonly HandleId[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

const CURSORS: Record<HandleId, string> = {
    nw: 'nwse-resize', n: 'ns-resize', ne: 'nesw-resize', e: 'ew-resize',
    se: 'nwse-resize', s: 'ns-resize', sw: 'nesw-resize', w: 'ew-resize',
};

/** Minimum box extent in local units, so a resize can't collapse the box. */
export const MIN_BOX = 8;

/** A box's geometry: its local→canvas affine and its local size. */
export interface BoxGeo {
    G: Affine2D;
    w: number;
    h: number;
}

/** The local-space anchor of a handle, in `[0..w] × [0..h]`. */
export function handleLocal(id: HandleId, w: number, h: number): [number, number] {
    const x = id.includes('w') ? 0 : id.includes('e') ? w : w / 2;
    const y = id.includes('n') ? 0 : id.includes('s') ? h : h / 2;
    return [x, y];
}

/**
 * Resolve a handle drag to a new {@link BoxGeo}. `geo0` is the geometry at
 * drag-start; `(cx, cy)` is the pointer in canvas space. The dragged edges move
 * to the pointer's local coordinate; the opposite edges stay fixed; the box is
 * clamped to {@link MIN_BOX}. `G`'s linear part is preserved: only the origin
 * (translation) and size change. Pure, so it can be unit-tested directly.
 * Returns `null` only if `geo0.G` is singular.
 */
export function resizeBox(geo0: BoxGeo, id: HandleId, cx: number, cy: number): BoxGeo | null {
    const Gi = affineInverse(geo0.G);
    if (!Gi) return null;
    const [lx, ly] = affineTransform(Gi, cx, cy);

    let x0 = 0;
    let y0 = 0;
    let x1 = geo0.w;
    let y1 = geo0.h;
    if (id.includes('w')) x0 = Math.min(lx, x1 - MIN_BOX);
    if (id.includes('e')) x1 = Math.max(lx, x0 + MIN_BOX);
    if (id.includes('n')) y0 = Math.min(ly, y1 - MIN_BOX);
    if (id.includes('s')) y1 = Math.max(ly, y0 + MIN_BOX);

    const w = x1 - x0;
    const h = y1 - y0;
    // The moved top-left in canvas space: its linear basis is unchanged, so the
    // new affine is `G` with a relabelled translation. The fixed corner is
    // therefore pinned (see file header).
    const [ox, oy] = affineTransform(geo0.G, x0, y0);
    const G: Affine2D = [geo0.G[0], geo0.G[1], ox, geo0.G[3], geo0.G[4], oy];
    return { G, w, h };
}

interface ResizeDrag {
    id: HandleId;
    geo0: BoxGeo;
}

export class TextBoxGizmo {
    private canvasEl: HTMLCanvasElement;
    private layerId: number | null = null;
    private objectId: number | null = null;
    private geo: BoxGeo | null = null;
    private overlay: OverlayBuilder | null = null;
    private drag: ResizeDrag | null = null;

    constructor(canvasEl: HTMLCanvasElement) {
        this.canvasEl = canvasEl;
    }

    get active(): boolean {
        return this.geo !== null;
    }

    get dragging(): boolean {
        return this.drag !== null;
    }

    isTarget(layerId: number, objectId: number): boolean {
        return this.layerId === layerId && this.objectId === objectId;
    }

    /** Bind to an object and draw its box. Resolves false (and clears) if the
     *  object has no geometry. */
    async attach(layerId: number, objectId: number): Promise<boolean> {
        this.layerId = layerId;
        this.objectId = objectId;
        if (!(await this.readGeo())) {
            this.detach();
            return false;
        }
        this.rebuildOverlay();
        return true;
    }

    private async readGeo(): Promise<boolean> {
        if (!app.engine || this.layerId === null || this.objectId === null) return false;
        const info = await app.engine.api.vectorObjectInfo({
            id: this.layerId,
            object: this.objectId,
        });
        // A stale read can land after detach/re-attach; ignore it.
        if (!info || this.layerId === null || this.objectId === null) return false;
        this.geo = { G: info.matrix as Affine2D, w: info.w, h: info.h };
        return true;
    }

    /** Re-sync geometry from the engine when idle, so panel style edits and
     *  undo/redo reflect in the frame (mirrors `TransformGizmo.frame`). */
    async frame(): Promise<void> {
        if (!this.active || this.drag) return;
        if (await this.readGeo()) this.rebuildOverlay();
        else this.detach();
    }

    detach(): void {
        const was = this.active;
        this.layerId = null;
        this.objectId = null;
        this.geo = null;
        this.overlay = null;
        this.drag = null;
        if (was) app.engine?.api.clearOverlay();
        if (app.toolCursor) app.toolCursor = null;
    }

    private cornersCanvas(): [number, number][] {
        const { G, w, h } = this.geo!;
        return ([[0, 0], [w, 0], [w, h], [0, h]] as [number, number][]).map(([x, y]) =>
            affineTransform(G, x, y),
        );
    }

    private rebuildOverlay(): void {
        if (!app.engine || !this.geo) return;
        const o = new OverlayBuilder(this.canvasEl);
        const c = this.cornersCanvas();
        for (let i = 0; i < 4; i++) {
            o.line(c[i], c[(i + 1) % 4], { color: '#4af', thickness: 1.5 });
        }
        for (const id of HANDLES) {
            const [lx, ly] = handleLocal(id, this.geo.w, this.geo.h);
            o.handle(affineTransform(this.geo.G, lx, ly), { id, cursor: CURSORS[id] });
        }
        o.push(app.engine);
        this.overlay = o;
    }

    /** Begin a resize if `(cx, cy)` lands on a handle. Returns whether it did. */
    pointerDown(cx: number, cy: number): boolean {
        if (!this.geo || !this.overlay) return false;
        const hit = this.overlay.hitTest(cx, cy);
        if (!hit) return false;
        this.drag = {
            id: hit.id as HandleId,
            geo0: { G: [...this.geo.G] as Affine2D, w: this.geo.w, h: this.geo.h },
        };
        return true;
    }

    pointerMove(cx: number, cy: number): void {
        if (!this.geo) return;
        if (this.drag) {
            const next = resizeBox(this.drag.geo0, this.drag.id, cx, cy);
            if (!next) return;
            this.geo = next;
            app.engine?.api.setTextBox({
                id: this.layerId!,
                object: this.objectId!,
                matrix: next.G,
                box: [next.w, next.h],
            });
            this.rebuildOverlay();
            app.requestFrame();
        } else if (this.overlay) {
            const hit = this.overlay.hitTest(cx, cy);
            app.toolCursor = hit ? CURSORS[hit.id as HandleId] : null;
        }
    }

    pointerUp(): void {
        this.drag = null;
    }
}
