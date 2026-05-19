/**
 * Polygon select tool — vertices are placed with discrete clicks.
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
 * `selectionMode(e)` — Shift = add, Alt = subtract, Shift+Alt = intersect.
 */
import type { Tool } from './registry';
import { app } from '../state/app.svelte';
import {
    KIND_LINE, KIND_CIRCLE, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR,
    prim, selectionMode,
} from './selection_helpers';

let points: [number, number][] = [];
let cursor: [number, number] | null = null;
let lastClickTime = 0;
let lastClickPos: [number, number] | null = null;

/** Snap zone radius in buffer pixels — matches Krita's 10px screen-space
 *  threshold (kis_tool_polyline_base.cpp:26). Converted to canvas-space
 *  per use via `/ app.zoom`, so the on-screen hit-target stays constant. */
const SNAP_RADIUS_BUFFER_PX = 10;

/** Double-click detection thresholds. Detected manually rather than via
 *  `PointerEvent.detail`: the canvas's pointerdown handler calls
 *  `e.preventDefault()`, which suppresses the browser's click-count
 *  tracking on pointer events. */
const DBLCLICK_MS = 400;
const DBLCLICK_RADIUS_BUFFER_PX = 6;

function snapRadiusCanvas(): number {
    return SNAP_RADIUS_BUFFER_PX / app.zoom;
}

function cursorOnFirstVertex(): boolean {
    if (points.length < 3 || !cursor) return false;
    const [fx, fy] = points[0];
    const dx = cursor[0] - fx;
    const dy = cursor[1] - fy;
    const r = snapRadiusCanvas();
    return dx * dx + dy * dy <= r * r;
}

function pushPreviewOverlay() {
    if (!app.handle || points.length === 0) return;
    const prims = [];
    for (let i = 1; i < points.length; i++) {
        prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR,
                        points[i - 1], points[i], { thickness: 1 }));
    }
    if (cursor) {
        prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR,
                        points[points.length - 1], cursor,
                        { dashLen: 4, thickness: 1 }));
    }
    if (cursorOnFirstVertex()) {
        const r = snapRadiusCanvas();
        prims.push(prim(KIND_CIRCLE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR,
                        points[0], [r, 0], { thickness: 1 }));
    }
    app.handle.set_overlay(prims);
}

function isDoubleClick(cx: number, cy: number, now: number): boolean {
    if (lastClickPos === null) return false;
    if (now - lastClickTime > DBLCLICK_MS) return false;
    const dx = cx - lastClickPos[0];
    const dy = cy - lastClickPos[1];
    const r = DBLCLICK_RADIUS_BUFFER_PX / app.zoom;
    return dx * dx + dy * dy <= r * r;
}

function clearState() {
    points = [];
    cursor = null;
    lastClickTime = 0;
    lastClickPos = null;
    app.handle?.clear_overlay();
}

function commit(e: MouseEvent | PointerEvent | KeyboardEvent) {
    if (!app.handle || points.length < 3) {
        clearState();
        return;
    }
    app.handle.select_lasso(points, selectionMode(e as PointerEvent), true, 0);
    clearState();
}

export const polygonSelectTool: Tool = {
    id: 'polygon_select',
    faIcon: 'fa-solid fa-draw-polygon',
    group: 'select',
    cluster: 'select',
    hotkeyAction: 'polygonSelectTool',

    onDeactivate() {
        clearState();
    },

    onPointerDown(_ctx, e, cx, cy) {
        cursor = [cx, cy];
        const now = e.timeStamp;

        // Double-click closes (the first click of the pair already added a
        // vertex on the prior pointerdown).
        if (points.length >= 3 && isDoubleClick(cx, cy, now)) {
            commit(e);
            return;
        }

        // Click while snap-indicator is active closes.
        if (cursorOnFirstVertex()) {
            commit(e);
            return;
        }

        points.push([cx, cy]);
        lastClickTime = now;
        lastClickPos = [cx, cy];
        pushPreviewOverlay();
    },

    onPointerMove(_ctx, _e, cx, cy) {
        cursor = [cx, cy];
        if (points.length > 0) pushPreviewOverlay();
    },

    onPointerUp() {
        // Vertices are placed on pointerdown; nothing to do here.
    },

    onPointerLeave() {
        // Drop the rubber-band so it doesn't dangle off-canvas, but keep
        // the placed vertices so the user can come back and continue.
        cursor = null;
        if (points.length > 0) pushPreviewOverlay();
    },

    onKeyDown(e) {
        if (e.key === 'Enter') {
            if (points.length >= 3) {
                commit(e);
            } else {
                clearState();
            }
            return true;
        }
        if (e.key === 'Backspace') {
            if (points.length === 0) return false;
            points.pop();
            if (points.length === 0) {
                clearState();
            } else {
                pushPreviewOverlay();
            }
            return true;
        }
        if (e.key === 'Escape') {
            if (points.length > 0) {
                clearState();
            } else {
                app.handle?.clear_selection();
            }
            return true;
        }
        return false;
    },
};
