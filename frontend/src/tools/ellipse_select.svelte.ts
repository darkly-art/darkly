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

function pushPreviewOverlay() {
    if (!app.handle || !dragStart || !dragEnd) return;
    const [x0, y0] = dragStart;
    const [x1, y1] = dragEnd;
    const cx = (x0 + x1) / 2;
    const cy = (y0 + y1) / 2;
    const rx = Math.abs(x1 - x0) / 2;
    const ry = Math.abs(y1 - y0) / 2;
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
    faIcon: 'fa-regular fa-circle',
    group: 'select',
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
        const x = Math.min(x0, x1);
        const y = Math.min(y0, y1);
        const w = Math.abs(x1 - x0);
        const h = Math.abs(y1 - y0);

        // Only commit if the ellipse has meaningful size
        if (w > 1 && h > 1) {
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
