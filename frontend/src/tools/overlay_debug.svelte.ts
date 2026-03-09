/**
 * POC tool for testing the GPU overlay system.
 * Click to place a crosshair, drag to draw a line.
 * Each interaction adds primitives; press Escape to clear.
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

// GPU overlay constants (must match Rust/WGSL)
const KIND_LINE          = 0;
const KIND_CIRCLE        = 1;
const KIND_RECT          = 2;
const KIND_DASHED_LINE   = 3;
const KIND_FILLED_RECT   = 4;
const KIND_FILLED_CIRCLE = 5;

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
        thickness: opts?.thickness ?? 2,
        dashLen: opts?.dashLen ?? 0,
        dashOffset: opts?.dashOffset ?? 0,
        cornerRadius: opts?.cornerRadius ?? 0,
    };
}

let prims: GpuPrim[] = [];
let dragStart: [number, number] | null = null;
let liveEnd: [number, number] | null = null;

function pushOverlay() {
    if (!app.handle) return;
    const all = [...prims];
    // Add live drag line if dragging
    if (dragStart && liveEnd) {
        all.push(prim(KIND_DASHED_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, dragStart, liveEnd, { thickness: 2, dashLen: 10 }));
    }
    app.handle.set_overlay(all);
}

function clearAll() {
    prims = [];
    dragStart = null;
    liveEnd = null;
    app.handle?.clear_overlay();
}

export const overlayDebugTool: Tool = {
    id: 'overlay_debug',
    name: 'Overlay Debug',
    icon: '+',
    hotkeyAction: 'overlayDebugTool',

    onDeactivate() {
        clearAll();
    },

    onPointerDown(_ctx, _e, cx, cy) {
        dragStart = [cx, cy];
        liveEnd = [cx, cy];

        // Place a small crosshair at click point
        const s = 8;
        prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, [cx - s, cy], [cx + s, cy]));
        prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, [cx, cy - s], [cx, cy + s]));
        prims.push(prim(KIND_FILLED_CIRCLE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, [cx, cy], [3, 0]));

        pushOverlay();
    },

    onPointerMove(_ctx, _e, cx, cy) {
        if (!dragStart) return;
        liveEnd = [cx, cy];
        pushOverlay();
    },

    onPointerUp(_ctx, _e) {
        if (dragStart && liveEnd) {
            const [x0, y0] = dragStart;
            const [x1, y1] = liveEnd;
            const dx = x1 - x0, dy = y1 - y0;
            if (dx * dx + dy * dy > 16) {
                // Commit the drag line as a solid line
                prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, dragStart, liveEnd));
                // Add a rect from drag start to end
                const tl: [number, number] = [Math.min(x0, x1), Math.min(y0, y1)];
                const br: [number, number] = [Math.max(x0, x1), Math.max(y0, y1)];
                prims.push(prim(KIND_RECT, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, tl, br, { thickness: 1, cornerRadius: 4 }));
            }
        }
        dragStart = null;
        liveEnd = null;
        pushOverlay();
    },

    onKeyDown(e) {
        if (e.key === 'Escape') {
            clearAll();
            return true;
        }
        return false;
    },
};
