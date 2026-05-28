/**
 * Ellipse select tool.
 * Drag to create an elliptical selection (inscribed in the drag rectangle).
 * Modifier keys control boolean mode:
 *   - No modifier: replace selection
 *   - Shift: add to selection
 *   - Alt: subtract from selection
 *   - Shift+Alt: intersect with selection
 * Escape clears the selection.
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { KIND_ELLIPSE, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim, selectionMode } from './selection_helpers';

let dragStart: [number, number] | null = null;
let dragEnd: [number, number] | null = null;

// Krita-style integer-pixel snapping of the bounding rect (see
// `kis_tool_select_elliptical.cc`). The ellipse boundary itself is curved,
// so antialiasing stays on at commit time — only the bbox is snapped.
function pushPreviewOverlay() {
    if (!app.handle || !dragStart || !dragEnd) return;
    const [x0, y0] = dragStart;
    const [x1, y1] = dragEnd;
    const sx0 = Math.round(x0);
    const sy0 = Math.round(y0);
    const sx1 = Math.round(x1);
    const sy1 = Math.round(y1);
    const cx = (sx0 + sx1) / 2;
    const cy = (sy0 + sy1) / 2;
    const rx = Math.abs(sx1 - sx0) / 2;
    const ry = Math.abs(sy1 - sy0) / 2;
    app.handle.set_overlay([
        prim(KIND_ELLIPSE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, [cx, cy], [rx, ry], { dashLen: 6, thickness: 1 }),
    ]);
}

function clearPreviewOverlay() {
    dragStart = null;
    dragEnd = null;
    app.handle?.clear_overlay();
}

export const ellipseSelectTool: Tool = {
    id: 'ellipse_select',
    faIcon: 'fa-solid fa-circle-dashed',
    group: 'select',
    cluster: 'select',
    hotkeyAction: 'ellipseSelectTool',

    onDeactivate() {
        clearPreviewOverlay();
    },

    onPointerDown(_ctx, _e, cx, cy) {
        dragStart = [cx, cy];
        dragEnd = [cx, cy];
        pushPreviewOverlay();
    },

    onPointerMove(_ctx, _e, cx, cy) {
        if (!dragStart) return;
        dragEnd = [cx, cy];
        pushPreviewOverlay();
    },

    onPointerUp(_ctx, e) {
        if (!dragStart || !dragEnd || !app.handle) {
            clearPreviewOverlay();
            return;
        }

        const [x0, y0] = dragStart;
        const [x1, y1] = dragEnd;
        const sx0 = Math.round(x0);
        const sy0 = Math.round(y0);
        const sx1 = Math.round(x1);
        const sy1 = Math.round(y1);
        const x = Math.min(sx0, sx1);
        const y = Math.min(sy0, sy1);
        const w = Math.abs(sx1 - sx0);
        const h = Math.abs(sy1 - sy0);

        // Only commit if the snapped bbox has meaningful size.
        if (w > 0 && h > 0) {
            const mode = selectionMode(e);
            app.handle.select_ellipse(x, y, w, h, mode, true, 0);
        } else if (selectionMode(e) === 'replace') {
            // Click without drag = deselect (only in replace mode)
            app.handle.clear_selection();
        }

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
