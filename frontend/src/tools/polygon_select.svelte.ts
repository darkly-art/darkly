/**
 * Polygon select tool: vertices are placed with discrete clicks.
 * Companion to the freehand lasso; both commit through the same
 * `select_lasso` WASM bridge (vertex list → sdf_polygon).
 *
 * Input semantics follow Krita's polygonal selection
 * (krita/libs/ui/tool/kis_tool_polyline_base.cpp):
 *   - Click:                 place a vertex.
 *   - Move:                  rubber-band line from last vertex to cursor;
 *                            snap-circle around the first vertex when the
 *                            cursor enters its snap zone and >= 3 vertices.
 *   - Click on first vertex: close (commit).
 *   - Double-click:          close at the click point.
 *   - Enter:                 close.
 *   - Backspace:             remove last vertex; clears tool when emptied.
 *   - Escape:                cancel the in-progress polygon, or clear the
 *                            selection if no polygon is being drawn.
 *
 * Modifier keys at the *closing* event map to selection mode via
 * `selectionMode(e)`: Shift = add, Alt = subtract, Shift+Alt = intersect.
 */
import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import {
    KIND_LINE, KIND_CIRCLE, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR,
    prim, selectionMode,
} from './selection_helpers';

/** Snap zone radius in buffer pixels, matching Krita's 10px screen-space
 *  threshold (kis_tool_polyline_base.cpp:26). Converted to canvas-space
 *  per use via `/ zoom`, so the on-screen hit-target stays constant. */
const SNAP_RADIUS_BUFFER_PX = 10;

/** Double-click detection thresholds. Detected manually rather than via
 *  `PointerEvent.detail`: the canvas's pointerdown handler calls
 *  `e.preventDefault()`, which suppresses the browser's click-count
 *  tracking on pointer events. */
const DBLCLICK_MS = 400;
const DBLCLICK_RADIUS_BUFFER_PX = 6;

class PolygonSelectTool extends ToolBase {
    private points: [number, number][] = [];
    private cursor: [number, number] | null = null;
    private lastClickTime = 0;
    private lastClickPos: [number, number] | null = null;

    private snapRadiusCanvas(): number {
        return SNAP_RADIUS_BUFFER_PX / this.inst.zoom;
    }

    private cursorOnFirstVertex(): boolean {
        if (this.points.length < 3 || !this.cursor) return false;
        const [fx, fy] = this.points[0];
        const dx = this.cursor[0] - fx;
        const dy = this.cursor[1] - fy;
        const r = this.snapRadiusCanvas();
        return dx * dx + dy * dy <= r * r;
    }

    private pushPreviewOverlay(): void {
        const engine = this.engine;
        if (!engine || this.points.length === 0) return;
        const prims = [];
        for (let i = 1; i < this.points.length; i++) {
            prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR,
                            this.points[i - 1], this.points[i], { thickness: 1 }));
        }
        if (this.cursor) {
            prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR,
                            this.points[this.points.length - 1], this.cursor,
                            { dashLen: 4, thickness: 1 }));
        }
        if (this.cursorOnFirstVertex()) {
            const r = this.snapRadiusCanvas();
            prims.push(prim(KIND_CIRCLE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR,
                            this.points[0], [r, 0], { thickness: 1 }));
        }
        engine.api.setOverlay({ primitives: prims });
    }

    private isDoubleClick(cx: number, cy: number, now: number): boolean {
        if (this.lastClickPos === null) return false;
        if (now - this.lastClickTime > DBLCLICK_MS) return false;
        const dx = cx - this.lastClickPos[0];
        const dy = cy - this.lastClickPos[1];
        const r = DBLCLICK_RADIUS_BUFFER_PX / this.inst.zoom;
        return dx * dx + dy * dy <= r * r;
    }

    private clearState(): void {
        this.points = [];
        this.cursor = null;
        this.lastClickTime = 0;
        this.lastClickPos = null;
        this.engine?.api.clearOverlay();
    }

    private commit(e: MouseEvent | PointerEvent | KeyboardEvent): void {
        const engine = this.engine;
        if (!engine || this.points.length < 3) {
            this.clearState();
            return;
        }
        engine.api.selectLasso({
            verts: this.points,
            mode: selectionMode(e as PointerEvent),
            antialias: true,
            feather: 0,
        });
        this.clearState();
    }

    onDeactivate(): void {
        this.clearState();
    }

    onPointerDown(e: PointerEvent, cx: number, cy: number): void {
        this.cursor = [cx, cy];
        const now = e.timeStamp;

        // Double-click closes (the first click of the pair already added a
        // vertex on the prior pointerdown).
        if (this.points.length >= 3 && this.isDoubleClick(cx, cy, now)) {
            this.commit(e);
            return;
        }

        // Click while snap-indicator is active closes.
        if (this.cursorOnFirstVertex()) {
            this.commit(e);
            return;
        }

        this.points.push([cx, cy]);
        this.lastClickTime = now;
        this.lastClickPos = [cx, cy];
        this.pushPreviewOverlay();
    }

    onPointerMove(_e: PointerEvent, cx: number, cy: number): void {
        this.cursor = [cx, cy];
        if (this.points.length > 0) this.pushPreviewOverlay();
    }

    onPointerUp(): void {
        // Vertices are placed on pointerdown; nothing to do here.
    }

    onPointerLeave(): void {
        // Drop the rubber-band so it doesn't dangle off-canvas, but keep
        // the placed vertices so the user can come back and continue.
        this.cursor = null;
        if (this.points.length > 0) this.pushPreviewOverlay();
    }

    onKeyDown(e: KeyboardEvent): boolean {
        if (e.key === 'Enter') {
            if (this.points.length >= 3) {
                this.commit(e);
            } else {
                this.clearState();
            }
            return true;
        }
        if (e.key === 'Backspace') {
            if (this.points.length === 0) return false;
            this.points.pop();
            if (this.points.length === 0) {
                this.clearState();
            } else {
                this.pushPreviewOverlay();
            }
            return true;
        }
        if (e.key === 'Escape') {
            if (this.points.length > 0) {
                this.clearState();
            } else {
                this.engine?.api.clearSelection();
            }
            return true;
        }
        return false;
    }
}

export const polygonSelectTool: ToolDescriptor = {
    id: 'polygon_select',
    group: 'select',
    cluster: 'select',
    create: (inst: DarklyInstance) => new PolygonSelectTool(inst),
};
