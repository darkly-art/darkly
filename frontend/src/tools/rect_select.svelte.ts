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

// Krita-style integer-pixel snapping: rectangle selections always commit to
// pixel-aligned bounds (mirrors `QRectF::toRect()` in Krita's
// `KisToolSelectRectangular::finishRect`). The preview overlay snaps too so
// what the user sees during the drag matches what they get on release.
function pushPreviewOverlay() {
    if (!app.handle || !dragStart || !dragEnd) return;
    const [x0, y0] = dragStart;
    const [x1, y1] = dragEnd;
    const sx0 = Math.round(x0);
    const sy0 = Math.round(y0);
    const sx1 = Math.round(x1);
    const sy1 = Math.round(y1);
    const tl: [number, number] = [Math.min(sx0, sx1), Math.min(sy0, sy1)];
    const br: [number, number] = [Math.max(sx0, sx1), Math.max(sy0, sy1)];
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
        const sx0 = Math.round(x0);
        const sy0 = Math.round(y0);
        const sx1 = Math.round(x1);
        const sy1 = Math.round(y1);
        const x = Math.min(sx0, sx1);
        const y = Math.min(sy0, sy1);
        const w = Math.abs(sx1 - sx0);
        const h = Math.abs(sy1 - sy0);

        // Only commit if the snapped rect has meaningful size. `antialias`
        // is off — pixel-aligned bounds need no SDF smoothing and the result
        // is a crisp 1-bit mask.
        if (w > 0 && h > 0) {
            const mode = selectionMode(e);
            app.handle.select_rect(x, y, w, h, mode, false, 0);
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
