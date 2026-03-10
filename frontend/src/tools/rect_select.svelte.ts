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

// GPU overlay constants (must match Rust/WGSL)
const KIND_RECT          = 2;
const KIND_DASHED_LINE   = 3;

const FLAG_CANVAS_SPACE  = 1;
const FLAG_INVERT_COLOR  = 2;

interface GpuPrim {
    kind: number;
    flags: number;
    p0: [number, number];
    p1: [number, number];
    color: [number, number, number, number];
    thickness: number;
    dashLen: number;
    dashOffset: number;
    cornerRadius: number;
}

function prim(kind: number, flags: number, p0: [number, number], p1: [number, number], opts?: Partial<GpuPrim>): GpuPrim {
    return {
        kind, flags, p0, p1,
        color: opts?.color ?? [1, 1, 1, 1],
        thickness: opts?.thickness ?? 1,
        dashLen: opts?.dashLen ?? 0,
        dashOffset: opts?.dashOffset ?? 0,
        cornerRadius: opts?.cornerRadius ?? 0,
    };
}

let dragStart: [number, number] | null = null;
let dragEnd: [number, number] | null = null;

function selectionMode(e: PointerEvent | MouseEvent): string {
    if (e.shiftKey && e.altKey) return 'intersect';
    if (e.shiftKey) return 'add';
    if (e.altKey) return 'subtract';
    return 'replace';
}

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
    name: 'Rectangle Select',
    icon: '⬚',
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
