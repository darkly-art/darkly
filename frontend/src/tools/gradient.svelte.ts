import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { OverlayBuilder } from '../canvas/gpu_overlay';

// --- Reactive state ---

let startX = $state(0);
let startY = $state(0);
let endX = $state(0);
let endY = $state(0);
let isDrawing = $state(false);
let hasPlacement = $state(false);

// Click-vs-drag detection: when clicking on the canvas with an active
// placement, we defer starting a new gradient until a drag threshold is
// exceeded.  If the pointer releases first, we just dismiss.
const DRAG_THRESHOLD = 4; // screen pixels
let pending: { screenX: number; screenY: number; cx: number; cy: number } | null = null;

/** Which handle is being dragged ('start' | 'end'), or null. */
let draggingHandle: string | null = null;

let canvasEl: HTMLCanvasElement | null = null;
let overlay: OverlayBuilder | null = null;

function applyGradient() {
    const layerId = app.activeLayerId;
    if (!layerId || !app.handle) return;

    const c = app.foreground;
    const bg = app.background;

    app.handle.begin_stroke(layerId);
    app.handle.stroke_to('linear_gradient', {
        x0: startX, y0: startY,
        x1: endX, y1: endY,
        r0: c.r, g0: c.g, b0: c.b, a0: c.a,
        r1: bg.r, g1: bg.g, b1: bg.b, a1: bg.a,
    });
    app.handle.end_stroke();
}

function clearPlacement() {
    isDrawing = false;
    hasPlacement = false;
    pending = null;
    draggingHandle = null;
    overlay = null;
    app.handle?.clear_overlay();
    app.toolCursor = null;
}

function beginDrawing(cx: number, cy: number) {
    startX = cx;
    startY = cy;
    endX = cx;
    endY = cy;
    isDrawing = true;
}

function buildOverlay(): OverlayBuilder | null {
    if ((!isDrawing && !hasPlacement) || !canvasEl || !app.handle) return null;

    const o = new OverlayBuilder(canvasEl);
    o.line([startX, startY], [endX, endY]);
    o.handle([startX, startY], { id: 'start', cursor: 'grab', fill: '#4af', stroke: '#fff' });
    o.handle([endX, endY],     { id: 'end',   cursor: 'grab', fill: '#fa4', stroke: '#fff' });
    o.push(app.handle);
    return o;
}

// --- Tool definition ---

export const gradientTool: Tool = {
    id: 'gradient',
    name: 'Gradient',
    faIcon: 'fa-solid fa-bars-staggered',
    group: 'paint',
    hotkeyAction: 'gradientTool',

    onActivate(ctx) {
        canvasEl = ctx.canvasEl;
    },

    onDeactivate() {
        clearPlacement();
    },

    dismissOverlay() {
        clearPlacement();
    },

    onPointerDown(_ctx, e, cx, cy) {
        if (!app.activeLayerId) return;

        // Check if clicking on an existing handle
        if (hasPlacement && overlay) {
            const hit = overlay.hitTest(cx, cy);
            if (hit) {
                draggingHandle = hit.id;
                return;
            }
        }

        if (hasPlacement) {
            // Defer: might be click-to-dismiss or drag-to-start-new
            pending = { screenX: e.clientX, screenY: e.clientY, cx, cy };
            return;
        }

        beginDrawing(cx, cy);
    },

    onPointerMove(_ctx, e, cx, cy) {
        // Handle drag on an endpoint
        if (draggingHandle) {
            if (draggingHandle === 'start') { startX = cx; startY = cy; }
            else { endX = cx; endY = cy; }
            app.requestFrame();
            return;
        }

        if (pending) {
            const dx = e.clientX - pending.screenX;
            const dy = e.clientY - pending.screenY;
            if (dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD) {
                const start = pending;
                clearPlacement();
                beginDrawing(start.cx, start.cy);
                endX = cx;
                endY = cy;
            }
            return;
        }

        if (isDrawing) {
            endX = cx;
            endY = cy;
        } else if (hasPlacement && overlay) {
            // Hover cursor feedback
            const hit = overlay.hitTest(cx, cy);
            app.toolCursor = hit?.cursor ?? null;
        }
    },

    onPointerUp(ctx, e) {
        if (draggingHandle) {
            draggingHandle = null;
            if (hasPlacement) applyGradient();
            app.requestFrame();
            return;
        }

        if (pending) {
            clearPlacement();
            return;
        }
        if (!isDrawing) return;
        const pos = ctx.screenToCanvas(e.clientX, e.clientY);
        endX = pos.x;
        endY = pos.y;
        isDrawing = false;
        hasPlacement = true;
        applyGradient();
    },

    onFrame() {
        if (isDrawing || hasPlacement) {
            overlay = buildOverlay();
        }
    },
};
