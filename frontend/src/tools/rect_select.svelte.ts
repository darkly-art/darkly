/**
 * Rectangle select tool.
 * Drag to create a rectangular selection. Modifier keys control boolean mode:
 *   - No modifier: replace selection
 *   - Shift: add to selection
 *   - Alt: subtract from selection
 *   - Shift+Alt: intersect with selection
 * Escape clears the selection.
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { KIND_RECT, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim, selectionMode } from './selection_helpers';

let dragStart: [number, number] | null = null;
let dragEnd: [number, number] | null = null;

function pushPreviewOverlay() {
    if (!app.handle || !dragStart || !dragEnd) return;
    const [x0, y0] = dragStart;
    const [x1, y1] = dragEnd;
    const tl: [number, number] = [Math.min(x0, x1), Math.min(y0, y1)];
    const br: [number, number] = [Math.max(x0, x1), Math.max(y0, y1)];
    app.handle.set_overlay([
        prim(KIND_RECT, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, tl, br, { dashLen: 6, thickness: 1 }),
    ]);
}

function clearPreviewOverlay() {
    dragStart = null;
    dragEnd = null;
    app.handle?.clear_overlay();
}

export const rectSelectTool: Tool = {
    id: 'rect_select',
    faIcon: 'fa-solid fa-square-dashed',
    group: 'select',
    cluster: 'select',
    hotkeyAction: 'rectSelectTool',

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

        // Only commit if the rect has meaningful size
        if (w > 1 && h > 1) {
            const mode = selectionMode(e);
            app.handle.select_rect(x, y, w, h, mode, true, 0);
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
