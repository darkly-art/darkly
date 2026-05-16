/**
 * Lasso (freehand polygon) select tool.
 * Click and drag to draw a freehand selection boundary. The polygon is
 * automatically closed on mouse up and rasterized via SDF.
 * Modifier keys control boolean mode:
 *   - No modifier: replace selection
 *   - Shift: add to selection
 *   - Alt: subtract from selection
 *   - Shift+Alt: intersect with selection
 * Escape clears the selection.
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { KIND_LINE, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim, selectionMode } from './selection_helpers';

let points: [number, number][] = [];

/** Minimum squared distance between consecutive points to avoid redundancy. */
const MIN_DIST_SQ = 4;

function pushPreviewOverlay() {
    if (!app.handle || points.length < 2) return;
    const prims = [];
    for (let i = 1; i < points.length; i++) {
        prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, points[i - 1], points[i], { thickness: 1 }));
    }
    // Closing line back to start
    prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, points[points.length - 1], points[0], { dashLen: 4, thickness: 1 }));
    app.handle.set_overlay(prims);
}

function clearPreviewOverlay() {
    points = [];
    app.handle?.clear_overlay();
}

export const lassoSelectTool: Tool = {
    id: 'lasso_select',
    faIcon: 'fa-solid fa-draw-polygon',
    group: 'select',
    hotkeyAction: 'lassoSelectTool',

    onDeactivate() {
        clearPreviewOverlay();
    },

    onPointerDown(_ctx, _e, cx, cy) {
        points = [[cx, cy]];
        pushPreviewOverlay();
    },

    onPointerMove(_ctx, _e, cx, cy) {
        if (points.length === 0) return;
        const last = points[points.length - 1];
        const dx = cx - last[0];
        const dy = cy - last[1];
        if (dx * dx + dy * dy >= MIN_DIST_SQ) {
            points.push([cx, cy]);
            pushPreviewOverlay();
        }
    },

    onPointerUp(_ctx, e) {
        if (points.length < 3 || !app.handle) {
            if (points.length < 3 && selectionMode(e) === 'replace') {
                app.handle?.clear_selection();
            }
            clearPreviewOverlay();
            return;
        }

        const mode = selectionMode(e);
        app.handle.select_lasso(points, mode, true, 0);
        clearPreviewOverlay();
    },

    onKeyDown(e) {
        if (e.key === 'Escape') {
            app.handle?.clear_selection();
            return true;
        }
        return false;
    },
};
